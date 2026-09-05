//! 増分 update と鮮度判定 (meta / dir ゲート / heal)。

use super::*;

// ─────────────────────────── update (増分 index) ───────────────────────────

/// 既存 index を開き、内容 hash が変わった / 追加 / 削除されたファイルだけ再 index する。
/// 未変更ファイルは再パースしない (増分の肝)。名前解決の一貫性は、変更で影響を受けた
/// symbol 名 (`affected`) の incoming call を再解決することで保つ ⇒ full 再 index と同一結果。
pub fn run_update(dir: &str, path: &str) {
    let dir = &abs_dir(dir);
    eprintln!("=== kenning update: {} → {} ===", dir, path);
    // 既存 DB を書込可能で開く (drop で永続 / entity_in は新 eid を再発行)。
    // db ファイルがまだ無ければ full index にフォールバック (update = 初回でも動く)。
    // ※ ファイルが在るのに open 失敗 (= lock 等) は full にしない — 消して作り直す事故を防ぐ。
    let t = Instant::now();
    let db = match Database::open(path) {
        Ok(db) => db,
        Err(e) => {
            if std::path::Path::new(path).exists() {
                eprintln!("# index を開けない ({e})。他プロセス使用中かも → 後で再試行を。");
            } else {
                eprintln!("(index が無いので full index します)");
                ensure_index(dir, path, scip_sidecar_of(path).as_deref());
            }
            return;
        }
    };
    let open = t.elapsed();
    update_with_heal(db, dir, path, UpdateScan::Walk { trust_mtime: false }, "update"); // 明示 update = 全件 hash 照合 (mtime 巻き戻しの答え合わせ)
    eprintln!("(update 内訳: rw open {open:?} / 走査+書込+drop {:?})", t.elapsed() - open);
}

/// update を試み、失敗 (旧 schema の index 等で panic) したら full 再 index で自己修復。
/// bake 済みの .scip が残っていれば精度も維持して焼き直す。
/// `scan`: 走査方式 (UpdateScan)。明示 update は `Walk { trust_mtime: false }` = 全件 hash 照合。
/// `why`: 自動 update の理由 (quiet 時の 1 行要約に載せる。明示 update は "update")。
pub(crate) fn update_with_heal(db: Database, dir: &str, path: &str, scan: UpdateScan, why: &str) {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| update_inner(db, dir, scan, why)));
    std::panic::set_hook(prev);
    if r.is_err() {
        heal_full_reindex(dir, path, "増分 update 失敗 (旧 schema の index?)");
    }
}

/// db に対応する bake 済み SCIP の path (`<stem>.scip`)。存在は問わない。
pub(crate) fn scip_path_of(db_path: &str) -> String {
    format!("{}.scip", db_path.trim_end_matches(".db"))
}

/// db に対応する bake 済み .scip の sidecar。存在する時だけ Some。
pub(crate) fn scip_sidecar_of(db_path: &str) -> Option<String> {
    let scip = scip_path_of(db_path);
    Path::new(&scip).exists().then_some(scip)
}

/// full 再 index で自己修復。bake 済みの .scip が残っていれば再利用して精度も維持する。
/// index は派生物なので、鮮度を確認できない db は (root が分かる限り) 焼き直すのが正解 —
/// 警告だけ出して「定義が無い」と返すと、stdout を読む側には嘘になる。
pub(crate) fn heal_full_reindex(dir: &str, path: &str, why: &str) {
    let scip = scip_sidecar_of(path);
    eprintln!(
        "# {why} → full 再 index で自己修復{}",
        if scip.is_some() { " (.scip 再利用で精度維持)" } else { "" }
    );
    ensure_index(dir, path, scip.as_deref());
}

