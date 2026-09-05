//! full index の構築 (lock・容量見積り・作りかけの swap)。

use super::*;

// ─────────────────────────── index コマンド ───────────────────────────

/// index のエントリ。table の eid 予約はファイル数から見積もるが、symbol 密度が高い
/// プロジェクト (コンパイラ等) では過小になり枯渇し得る。その時は capacity を上げて自動リトライ
/// (tight-by-default で open を速く保ちつつ、外れ値でも落ちない)。
/// 書き終わりに enchudb の fault (容量満杯 / disk full で拒否された write) を検査する。
/// enchudb 0.23+ は entity 枠以外 (vocab / content / disk) の満杯を **panic でなく「拒否 + 計数」** にした。
/// 何もしないと欠けた index が黙って焼き上がるので、ここで panic して呼び側の capacity リトライ
/// (run_index の ×4) / 自己修復 (update_with_heal → full 再 index) に落とす。
pub(crate) fn assert_no_faults(db: &Database, phase: &str) {
    let eng = db.engine();
    let total = eng.fault_total();
    if total == 0 {
        return;
    }
    const KINDS: [FaultKind; 5] =
        [FaultKind::EntitySpace, FaultKind::ContentSpace, FaultKind::VocabSpace, FaultKind::ValueOutOfRange, FaultKind::DiskSpace];
    let detail: Vec<String> = KINDS
        .iter()
        .filter_map(|k| {
            let n = eng.fault_count(*k);
            (n > 0).then(|| format!("{}={n}", k.as_str()))
        })
        .collect();
    panic!("{phase}: enchudb が {total} 件の write を拒否 ({}) → 容量不足", detail.join(", "));
}

/// full index (明示 `kenning index` / bake 用: 常に焼く)。自動経路は [`ensure_index`]。
pub fn run_index(dir: &str, path: &str, scip_path: Option<&str>) {
    index_locked(dir, path, scip_path, false);
}

/// 自動経路 (heal / 「index が無い」) 用の full index。別 process が同じ db を焼いている最中なら
/// 完了を待ち、その成果がこの repo の現行版ならそれを再利用して戻る (戻り値 false)。
/// Claude は kenning を並列に叩くので、v10 移行直後などに同じ db の heal が 3 本同時に走る —
/// 直列化しないと互いの作りかけ directory を消し合い、1 本しか答えを返さなかった (再現済み)。
/// 直列化するだけだと 3 回 full index するので、待った側は焼き直さない。
pub(crate) fn ensure_index(dir: &str, path: &str, scip_path: Option<&str>) -> bool {
    index_locked(dir, path, scip_path, true)
}

/// 戻り値 = 実際に焼いたか (false: 他 process の成果を再利用 / 失敗)。
pub(crate) fn index_locked(dir: &str, path: &str, scip_path: Option<&str>, reuse_if_waited: bool) -> bool {
    let dir = &abs_dir(dir);
    let lock = match IndexLock::acquire(path) {
        Ok(l) => Some(l),
        // lock file を置けない (cache dir が read-only 等) なら db も置けないので実害は無いが、
        // 直列化なしで進む理由は言っておく。
        Err(e) => {
            eprintln!("# ⚠ index lock を取れない ({e}) → 直列化なしで続行");
            None
        }
    };
    if reuse_if_waited && lock.as_ref().is_some_and(|(_, waited)| *waited) && index_is_current(path, dir) {
        eprintln!("# 待った index がこの repo の現行版 → 再利用 (焼き直し省略)");
        return false;
    }
    sweep_leftovers(path); // lock を握っている = 他に作成中の process は居ない = 残骸と断定できる
    let tmp = tmp_db_path(path);
    let mut cap_mult = 1u32;
    loop {
        // 予約枯渇 (enchudb の unwrap 失敗) を捕まえるため panic を握りつぶして試行。
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_index_inner(dir, path, &tmp, scip_path, cap_mult)
        }));
        std::panic::set_hook(prev);
        // panic payload を人間可読に (握りつぶすと真因が消えるので必ず表示する)。
        let msg_of = |e: Box<dyn std::any::Any + Send>| -> String {
            e.downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "<non-string panic>".into())
        };
        match r {
            Ok(placed) => return placed,
            Err(e) if cap_mult < 64 => {
                cap_mult *= 4;
                eprintln!("# index 失敗 ({}) → capacity {cap_mult}x で再試行", msg_of(e));
            }
            Err(e) => {
                eprintln!("# index 失敗: capacity 64x でも解消せず ({dir}): {}", msg_of(e));
                wipe_db(&tmp); // 作りかけを残さない (本番 path は無傷のまま)
                return false;
            }
        }
    }
}