/// update の本体 (open 済み db を受け取る)。auto-update (maybe_auto_update) と run_update が共用。
pub(crate) fn update_inner(db: Database, dir: &str, scan: UpdateScan, why: &str) {
    // index 意味論の版が違えば増分は不整合 (旧値と新値が混ざる) → panic して
    // update_with_heal の full 再 index (.scip 再利用) に落とす。
    let mut built_at = 0u32; // 前回 index / update の開始時刻 (mtime 絞り込みの基準)
    if let Some(meta_t) = db.get_table("meta")
        && let Some(e) = meta_t.all().find().unwrap().into_iter().next() {
            let er = meta_t.entity(e);
            let v = match er.get("ver") {
                Some(Value::Number(n)) => n as u32,
                _ => 0,
            };
            if v != INDEX_VER {
                panic!("index ver {v} != {INDEX_VER} (意味論変更) → full 再 index が必要");
            }
            built_at = num(er.get("built_at"));
        }
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let impl_t = db.get_table("impl"); // 旧 index には無い (Option)
    let t = Instant::now();
    // 走査**開始**時刻を焼く (完了時刻ではない)。理由は run_index_inner の started_at と同じ — この
    // update 中に編集されたファイルを、次のクエリの mtime 判定で必ず拾い直せるようにする (#13)。
    let started_at = now_secs();

    // 1. 既存 file 表 (path → (eid, hash))。
    let mut prev: HashMap<String, (EntityId, u32)> = HashMap::new();
    for fe in file_t.all().find().unwrap() {
        let er = file_t.entity(fe);
        prev.insert(txt(er.get("path")), (fe, num(er.get("hash"))));
    }

    // 2. 現在の disk を走査 (path → src)。**mtime を信じられる時は、index 時刻より古い既知ファイルを
    //    読まない** (tokio 725 files で全件読み + hash が 25〜30ms、絞ると数 ms)。規則は walk_stats と
    //    同じ `>=` (秒粒度の等値は「変わったかも」側に倒す)。新規 path は必ず読む。
    //    mtime の巻き戻し (rsync -a / touch -t) は mtime 信頼中は見えないが、明示 `kenning update` と
    //    TRUST_MTIME_MAX_AGE 超の db は trust_mtime=false で全件 hash 照合するので、そこで拾う
    //    (maybe_auto_update の「念のため hash 照合」がこれ)。
    //    非 Rust テキストも同じ hash 差分に載せる (rust_files と text_files は .rs で排他)。
    //    `Stale(files)` は dir ゲート (fs_gate) が「file の増減は無く、この既知 file だけ mtime が新しい」と
    //    確定済みの経路 — walk すらせず、その file だけ読む (編集 → 最初の query の最短経路)。
    let mut cur: HashMap<String, Option<String>> = HashMap::new(); // None = 読まず「未変更」と判定
    let mut n_read = 0usize;
    let mut walked_dirs: Option<Vec<PathBuf>> = None;
    let read_src = |p: &Path| if lang_of(p) == LANG_RUST { std::fs::read_to_string(p).ok() } else { read_text_file(p) };
    match scan {
        UpdateScan::Stale(files) => {
            for p in prev.keys() {
                cur.insert(p.clone(), None);
            }
            for p in &files {
                // 読めなければ None のまま = 未変更扱い (dir 不変なので消えてはいないはず。安全側)
                if let Some(src) = read_src(p) {
                    n_read += 1;
                    cur.insert(p.to_string_lossy().into_owned(), Some(src));
                }
            }
        }
        UpdateScan::Walk { trust_mtime } => {
            let skip_old_known = trust_mtime && built_at > 0;
            let ws = walk_index_set(dir);
            for p in ws.rs.iter().chain(ws.text.iter()) {
                let key = p.to_string_lossy().into_owned();
                if skip_old_known && prev.contains_key(&key) && mtime_secs(p).is_some_and(|m| m < built_at) {
                    cur.insert(key, None);
                    continue;
                }
                if let Some(src) = read_src(p) {
                    n_read += 1;
                    cur.insert(key, Some(src));
                }
            }
            walked_dirs = Some(ws.dirs);
        }
    }
    // walk したなら dir 表を取り直す (変更なしでも: 空 dir の追加は file 表に映らないが、以後その dir 内の
    // 追加を dir ゲートで見張るには表に載っている必要がある)。
    if let (Some(dirs), Some(dir_t)) = (&walked_dirs, db.get_table("dir")) {
        for e in dir_t.all().find().unwrap() {
            dir_t.entity(e).delete().unwrap();
        }
        store_dirs(&dir_t, dirs);
    }

    // 2.5 walk が 1 件も拾えないのに db には行がある = root がずれている / 権限で読めない、の類。
    // ここで素通しすると「全ファイルが消えた」と解釈して db を丸ごと空にしてしまう (復旧は full
    // 再 index)。差分を捨てて警告に留める方が安全 (#13)。
    if cur.is_empty() && !prev.is_empty() {
        eprintln!("# ⚠ {dir} で index 対象ファイルが 0 件 (db には {} 件)。root がずれている疑いがあるため update を中止。", prev.len());
        eprintln!("# root を指定して: kenning index <repo> / 全部 ignore していないか `git check-ignore -v <file>` を確認。");
        return;
    }

    // 3. 分類。to_add = 変更 ∪ 新規 (再 index)、to_remove = 変更 ∪ 削除 (旧 facts 消去)。
    let mut to_add: Vec<(String, String)> = Vec::new();
    let mut to_remove: Vec<EntityId> = Vec::new();
    for (p, src) in &cur {
        match (prev.get(p), src) {
            (Some(_), None) => {} // mtime が index 時刻より古い既知ファイル = 読まずに未変更
            (Some((eid, h)), Some(src)) => {
                if *h != hash_u32(src) {
                    to_remove.push(*eid);
                    to_add.push((p.clone(), src.clone()));
                }
            }
            (None, Some(src)) => to_add.push((p.clone(), src.clone())),
            (None, None) => unreachable!("新規 path は必ず読む"),
        }
    }
    let mut n_deleted = 0u64;
    for (p, (eid, _)) in &prev {
        if !cur.contains_key(p) {
            to_remove.push(*eid);
            n_deleted += 1;
        }
    }

    if to_add.is_empty() && to_remove.is_empty() {
        let scan = t.elapsed();
        // built_at だけ再スタンプして返る。これをしないと mtime だけ変わったファイル (touch 等) が
        // 毎クエリ「古い」判定 → 空 update が永遠に走り続ける (実測で踏んだバグ)。
        if let Some(meta_t) = db.get_table("meta")
            && let Some(e) = meta_t.all().find().unwrap().into_iter().next() {
                let er = meta_t.entity(e);
                let (root_s, nfiles) = (txt(er.get("root")), num(er.get("nfiles")));
                let (baked, peak, upd) = (er.get("baked_at"), er.get("bake_peak_mb"), er.get("upd_since_bake"));
                meta_t.entity(e).delete().unwrap();
                let mut ins = meta_t.insert().set("root", root_s.as_str()).set("built_at", started_at).set("nfiles", nfiles).set("ver", INDEX_VER);
                if let Some(Value::Number(b)) = baked {
                    ins = ins.set("baked_at", b as u32);
                    if let Some(Value::Number(p)) = peak { ins = ins.set("bake_peak_mb", p as u32); }
                    if let Some(Value::Number(u)) = upd { ins = ins.set("upd_since_bake", u as u32); }
                }
                ins.commit().unwrap();
            }
        if !quiet() {
            eprintln!("変更なし ({} files 走査 / {n_read} 読込 {scan:?}、meta 再スタンプ {:?})。", cur.len(), t.elapsed() - scan);
        }
        return;
    }

    // 4. 変更/削除ファイルの旧 facts を消去 (影響 symbol 名を集める)。
    let mut affected: HashSet<String> = HashSet::new();
    for eid in &to_remove {
        purge_file(&sym_t, &call_t, &file_t, impl_t.as_ref(), *eid, &mut affected);
    }

    // 5. 追加/変更ファイルを parse して新 sym を挿入 + call-site を持ち越す。
    let mut acc = Acc::default();
    let mut n_skip = 0u64;
    for (p, src) in &to_add {
        // 非 Rust は syn に食わせない (parse 失敗を skip として数えてしまう) — file 行だけ差し替え。
        if lang_of(std::path::Path::new(p)) != LANG_RUST {
            index_text_file(&file_t, &mut acc, p, src);
            continue;
        }
        if !index_one_file(&file_t, &sym_t, &mut acc, dir, p, src) {
            n_skip += 1;
        }
    }
    for name in acc.defs.keys() {
        affected.insert(name.clone()); // 新規定義した名前も incoming 再解決の対象
    }

    // 6. 全 sym から global defs を再構築 (新旧すべて反映、再パース不要)。
    let defs = build_defs_from_table(&sym_t);

    // 7. incoming 再解決: 影響名を callee に持つ「既存 (=未変更ファイル) の call」を
    //    delete + 再挿入して callee_sym/res を最新化。この時点で存在する該当 call は
    //    未変更ファイル由来のみ (変更/削除は 4 で purge 済、新規は 8 で未挿入)。
    let mut reresolved = 0u64;
    for name in &affected {
        for ce in call_t.where_eq("callee", name.as_str()).find().unwrap() {
            let er = call_t.entity(ce);
            let Some(Value::Ref(caller)) = er.get("caller") else { continue };
            let Some(Value::Ref(file)) = er.get("file") else { continue };
            let qual_s = txt(er.get("qual"));
            let cs = CallSite {
                caller,
                caller_container: txt(sym_t.entity(caller).get("container")),
                file,
                rel_path: String::new(), // update は syn 再解決 (rel_path/col は使わない)
                name: name.clone(),
                qualifier: if qual_s.is_empty() { None } else { Some(qual_s) },
                is_method: num(er.get("is_method")) == 1,
                line: num(er.get("line")),
                col: 0,
            };
            call_t.entity(ce).delete().unwrap();
            insert_call(&call_t, &cs, &defs);
            reresolved += 1;
        }
    }

    // 8. 追加/変更ファイルの outgoing call を解決して挿入。
    let mut n_new_call = 0u64;
    for cs in &acc.pending {
        insert_call(&call_t, cs, &defs);
        n_new_call += 1;
    }

    // 8b. 追加/変更ファイルの impl edge を挿入 (旧 facts は 4 で purge 済)。
    if let Some(it) = &impl_t {
        for ie in &acc.impls {
            it.insert()
                .set("trait_name", ie.trait_name.as_str())
                .set("type_name", ie.type_name.as_str())
                .set("file", Value::Ref(ie.file))
                .set("line", ie.line)
                .commit()
                .unwrap();
        }
    }

    let n_changed = to_add.len() as u64;
    if quiet() {
        eprintln!("# {why} → 自動 update: {n_changed} 再 index / {n_deleted} 削除 ({:.1?})", t.elapsed());
    } else {
        eprintln!(
            "update: {} 再 index / {} 削除 / {} 未変更 ({:?}, {} parse-skip)",
            n_changed,
            n_deleted,
            cur.len() as u64 - n_changed,
            t.elapsed(),
            n_skip,
        );
        eprintln!(
            "  新規 outgoing call {} / incoming 再解決 {} / affected 名前 {}",
            n_new_call,
            reresolved,
            affected.len(),
        );
    }

    // meta を再スタンプ (built_at を現在に)。bake 情報は持ち越し + 変更数を積算 (閾値で bake 推奨)。
    // 旧 index (meta 表 / bake 列なし) は present なフィールドだけ扱い後方互換。
    if let Some(meta_t) = db.get_table("meta") {
        let root_abs = abs_dir(dir);
        let nfiles = file_t.all().count().unwrap() as u32;
        let old = meta_t.all().find().unwrap().into_iter().next();
        let (baked_at, peak_mb, upd_sb) = old
            .map(|e| {
                let er = meta_t.entity(e);
                (er.get("baked_at"), er.get("bake_peak_mb"), er.get("upd_since_bake"))
            })
            .unwrap_or((None, None, None));
        for e in meta_t.all().find().unwrap() {
            meta_t.entity(e).delete().unwrap(); // 更新は delete+reinsert (Null tie を避ける)
        }
        let mut ins = meta_t.insert().set("root", root_abs.as_str()).set("built_at", started_at).set("nfiles", nfiles).set("ver", INDEX_VER);
        if let Some(Value::Number(b)) = baked_at {
            let upd = match upd_sb { Some(Value::Number(u)) => u as u32, _ => 0 } + n_changed as u32;
            ins = ins.set("baked_at", b as u32).set("upd_since_bake", upd);
            if let Some(Value::Number(p)) = peak_mb {
                ins = ins.set("bake_peak_mb", p as u32);
            }
            if b > 0 && upd >= 20 {
                eprintln!("# SCIP facts が古くなってきた (bake 後 {upd} ファイル変更) → `kenning bake` 推奨");
            }
        }
        ins.commit().unwrap();
        drop(meta_t);
    }

    drop(file_t);
    drop(sym_t);
    drop(call_t);
    drop(impl_t);
    assert_no_faults(&db, "update"); // 拒否された write があれば heal (full 再 index) へ
    drop(db); // standalone: drop で schema + data を永続化。
    if !quiet() {
        eprintln!("\n次: `kenning def <name>` / `callers <name>` / `search kind:fn vis:pub`");
    }
}