/// 同じ db への full index を process 間で直列化する flock (`<db>.index.lock`)。
/// kernel が process 終了で解放するので、BakeLock と違って pid 残骸の回収は要らない。
/// Drop で解放 (panic / exit いずれでも外れる)。
pub(crate) struct IndexLock {
    _held: std::fs::File, // 生きている間 flock を保持する (読まない)
}

impl IndexLock {
    /// 取れるまで待つ。`.1` = 他 process が同じ db を焼いていて完了を待ったか。
    pub(crate) fn acquire(db_path: &str) -> std::io::Result<(IndexLock, bool)> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(index_lock_path(db_path))?;
        match f.try_lock() {
            Ok(()) => Ok((IndexLock { _held: f }, false)),
            Err(std::fs::TryLockError::WouldBlock) => {
                eprintln!("# 別 process が同じ index を作成中 → 完了を待つ ({db_path})");
                f.lock()?;
                Ok((IndexLock { _held: f }, true))
            }
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }
}

pub(crate) fn index_lock_path(db_path: &str) -> String {
    format!("{db_path}.index.lock")
}

/// index を焼く作業場 (`<db>.tmp-<pid>`)。完成してから [`swap_in`] で本番 path に rename する。
/// 本番 path に直接焼かないのは、(1) reader が作りかけの db を開いて `get_table().unwrap()` で
/// panic しない、(2) 途中で crash しても旧 index が残る、ため。
pub(crate) fn tmp_db_path(path: &str) -> String {
    format!("{path}.tmp-{}", std::process::id())
}

/// 差し替えで退避した旧 index (`<db>.old-<pid>`)。差し替え直後に消す。
pub(crate) fn old_db_path(path: &str) -> String {
    format!("{path}.old-{}", std::process::id())
}

/// `path` の index が `dir` の現行版か (meta の root と INDEX_VER が一致)。mtime 鮮度は見ない —
/// 直前まで他 process が焼いていた物なので、以後の通常の stale 判定に任せる。
pub(crate) fn index_is_current(path: &str, dir: &str) -> bool {
    Database::open_readonly(path)
        .ok()
        .and_then(|db| probe_meta(&db).ok())
        .is_some_and(|(root, _, ver)| root == dir && ver == INDEX_VER)
}

/// crash した process が残した `<db>.tmp-<pid>` / `<db>.old-<pid>` を回収する。
/// IndexLock 下で呼ぶこと (他に作成中の process が居ないと言えるのは lock を握っている時だけ)。
pub(crate) fn sweep_leftovers(path: &str) {
    let p = Path::new(path);
    let (Some(parent), Some(name)) = (p.parent(), p.file_name().and_then(|n| n.to_str())) else { return };
    let parent = if parent.as_os_str().is_empty() { Path::new(".") } else { parent };
    let Ok(rd) = std::fs::read_dir(parent) else { return };
    let (tmp_prefix, old_prefix) = (format!("{name}.tmp-"), format!("{name}.old-"));
    for e in rd.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if n.starts_with(&tmp_prefix) || n.starts_with(&old_prefix) {
            eprintln!("# 前回の作りかけを回収: {}", e.path().display());
            wipe_db(&e.path().to_string_lossy());
        }
    }
}