/// `update <db>` — dir 省略時: meta の root を読んで再 index (自己記述 index の活用)。
pub fn run_update_from_db(path: &str) {
    let root = match Database::open_readonly(path) {
        Ok(db) => read_meta(&db).map(|(r, _)| r),
        Err(e) => {
            eprintln!("# index を開けない ({path}): {e}");
            eprintln!("# 先に: kenning index <dir> {path}");
            return;
        }
    };
    match root {
        Some(r) if !r.is_empty() => run_update(&r, path),
        _ => eprintln!("# この index に root 情報が無い (旧 index)。`kenning update <dir> {path}` で dir 指定を。"),
    }
}

// ═══════════════════════ 探索コマンド (Claude 向け、token 効率重視) ═══════════════════════
//
// 出力は `path:line<TAB>詳細` の 1 行 = そのまま Read に渡せる。装飾/計測なし、決定的順序。
// 目的は「grep 全マッチ + ファイル通読」を「精密な少数行」に圧縮すること。

/// 現在時刻 (unix 秒)。index 時刻の記録に使う。
pub(crate) fn now_secs() -> u32 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0)
}

/// 自己記述メタ (root, built_at) を読む。旧 index (meta 無し) なら None。
pub(crate) fn read_meta(db: &Database) -> Option<(String, u32)> {
    let meta_t = db.get_table("meta")?;
    let e = meta_t.all().find().ok()?.into_iter().next()?;
    let er = meta_t.entity(e);
    Some((txt(er.get("root")), num(er.get("built_at"))))
}

/// meta を「鮮度判定に使える形」で読む → Ok((root, built_at, ver)) / Err(確認できない理由)。
/// 理由は人間向けの一文 (警告にも heal のログにもそのまま使う)。
pub(crate) fn probe_meta(db: &Database) -> Result<(String, u32, u32), String> {
    let Some((root, built_at)) = read_meta(db) else {
        return Err("index に meta が無い (旧版)".into());
    };
    if root.is_empty() || built_at == 0 {
        return Err("index の root/built_at が空".into());
    }
    if !Path::new(&root).is_dir() {
        return Err(format!("index の root が無い ({root}、repo を移動/削除?)"));
    }
    let ver = db
        .get_table("meta")
        .and_then(|t| t.all().find().unwrap().into_iter().next().map(|e| match t.entity(e).get("ver") {
            Some(Value::Number(n)) => n as u32,
            _ => 0,
        }))
        .unwrap_or(0);
    Ok((root, built_at, ver))
}

/// 増分 update の走査方式。
pub(crate) enum UpdateScan {
    /// walk して全 file を突き合わせる。`trust_mtime` なら index 時刻より mtime が古い既知 file は読まない。
    Walk { trust_mtime: bool },
    /// dir ゲート (fs_gate) が「file の増減なし、この既知 file だけ mtime が新しい」と確定済み。walk しない。
    Stale(Vec<PathBuf>),
}