/// 作り終えた `tmp` を本番 `path` に差し替える。旧 index を退避 → tmp を本番へ、の rename 2 段で
/// 本番 path に作りかけが存在する瞬間を作らない。旧 index (v10 directory / v9 単一ファイル +
/// 隣置き sidecar) は差し替え後に回収する。戻り値 = 配置できたか。
///
/// 残る窓: reader が query の最中 (enchudb の遅延 open が segment を path で開く前) に差し替わると
/// その 1 query は失敗し得る。open 時に fd を確保する enchudb 側の変更でしか閉じない。
pub(crate) fn swap_in(tmp: &str, path: &str) -> bool {
    let old = old_db_path(path);
    let _ = std::fs::rename(path, &old); // 初回は無いので Err (無視)
    if let Err(e) = std::fs::rename(tmp, path) {
        eprintln!("# ⚠ index を配置できない ({tmp} → {path}): {e} → 旧 index を戻す");
        let _ = std::fs::rename(&old, path);
        wipe_db(tmp);
        return false;
    }
    wipe_db(&old);
    remove_v9_sidecars(path);
    true
}

/// index 本体を消す。enchudb v10 の db は **directory** だが、旧 binary が残した v9 は
/// 「単一ファイル + 隣置き sidecar」なので両方剥がす (v9 の sidecar を残すと新 db の隣に
/// ゴミとして永久に居座る)。kenning 自身の sidecar (`.scip` / `.racfg.json` / `.time`) は消さない —
/// bake 済み facts は焼き直しで再利用する (heal_full_reindex の「.scip 再利用で精度維持」)。
pub(crate) fn wipe_db(path: &str) {
    let _ = std::fs::remove_dir_all(path); // v10: db は directory (中に sidecar も入る)
    let _ = std::fs::remove_file(path); // v9 以前: 単一ファイル
    remove_v9_sidecars(path);
}

pub(crate) fn remove_v9_sidecars(path: &str) {
    for ext in V9_SIDECAR_EXTS {
        let _ = std::fs::remove_file(format!("{path}.{ext}"));
    }
}

/// v9 以前が db の**隣**に置いていた enchudb sidecar の拡張子。v10 では db directory の
/// 中に入るので、v9 → v10 の焼き直し時に一度だけ回収する対象。
pub(crate) const V9_SIDECAR_EXTS: [&str; 7] = ["oplog", "lock", "schema", "tables", "eidmap", "vocabmap", "crc"];