/// db が知っている file と dir (dir ゲートの材料)。dir 表が無い旧 index は None (walk に落ちる)。
pub(crate) struct Known {
    pub(crate) files: Vec<PathBuf>,
    pub(crate) dirs: Vec<PathBuf>,
}

pub(crate) fn load_known(db: &Database) -> Option<Known> {
    let dir_t = db.get_table("dir")?;
    let file_t = db.get_table("file")?;
    let paths = |t: &Table| -> Option<Vec<PathBuf>> {
        Some(t.all().find().ok()?.into_iter().map(|e| PathBuf::from(txt(t.entity(e).get("path")))).collect())
    };
    Some(Known { files: paths(&file_t)?, dirs: paths(&dir_t)? })
}

pub(crate) fn store_dirs(dir_t: &Table, dirs: &[PathBuf]) {
    for d in dirs {
        dir_t.insert().set("path", d.to_string_lossy().as_ref()).commit().unwrap();
    }
}

/// dir ゲートの結果。
pub(crate) enum Gate {
    /// 既知 dir (か、その .gitignore / .ignore) が index 時刻以降に変わった = file の増減があり得る → walk
    DirsChanged,
    /// dir は不変で、mtime が新しい既知 file も無い
    Fresh,
    /// dir は不変で、この既知 file だけ mtime が新しい (増減が無いので walk 不要)
    Stale(Vec<PathBuf>),
}