/// `tmp` に焼いて最後に `path` へ差し替える。戻り値 = 配置できたか。
pub(crate) fn run_index_inner(dir: &str, path: &str, tmp: &str, scip_path: Option<&str>, cap_mult: u32) -> bool {
    wipe_db(tmp); // 前 loop (capacity 不足で panic) の作りかけ

    if !quiet() {
        eprintln!("=== kenning index: {} → {} ===", dir, path);
        if let Some(sp) = scip_path {
            eprintln!("(SCIP 正確解決: {})", sp);
        }
    }
    let t_all = Instant::now();

    // built_at は index の **開始**時刻 (完了時刻ではない)。完了時刻を焼くと、index 中に別プロセスが
    // 編集したファイルが「mtime < built_at」に収まり、以後の mtime ベース stale 判定を**永久に**
    // すり抜ける (#13: シンボルの同定は正しいのに行番号だけ 1 週間古い、の正体)。開始時刻なら
    // 最悪 1 回だけ余分な増分 update が走るだけで自己修復する。
    let started_at = now_secs();

    // eid 予約は実データ規模 (ファイル数) から見積もる。過剰予約はファイルを太らせ open を遅くする。
    // growable なので不足したら enchudb が伸ばす (build 時コストのみ)。ref は --scip 時のみ大きく。
    // cap_mult は枯渇リトライ時に上がる (1 → 4 → 16 → 64)。file 数は不変なので掛けない。
    let ws = walk_index_set(dir); // 1 回だけ walk (見積りと本走査で共用)
    let n_files_est = ws.rs.len().max(1) as u32;
    // file 表だけは非 Rust テキストも載る (sym/call 系の見積りは Rust ファイル数のまま)。
    let n_text_est = ws.text.len() as u32;
    // file 数だけの見積りは「少数の巨大 file」で外れる (ravn: 46 files / 160k 行 / 41,792 call で
    // 32,768 を枯渇 → 4x で焼き直し = full index 2 回)。.rs の総 byte でも見積もり、大きい方を取る。
    // 実測 (6 repo): call ≤ 10.4/KB、sym ≤ 1.3/KB、impl ≤ 0.3/KB → 余裕 2 倍強。
    let rs_kb = (ws.rs.iter().filter_map(|p| std::fs::metadata(p).ok()).map(|m| m.len()).sum::<u64>() / 1024) as u32;
    let file_cap = ((n_files_est + n_text_est) * 2).max(1_024);
    let dir_cap = (ws.dirs.len() as u32 * 2).max(256);
    let sym_cap = (n_files_est * 64).max(rs_kb * 3).max(8_192) * cap_mult; // enchudb 実測 ~17 sym/file、余裕 64
    let call_cap = (n_files_est * 400).max(rs_kb * 24).max(32_768) * cap_mult; // 実測 ~154 call/file、余裕 400
    let ref_cap = if scip_path.is_some() { (n_files_est * 400).max(rs_kb * 24).max(32_768) * cap_mult } else { 1_024 };
    let impl_cap = (n_files_est * 16).max(rs_kb).max(1_024) * cap_mult; // impl Trait for Type edge
    let extref_cap = if scip_path.is_some() { (n_files_est * 100).max(8_192) * cap_mult } else { 1_024 };
    let max_entities = (file_cap + dir_cap + sym_cap + call_cap + ref_cap + impl_cap + extref_cap) * 11 / 10; // +10% 余白
    let mut db = Database::create_growable_with_capacity(tmp, max_entities).unwrap();
    db.table("file")
        .tag("path")
        .tag("crate_")
        .number("lang")
        .number("loc")
        .number("hash") // 内容 fingerprint (増分 index の変更検知)
        .with_capacity(file_cap)
        .build()
        .unwrap();
    db.table("sym")
        .tag("name")
        .number("kind")
        .number("vis")
        .number("is_async")
        .number("is_test")
        .ref_to("file", "file")
        .tag("module")
        .tag("crate_")
        .tag("container")
        .tag("symbol") // SCIP グローバル一意 symbol (無指定なら "")。cross-file 同一性の鍵
        .tag("sig") // fn/method のシグネチャ 1 行 (hover 相当。他 kind は "")
        .tag("doc") // doc コメント 1 行目 (無ければ ""。def/outline で sig と並べて出す)
        .number("line")
        .number("end_line") // item 終端行 (`read` の切り出し下端。旧 index は 0 = 開始行のみ)
        .with_capacity(sym_cap)
        .build()
        .unwrap();
    db.table("call")
        .ref_to("caller", "sym")
        .tag("callee")            // 単純名 (text fallback / 表示用)
        .ref_to("callee_sym", "sym") // 解決先 sym (未解決なら未 set = Null)
        .number("res")            // 信頼度 (R_UNRESOLVED..R_AMBIG)
        .tag("qual")              // 修飾 (Type::/mod::。無ければ "")。増分の再解決用に永続化
        .number("is_method")      // x.f() 形式か。同上
        .ref_to("file", "file")
        .number("line")
        .with_capacity(call_cap)
        .build()
        .unwrap();
    // 全参照 edge (SCIP occurrence)。who-calls の上位互換 = find-all-refs。
    db.table("ref")
        .ref_to("symbol_sym", "sym") // 参照先の workspace sym
        .ref_to("file", "file")
        .number("line")
        .number("col")
        .number("role") // SCIP symbol_roles bit (1=def 2=import 4=write 8=read)
        .with_capacity(ref_cap)
        .build()
        .unwrap();
    // 外部 crate への参照 (SCIP occurrence のうち workspace 外・std 以外)。cross-repo refs の鍵:
    // SCIP symbol はグローバル一意 (crate+version) なので、別 repo の db の sym.symbol と突き合わせると
    // 「この repo が enchudb::X をどこで使うか」が repo を跨いで精密に出る (`across`)。
    db.table("extref")
        .tag("symbol") // 外部シンボルのグローバル一意文字列
        .ref_to("file", "file")
        .number("line")
        .number("col")
        .number("role")
        .with_capacity(extref_cap)
        .build()
        .unwrap();
    // impl Trait for Type edge (go-to-implementation)。file 別 = 増分 update で purge/再挿入。
    db.table("impl")
        .tag("trait_name")
        .tag("type_name")
        .ref_to("file", "file")
        .number("line")
        .with_capacity(impl_cap)
        .build()
        .unwrap();
    // 自己記述メタ (1 行): どの root をいつ index したか。staleness 警告と `update <db>` に使う。
    db.table("meta")
        .tag("root") // index した絶対 dir
        .number("built_at") // index 時刻 (unix 秒)
        .number("nfiles")
        .number("ver") // INDEX_VER (意味論の版。違えば増分せず full 再 index)
        .number("baked_at") // SCIP bake 時刻 (0 = 未 bake)。freshness 2層の SCIP 側
        .number("bake_peak_mb") // 前回 bake の peak RSS (次回ゲートの見積り)
        .number("upd_since_bake") // bake 後に増分 update したファイル数 (閾値で bake 推奨)
        .with_capacity(16)
        .build()
        .unwrap();
    // walk で通過した dir (root 含む)。鮮度判定の dir ゲート用 — dir の mtime は直下 entry の増減 /
    // rename で動くので、全既知 dir が index 時刻より古ければ file の集合は不変と言える (fs_gate)。
    db.table("dir").tag("path").with_capacity(dir_cap).build().unwrap();

    let file_t = db.get_table("file").unwrap();
    let dir_t = db.get_table("dir").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let ref_t = db.get_table("ref").unwrap();

    let mut n_files = 0u64;
    let mut n_skip = 0u64;
    let mut acc = Acc::default();
    // SCIP があれば pass1 の前にロード (sym の symbol タグ付けに使う)。
    if let Some(sp) = scip_path {
        acc.scip = Some(Scip::load(sp, dir));
    }
    let t = Instant::now();

    // ── pass1: 全ファイルを歩いて sym を挿入 + call-site を deferred 収集 ──
    //    parse は worker で並列 (facts の中間表現に落とす)、挿入は eid の採番順 = walk 順を保つため main で逐次。
    for batch in ws.rs.chunks(PARSE_BATCH) {
        for (p, facts) in batch.iter().zip(extract_parallel(batch)) {
            match facts {
                Some(facts) => {
                    insert_file_facts(&file_t, &sym_t, &mut acc, dir, &p.to_string_lossy(), facts);
                    n_files += 1;
                }
                None => n_skip += 1,
            }
        }
    }
    // ── pass1b: 非 Rust テキストを file 表に載せる (sym 無し = `text` の対象が増えるだけ) ──
    let mut n_text = 0u64;
    for p in &ws.text {
        let Some(src) = read_text_file(p) else { continue };
        index_text_file(&file_t, &mut acc, &p.to_string_lossy(), &src);
        n_text += 1;
    }
    store_dirs(&dir_t, &ws.dirs);
    let n_sym = sym_t.all().count().unwrap() as u64;
    let parse_el = t.elapsed();

    // ── pass2: callee を解決して call を挿入。SCIP があれば位置 join、無ければ syn ヒューリ ──
    let t2 = Instant::now();
    let n_call = acc.pending.len() as u64;
    let mut res_counts = [0u64; 4]; // [unresolved, unique, qualified, ambiguous]
    let mut scip_ws = 0u64; // SCIP occurrence が workspace 定義に確定
    let mut scip_external = 0u64; // SCIP は symbol を知ってるが自 index 外 (std/dep)
    let mut syn_recovered = 0u64; // SCIP 沈黙 (no-occ) を syn ヒューリで拾った数
    let diag = std::env::var_os("KENNING_DIAG_NOOCC").is_some();
    let mut d_nodoc = 0u64; // no-occ かつ SCIP に doc 自体が無い (test-helper/例外ファイル)
    let mut d_indoc = 0u64; // no-occ だが doc はある = 位置ズレ (macro/closure 等)
    let mut d_indoc_method = 0u64; // うち method 呼び (x.f())
    let mut d_samples: Vec<String> = Vec::new();
    for cs in &acc.pending {
        let (target, res) = if let Some(scip) = &acc.scip {
            match scip.symbol_at(&cs.rel_path, cs.line, cs.col) {
                Some(sym) => match acc.sym_by_symbol.get(sym) {
                    Some(&eid) => {
                        scip_ws += 1;
                        (Some(eid), R_UNIQUE) // workspace の定義に確定
                    }
                    None => {
                        scip_external += 1;
                        (None, R_UNRESOLVED) // std/dep/macro (SCIP は知ってるが自 index に無い)
                    }
                },
                // SCIP がこの位置に occurrence を出さない = 主に cfg 非活性コード
                // (syn は cfg-blind に全ブランチを parse するが RA は活性 cfg のみ解析) や
                // macro 生成領域。SCIP は沈黙なので syn ヒューリで best-effort 解決を試みる。
                None => {
                    if diag {
                        if scip.has_doc(&cs.rel_path) {
                            d_indoc += 1;
                            if cs.is_method {
                                d_indoc_method += 1;
                            }
                            if d_samples.len() < 25 {
                                d_samples.push(format!(
                                    "  {}:{}:{} {}{}{}",
                                    cs.rel_path, cs.line, cs.col,
                                    if cs.is_method { "." } else { "" },
                                    cs.name,
                                    cs.qualifier.as_ref().map(|q| format!(" (qual={})", q)).unwrap_or_default(),
                                ));
                            }
                        } else {
                            d_nodoc += 1;
                        }
                    }
                    let (t, r) = resolve_call(cs, &acc.defs);
                    if t.is_some() {
                        syn_recovered += 1;
                    }
                    (t, r)
                }
            }
        } else {
            resolve_call(cs, &acc.defs)
        };
        res_counts[res as usize] += 1;
        do_insert_call(&call_t, cs, target, res);
    }
    if diag {
        eprintln!(
            "[DIAG] no-occ 内訳: doc欠落={} / doc有り(位置ズレ)={} (うち method={}, path/fn={}) | syn 回収={}",
            d_nodoc, d_indoc, d_indoc_method, d_indoc - d_indoc_method, syn_recovered
        );
        eprintln!("[DIAG] doc有り no-occ サンプル:");
        for s in &d_samples {
            eprintln!("{}", s);
        }
    }
    let resolve_el = t2.elapsed();
    let resolved = res_counts[R_UNIQUE as usize] + res_counts[R_QUALIFIED as usize];

    let pct = if n_call > 0 { resolved as f64 * 100.0 / n_call as f64 } else { 0.0 };
    if !quiet() {
        eprintln!(
            "indexed: {} files (+{} text) / {} symbols / {} call-sites (parse {:?} + resolve {:?}, {} parse-skip)",
            n_files, n_text, n_sym, n_call, parse_el, resolve_el, n_skip
        );
        if acc.scip.is_some() {
            eprintln!(
                "resolve[SCIP]: {} / {} 解決 ({pct:.1}%) = SCIP確定 {} + syn回収 {} (cfg非活性/SCIP沈黙を best-effort) — external/std {} (SCIP識別), 未解決 {}",
                resolved,
                n_call,
                scip_ws,
                syn_recovered,
                scip_external,
                res_counts[R_UNRESOLVED as usize] - scip_external,
            );
        } else {
            eprintln!(
                "resolve[syn]: {} / {} call-sites 解決 ({pct:.1}%) — unique {} / qualified {} / ambiguous {} / external {}",
                resolved,
                n_call,
                res_counts[R_UNIQUE as usize],
                res_counts[R_QUALIFIED as usize],
                res_counts[R_AMBIG as usize],
                res_counts[R_UNRESOLVED as usize],
            );
        }
    }

    // ── ref-ingest: SCIP の全 occurrence を workspace sym への参照 edge にする (find-all-refs) ──
    if let Some(scip) = &acc.scip {
        let t3 = Instant::now();
        let extref_t = db.get_table("extref").unwrap();
        let mut n_ref = 0u64;
        let mut n_ext = 0u64;
        for o in &scip.occ {
            let Some(&file_eid) = acc.file_by_rel.get(&o.rel_path) else { continue };
            match acc.sym_by_symbol.get(&o.symbol) {
                Some(&sym_eid) => {
                    ref_t
                        .insert()
                        .set("symbol_sym", Value::Ref(sym_eid))
                        .set("file", Value::Ref(file_eid))
                        .set("line", o.line0 + 1) // 1-indexed に揃える
                        .set("col", o.col0)
                        .set("role", o.roles as u32)
                        .commit()
                        .unwrap();
                    n_ref += 1;
                }
                None => {
                    // 外部シンボル: std/toolchain と local は捨て、dep crate への参照だけ残す
                    // (cross-repo `across` の材料。std を入れると数十万行のノイズになる)。
                    if o.symbol.contains("https://github.com/rust-lang/rust/library") || o.symbol.starts_with("local ") {
                        continue;
                    }
                    extref_t
                        .insert()
                        .set("symbol", o.symbol.as_str())
                        .set("file", Value::Ref(file_eid))
                        .set("line", o.line0 + 1)
                        .set("col", o.col0)
                        .set("role", o.roles as u32)
                        .commit()
                        .unwrap();
                    n_ext += 1;
                }
            }
        }
        drop(extref_t);
        if !quiet() {
            eprintln!("ref: {} workspace 参照 + {} 外部 crate 参照 (extref) を edge 化 ({:?})", n_ref, n_ext, t3.elapsed());
        }
    }

    // ── impl edge を焼く (impl Trait for Type)。go-to-implementation 用。 ──
    let impl_t = db.get_table("impl").unwrap();
    for ie in &acc.impls {
        impl_t
            .insert()
            .set("trait_name", ie.trait_name.as_str())
            .set("type_name", ie.type_name.as_str())
            .set("file", Value::Ref(ie.file))
            .set("line", ie.line)
            .commit()
            .unwrap();
    }
    if !quiet() {
        eprintln!("impl: {} 個の impl Trait for Type edge", acc.impls.len());
    }
    drop(impl_t);

    // 自己記述メタを焼く (root は絶対パスに正規化して、cwd に依らず update/staleness を効かせる)。
    let meta_t = db.get_table("meta").unwrap();
    let root_abs = abs_dir(dir);
    let mut ins = meta_t
        .insert()
        .set("root", root_abs.as_str())
        .set("built_at", started_at) // index 開始時刻 (理由は started_at の定義箇所)
        .set("nfiles", n_files as u32)
        .set("ver", INDEX_VER);
    // SCIP を食ったならこの index は baked。時刻は .scip の mtime (= RA が facts を生成した時) —
    // heal/移行の再 index で bake スタンプが消えないように (bake コマンド自身は後で now に上書き)。
    if let Some(sp) = scip_path
        && let Some(t) = std::fs::metadata(sp)
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        {
            ins = ins.set("baked_at", t.as_secs() as u32).set("upd_since_bake", 0u32);
        }
    ins.commit().unwrap();

    drop(file_t);
    drop(sym_t);
    drop(call_t);
    drop(ref_t);
    drop(meta_t);
    assert_no_faults(&db, "index"); // 拒否された write があれば capacity ×4 で焼き直し
    let t_fin = Instant::now();
    // finish_with_oplog (consumer thread + 256MB oplog の concurrent 化) は要らない — index は single-writer で、
    // 以後は open (standalone) / open_readonly しか使わない。Drop が schema と sidecar を 1 回 persist する。
    drop(db);
    let fin_el = t_fin.elapsed();

    let t_swap = Instant::now();
    if !swap_in(tmp, path) {
        return false;
    }
    if quiet() {
        eprintln!("# full index 完了: {n_files} files (+{n_text} text) / {n_sym} symbols / 解決率 {pct:.1}% ({:.2?})", t_all.elapsed());
    } else {
        eprintln!(
            "(index 内訳: parse+挿入 {parse_el:?} / finish+drop {fin_el:?} / 配置 {:?} / 全体 {:?})",
            t_swap.elapsed(),
            t_all.elapsed()
        );
        eprintln!("db real disk: {:.1} MB", real_bytes(Path::new(path)) as f64 / 1_048_576.0);
        eprintln!("\n次: `kenning def <name>` / `callers <name>` / `search kind:fn vis:pub`");
    }
    true
}