/// **walk せず** 既知の dir と file を stat するだけで鮮度を判定する。dir の mtime は直下 entry の
/// 追加 / 削除 / rename で動くので、全既知 dir が index 時刻より古ければ file の集合は不変 — あとは
/// 既知 file の mtime だけ見れば足りる (tokio: walk 2 回 18ms → stat ~830 件 2ms)。
/// 規則は walk_stats と同じ `>=`。stat できない (消えた) 物は「変わった」側に倒す。
/// `.gitignore` / `.ignore` の in-place 編集は dir の mtime を動かさないので個別に見る。
pub(crate) fn fs_gate(known: &Known, built_at: u32) -> Gate {
    const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];
    if known.files.is_empty() {
        return Gate::DirsChanged; // 0 件は walk 側の「root がずれている?」判定に任せる
    }
    for d in &known.dirs {
        if mtime_secs(d).is_none_or(|m| m >= built_at) {
            return Gate::DirsChanged;
        }
        if IGNORE_FILES.iter().any(|f| mtime_secs(&d.join(f)).is_some_and(|m| m >= built_at)) {
            return Gate::DirsChanged;
        }
    }
    let mut stale = Vec::new();
    for f in &known.files {
        match mtime_secs(f) {
            Some(m) if m < built_at => {}
            Some(_) => stale.push(f.clone()),
            None => return Gate::DirsChanged,
        }
    }
    if stale.is_empty() { Gate::Fresh } else { Gate::Stale(stale) }
}

/// root 以下の walk を stat だけで見る → (index 時刻より新しいファイル数, 走査できたファイル数)。
/// .rs と index 対象テキストの両方を見る — 片方だけだと md 編集が黙って古いまま残る。
/// **seen も返す**のが要点: stale 0 には「本当に最新」と「root がずれて 1 件も見えていない」の
/// 2 通りがあり、後者を「最新」と読むと黙って古い facts を返し続ける (#13)。
pub(crate) fn walk_stats(root: &str, built_at: u32) -> (usize, usize) {
    let mut stale = 0;
    let mut seen = 0;
    let ws = walk_index_set(root); // 1 回の walk で両方
    for p in ws.rs.iter().chain(ws.text.iter()) {
        seen += 1;
        // `>=` (`>` ではない): mtime も built_at も秒粒度なので、index 開始と同じ秒に入った編集は
        // `>` だと永久に見えない。等値も stale 側に倒す方が安全 — 空振りしても update 側が built_at
        // を「その update の開始時刻」で焼き直すので、余分な no-op update は 1 回で収束する。
        if mtime_secs(p).is_some_and(|m| m >= built_at) {
            stale += 1;
        }
    }
    (stale, seen)
}

/// ファイルの mtime (秒)。取れなければ None — 鮮度判定 (walk_stats) では「古くない」、増分 update の
/// 読み飛ばし判定では「読む」側に倒れる (どちらも安全側)。
pub(crate) fn mtime_secs(p: &std::path::Path) -> Option<u32> {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as u32)
}

/// index が焼かれてからの経過 (日)。警告文と「念のため update」の閾値に使う。
pub(crate) fn age_days(built_at: u32) -> u32 {
    now_secs().saturating_sub(built_at) / 86_400
}

/// mtime 判定を信じ切る上限。これを超えて古い db は、stale 0 に見えても一度 hash 差分を取り直す。
/// mtime は巻き戻る (git checkout / rsync / touch -t) ので、mtime だけだと取りこぼしが永続化する。
pub(crate) const TRUST_MTIME_MAX_AGE: u32 = 7 * 86_400;

/// index が古ければ stderr に警告 (stdout の path:line は汚さない)。KENNING_NO_STALE で無効化。
pub(crate) fn warn_if_stale(db: &Database, db_path: &str) {
    if std::env::var_os("KENNING_NO_STALE").is_some() || STALE_CHECKED.load(Ordering::Relaxed) {
        return; // auto 経路が同一プロセスで確認/更新済みなら walk を繰り返さない
    }
    let Some((root, built_at)) = read_meta(db) else {
        eprintln!("# ⚠ index に meta が無い (旧版) → 鮮度を確認できない。`kenning index <repo>` で焼き直しを。");
        return;
    };
    if root.is_empty() || built_at == 0 {
        eprintln!("# ⚠ index の root/built_at が空 → 鮮度を確認できない。`kenning index <repo>` で焼き直しを。");
        return;
    }
    let (stale, seen) = walk_stats(&root, built_at);
    if seen == 0 {
        eprintln!("# ⚠ {root} で index 対象ファイルが 0 件 → 鮮度を確認できない (repo を移動した / 全て ignore?)。");
    } else if stale > 0 {
        eprintln!("# ⚠ index が古い ({} 日前): {root} で {stale} ファイルが index 後に更新。`kenning update {db_path}` で最新に。", age_days(built_at));
    }
}
