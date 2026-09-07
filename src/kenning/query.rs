//! 探索コマンド (def / read / find / search / callers / text / outline / stats) と出力整形。

use super::*;

/// index を read-only で開く。無い/壊れているときは backtrace でなく親切に落とす。
pub(crate) fn open_ro(db_path: &str) -> Option<Database> {
    match Database::open_readonly(db_path) {
        Ok(db) => {
            warn_if_stale(&db, db_path);
            Some(db)
        }
        Err(e) => {
            eprintln!("# index を開けない ({db_path}): {e}");
            eprintln!("# 先に: kenning index <dir> {db_path}");
            None
        }
    }
}

/// ファイル eid → path のキャッシュ (sym の所在解決に使う)。
pub(crate) fn file_paths(file_t: &Table) -> HashMap<EntityId, String> {
    let mut m = HashMap::new();
    for e in file_t.all().find().unwrap() {
        m.insert(e, txt(file_t.entity(e).get("path")));
    }
    m
}

/// sym 1 行を `path:line<TAB>vis [async] kind Qual::name  (crate) [#test]` に整形。
pub(crate) fn fmt_sym(sym_t: &Table, paths: &HashMap<EntityId, String>, eid: EntityId) -> String {
    let er = sym_t.entity(eid);
    let name = txt(er.get("name"));
    let container = txt(er.get("container"));
    let qual = if container.is_empty() { name } else { format!("{container}::{name}") };
    let kind = kind_name(num(er.get("kind")));
    let vis = vis_name(num(er.get("vis")));
    let asy = if num(er.get("is_async")) == 1 { "async " } else { "" };
    let tst = if num(er.get("is_test")) == 1 { "  #test" } else { "" };
    let crate_ = txt(er.get("crate_"));
    let line = num(er.get("line"));
    let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
    format!("{path}:{line}\t{vis} {asy}{kind} {qual}  ({crate_}){tst}")
}

/// sym 群を path:line 昇順で出力 (limit 超は件数を明示)。with_sig でシグネチャも (hover 相当)。
/// 表示用に source 1 行を整形 (trim + 長行 cap。行末の見切れは … で明示)。
pub(crate) const SRC_LINE_MAX: usize = 110;
pub(crate) fn trim_src(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() > SRC_LINE_MAX {
        format!("{}…", t.chars().take(SRC_LINE_MAX).collect::<String>())
    } else {
        t.to_string()
    }
}

/// query 時の source 行 lookup (best-effort 表示用)。file 単位で読んで cache。
/// index 時と path の前提が食い違う (相対 index で cwd が違う等) 場合は黙って "" —
/// path:line は既に出ているので、本文はあくまで「読みに行く手間を省く」おまけ。
pub(crate) struct SrcLines {
    files: HashMap<String, Option<Vec<String>>>,
}
impl SrcLines {
    fn new() -> Self {
        Self { files: HashMap::new() }
    }
    fn line(&mut self, path: &str, ln: u32) -> String {
        let lines = self.files.entry(path.to_string()).or_insert_with(|| {
            std::fs::read_to_string(path).ok().map(|s| s.lines().map(str::to_string).collect())
        });
        match lines {
            Some(v) if ln >= 1 => v.get(ln as usize - 1).map(|s| trim_src(s)).unwrap_or_default(),
            _ => String::new(),
        }
    }
}

/// "path:line\tin caller" 行に source 本文を付ける (取れなければそのまま)。
pub(crate) fn append_src(base: String, src: String) -> String {
    if src.is_empty() { base } else { format!("{base}\t{src}") }
}

/// 定義行 start (1-indexed) の直上に連なる doc コメント / attr 行まで上に広げた開始行を返す。
/// `read` が `#[derive(…)]` や `///` を定義本体と一緒に見せるため。
pub(crate) fn extend_up(lines: &[&str], start: usize) -> usize {
    let mut s = start;
    while s > 1 {
        let t = lines.get(s - 2).map(|l| l.trim_start()).unwrap_or("");
        if t.starts_with("///") || t.starts_with("#[") {
            s -= 1;
        } else {
            break;
        }
    }
    s
}

pub(crate) fn print_syms(sym_t: &Table, paths: &HashMap<EntityId, String>, eids: &[EntityId], limit: usize, with_sig: bool) {
    let mut rows: Vec<(String, u32, EntityId)> = eids
        .iter()
        .map(|&e| {
            let er = sym_t.entity(e);
            (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), e)
        })
        .collect();
    rows.sort();
    for (_, _, e) in rows.iter().take(limit) {
        let mut line = fmt_sym(sym_t, paths, *e);
        if with_sig {
            let er = sym_t.entity(*e);
            let sig = txt(er.get("sig"));
            if !sig.is_empty() {
                line = format!("{line}\t{sig}");
            }
            let doc = txt(er.get("doc"));
            if !doc.is_empty() {
                line = format!("{line}\t/// {doc}");
            }
        }
        println!("{line}");
    }
    if rows.len() > limit {
        println!("… (+{} 件省略、--limit {} で全部)", rows.len() - limit, rows.len());
    }
}

/// `--db <path>` / `--limit <n>` を抜き取り、残りを位置引数として返す。
/// db 未指定なら cwd の repo root から自動導出し、無ければ auto-index・古ければ auto-update まで
/// ここで済ませる (儀式ゼロ: クエリ側は db 管理を考えない)。KENNING_NO_AUTO=1 で魔法を全部止める。
pub(crate) struct Opts {
    pub(crate) db: String,
    pub(crate) limit: usize,
    pub(crate) pos: Vec<String>,
}
pub(crate) fn parse_opts(args: &[String]) -> Opts {
    let mut db = std::env::var("KENNING_DB").ok().filter(|s| !s.is_empty());
    let mut limit = 50usize;
    let mut pos = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--db" => { i += 1; if let Some(v) = args.get(i) { db = Some(v.clone()); } }
            "--limit" => { i += 1; if let Some(v) = args.get(i) { limit = v.parse().unwrap_or(50); } }
            // `-x` を黙って位置引数 (= 検索語 / 名前) にすると、typo した flag が「効いたように見えて
            // 効いていない」結果を返す (search は未知 facet を警告するのにここだけ素通りだった)。
            other if other.starts_with('-') && other.len() > 1 && !other[1..].starts_with(|c: char| c.is_ascii_digit()) => {
                eprintln!("# 無視: 未知 flag \"{other}\" (共通 --db/--limit、text -e/--and/--files、read --all)。語として検索するなら `text -e` で正規表現に");
            }
            other => pos.push(other.to_string()),
        }
        i += 1;
    }
    let no_auto = std::env::var_os("KENNING_NO_AUTO").is_some();
    // 明示 db (--db / env) はそのまま尊重。未指定なら cwd から導出。
    let (db, auto_root) = match db {
        Some(d) => (d, None),
        None => {
            let Some(root) = repo_root_of(".") else {
                eprintln!("# repo root が見つからない (cwd に .git / Cargo.toml の祖先なし)。");
                eprintln!("# repo 内で実行するか、--db <path> / env KENNING_DB で指定を。");
                std::process::exit(2);
            };
            let Some(d) = auto_db_path(&root) else {
                eprintln!("# ~/.cache/kenning を用意できない。--db <path> で指定を。");
                std::process::exit(2);
            };
            (d, Some(root))
        }
    };
    if !no_auto {
        AUTO_QUIET.store(true, Ordering::Relaxed); // query の前座: stderr は要点 1 行だけ
        // auto-index: 導出 db が無ければこの場で作る (root 既知の時のみ。明示 db は誤爆防止で作らない)。
        if !std::path::Path::new(&db).exists() {
            if let Some(root) = &auto_root {
                eprintln!("# index が無い → 自動 full index: {} → {}", root.display(), db);
                // bake 済み .scip が隣に残っていれば (cache prune は意図的に残す) 精度を引き継ぐ。
                ensure_index(&root.to_string_lossy(), &db, scip_sidecar_of(&db).as_deref());
                STALE_CHECKED.store(true, Ordering::Relaxed); // 今作ったばかり = 最新
            }
        } else {
            maybe_auto_update(&db, auto_root.as_deref()); // auto-update: 古ければ増分してから答える
        }
    }
    Opts { db, limit, pos }
}

/// index が古ければ増分 update してから返る。lock が取れなければ古いまま警告 (安全側)。
/// stat-walk のみなので通常コストは数 ms。KENNING_NO_STALE=1 でスキップ。
///
/// `auto_root` = db を cwd の repo root から導出した時のその root。これが分かっていれば
/// 鮮度を確認できない db (meta 無しの旧版 / root 不明) は警告でなく **full 再 index で自己修復**
/// する — db は root から一意に導出した派生物なので焼き直して困るものが無い。明示 db
/// (`--db` / env) は他 repo の db を誤って潰しかねないので従来通り警告のみ。
pub(crate) fn maybe_auto_update(db_path: &str, auto_root: Option<&Path>) {
    if std::env::var_os("KENNING_NO_STALE").is_some() {
        return;
    }
    // ここで鮮度は確認 (必要なら更新) される。lock 失敗時も警告は自前で出すので、
    // どの経路でも open_ro 側 warn_if_stale の再 walk は不要。
    STALE_CHECKED.store(true, Ordering::Relaxed);
    let probed = {
        let db = match Database::open_readonly(db_path) {
            Ok(db) => db,
            // 開けない db も (root が分かるなら) 焼き直す。probe_meta の Err と同じ理屈で、
            // db は root から一意に導出した派生物なので焼き直して困るものが無い。
            // ここに来る主役は **enchudb v10 で on-disk format が directory になったこと**:
            // 旧 binary が残した v9 の 1 ファイル db は open 段階で弾かれる (書込み lock を
            // 取る前に弾かれるので旧 db は 1 byte も変更されない = 旧 binary で開き直せる)。
            // 破損・書きかけも同じ扱いで良い。明示 db は他 repo のものを潰しかねないので警告のみ。
            Err(e) => {
                match auto_root {
                    Some(r) => heal_full_reindex(&r.to_string_lossy(), db_path, &format!("index を開けない ({e})")),
                    // 黙って返ると「鮮度を確認した上で最新」と区別が付かない (#13)。
                    None => eprintln!("# ⚠ index を開けないので鮮度を確認できない ({db_path}: {e}) → 古い結果の可能性。"),
                }
                return;
            }
        };
        probe_meta(&db).map(|m| (m, load_known(&db)))
    }; // ← readonly を閉じてから書込 open する
    let ((root, built_at, ver), known) = match probed {
        Ok(v) => v,
        Err(why) => {
            match auto_root {
                Some(r) => heal_full_reindex(&r.to_string_lossy(), db_path, &why),
                None => eprintln!("# ⚠ {why} → 鮮度を確認できない。`kenning index <repo>` で焼き直しを。"),
            }
            return;
        }
    };
    if ver != INDEX_VER {
        // 版違い: ファイル無変更でも full 再 index (増分は旧値と新値が混ざる)。open せず直接 heal —
        // update 経路に流すと「増分 update 失敗 (旧 schema?)」の紛らわしい 2 行目が出る。
        heal_full_reindex(&root, db_path, &format!("index の版が古い (v{ver} → v{INDEX_VER})"));
        return;
    }
    // mtime は巻き戻る (git checkout / rsync / touch -t) ので、若い db だけ mtime を信じ、古い db は
    // 一度 hash 差分で答え合わせする (update 側が全ファイル読んで比較する)。
    let young = age_days(built_at) * 86_400 < TRUST_MTIME_MAX_AGE;
    // dir ゲート: 既知 dir / file の stat だけで判定し、walk は dir に増減があった時だけ (update 側で回す。
    // 削除だけの変更も walk で拾う)。dir 表の無い旧 index は INDEX_VER 違いで上の heal に落ちている。
    let (why_old, scan) = match known.as_ref().map(|k| fs_gate(k, built_at)) {
        Some(Gate::Fresh) if young => return,
        Some(Gate::Fresh) => ("index が古い (mtime 上は最新だが念のため hash 照合)".to_string(), UpdateScan::Walk { trust_mtime: false }),
        Some(Gate::Stale(files)) if young => (format!("{} file が編集済み", files.len()), UpdateScan::Stale(files)),
        Some(Gate::Stale(_)) => ("index が古い (念のため全件 hash 照合)".to_string(), UpdateScan::Walk { trust_mtime: false }),
        Some(Gate::DirsChanged) | None => ("dir に増減あり".to_string(), UpdateScan::Walk { trust_mtime: young }),
    };
    match Database::open(db_path) {
        Ok(db) => {
            if !quiet() {
                eprintln!("# {why_old} → 自動増分 update ({root})");
            }
            update_with_heal(db, &root, db_path, scan, &why_old); // 旧 schema なら full 再 index で自己修復
        }
        Err(e) => {
            eprintln!("# ⚠ index が {} 日前だが lock を取れない ({e}) → 古い結果で回答。後で `kenning update` を。", age_days(built_at));
        }
    }
}

/// `def <name>` — 名前の定義位置。grep の全マッチではなく sym の等値一撃。
pub fn cmd_def(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning def <name> [--db P] [--limit N]");
        return;
    };
    run_search(&o.db, &[format!("name:{name}")], o.limit, true); // def = hover 相当で sig も
}

/// `read` — 定義本体をそのまま出す (`def` → Read の 2 手を 1 手に)。3 つの形:
/// - `read <name> [container] [crate:X] [path:S] [--all]` — 同名が複数なら container / crate / path で絞る
///   か `--all` で全部出す (同名 symbol で grep に戻る最大の原因だったので、絞り方を増やした)
/// - `read <path>:<line>` — その行を囲む item の本体 (`grep -n "fn X"` → `sed -n` の代わり)。
///   非 Rust ならその行を含む見出し / [table] 配下
/// - `read <path>:<from>-<to>` — 行範囲 (`sed -n 'A,Bp'` の代わり)。跨ぐ定義 / 見出しを頭に列挙する
/// - `read <file.md>#<見出し>` — 見出し配下 (toml は `[table]`、yaml はキー)。CHANGELOG を awk で切る代わり
///
/// Read tool と違い「その item の範囲だけ」なので token も節約。範囲は index 済みの
/// line..end_line + 直上の doc/attr 行 (query 時に上へ拡張)。
pub fn cmd_read(args: &[String]) {
    let all = args.iter().any(|a| a == "--all");
    let rest: Vec<String> = args.iter().filter(|a| *a != "--all").cloned().collect();
    let o = parse_opts(&rest);
    let Some(first) = o.pos.first() else {
        eprintln!("usage: kenning read <name> [container] [crate:X] [path:S] [--all] | read <path>:<line> | read <path>:<from>-<to> | read <file>#<見出し>  [--db P]");
        return;
    };
    if let Some((p, a, b)) = split_path_range(first) {
        run_read_range(&o.db, p, a, b);
        return;
    }
    if let Some((p, l)) = split_path_line(first) {
        run_read_at(&o.db, p, l);
        return;
    }
    if let Some((p, h)) = first.split_once('#')
        && looks_like_path(p)
    {
        run_read_section(&o.db, p, h);
        return;
    }
    // `read <path>` (行も見出しも無し) = file 全体は Read tool の仕事。「定義が無い」+ 近い symbol 名を
    // 返すと嘘になるので、outline を出して `:<line>` / `#<見出し>` へ誘導する。
    if looks_like_path(first) {
        eprintln!("# {first} は file → outline を出す (本体は read <path>:<line> / read <file>#<見出し>、全文は Read)");
        run_outline(&o.db, first, o.limit);
        return;
    }
    let (mut container, mut crate_f, mut path_f) = (None, None, None);
    for a in &o.pos[1..] {
        match a.split_once(':') {
            Some(("crate", v)) => crate_f = Some(v),
            Some(("path", v)) => path_f = Some(v),
            Some(("container", v)) => container = Some(v),
            _ if container.is_none() => container = Some(a.as_str()),
            _ => eprintln!("# 無視: {a} (container は 1 つ、絞るなら crate: / path:)"),
        }
    }
    run_read(&o.db, first, container, crate_f, path_f, all, o.limit);
}

/// `<path>:<from>-<to>` 形か。範囲 read (`sed -n 'A,Bp'` の代わり) の入口。
/// file 名に `-` があっても壊れない (`:` の後ろだけを見る)。逆順で書かれたら黙って直す。
pub(crate) fn split_path_range(s: &str) -> Option<(&str, usize, usize)> {
    let (p, r) = s.rsplit_once(':')?;
    let (a, b) = r.split_once('-')?;
    let n = |v: &str| v.parse::<usize>().ok().filter(|&n| n > 0);
    let (a, b) = (n(a)?, n(b)?);
    looks_like_path(p).then_some((p, a.min(b), a.max(b)))
}

/// `<path>:<line>` 形か (末尾が数字で、前半が path らしい)。symbol 名に `:` は無いので衝突しない。
pub(crate) fn split_path_line(s: &str) -> Option<(&str, usize)> {
    let (p, l) = s.rsplit_once(':')?;
    let line = l.parse::<usize>().ok().filter(|&n| n > 0)?;
    looks_like_path(p).then_some((p, line))
}

pub(crate) fn looks_like_path(s: &str) -> bool {
    !s.is_empty() && (s.contains('/') || s.contains('.'))
}

/// file の解決 (outline / read 共通): 完全一致 → 末尾一致 (`src/lib.rs` で index の絶対 path に当たる)。
pub(crate) fn find_file(file_t: &Table, paths: &HashMap<EntityId, String>, arg: &str) -> Option<EntityId> {
    match file_t.where_eq("path", arg).find_one().unwrap() {
        Some(e) => Some(e),
        None => {
            let mut hits: Vec<(&String, EntityId)> = paths.iter().filter(|(_, p)| p.ends_with(arg)).map(|(e, p)| (p, *e)).collect();
            hits.sort();
            hits.first().map(|(_, e)| *e)
        }
    }
}

/// sym の本体を `path:line<TAB>詳細` の見出し + 行番号付きで出す (read の中核)。
pub(crate) fn print_sym_body(sym_t: &Table, paths: &HashMap<EntityId, String>, eid: EntityId) {
    let er = sym_t.entity(eid);
    let path = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
    let start = num(er.get("line")) as usize;
    let end = (num(er.get("end_line")) as usize).max(start);
    println!("{}", fmt_sym(sym_t, paths, eid));
    let Ok(src) = std::fs::read_to_string(&path) else {
        println!("# source を読めない: {path} (index 時と cwd が違う? full path で index を)");
        return;
    };
    let lines: Vec<&str> = src.lines().collect();
    let s = extend_up(&lines, start);
    print_lines(&path, &lines, s, end);
}

/// lines[s..=e] (1-indexed) を行番号付きで出す。長大なら READ_MAX_LINES で打ち切って続きの Read を案内。
pub(crate) fn print_lines(path: &str, lines: &[&str], s: usize, e: usize) {
    let e = e.min(lines.len());
    let shown_end = e.min(s + READ_MAX_LINES - 1);
    for (i, l) in lines.iter().enumerate().take(shown_end).skip(s.saturating_sub(1)) {
        println!("{:>5}\t{l}", i + 1);
    }
    if shown_end < e {
        // 続きも kenning で読める (範囲 read があるので Read tool に投げ直さなくていい)。
        println!("# … 長大なので {READ_MAX_LINES} 行で打ち切り (+{} 行)。続き: `read {path}:{}-{e}`", e - shown_end, shown_end + 1);
    }
}

/// `read <path>:<from>-<to>` — 行範囲をそのまま (`sed -n 'A,Bp'` の代わり)。
/// 隣接する複数 item をまとめて見たい時に `read <path>:<line>` (1 item) では足りないので置いた。
/// ただの切り出しなら sed と同じなので、**範囲が跨ぐ定義 / 見出しを頭に列挙**する
/// (「今どこを読んでいるか」が 1 行で分かる = 追いの outline を 1 回消す)。
pub(crate) fn run_read_range(db_path: &str, path_arg: &str, from: usize, to: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);
    let Some(fe) = find_file(&file_t, &paths, path_arg) else {
        println!("# file not found: {path_arg}");
        return;
    };
    let path = paths[&fe].clone();
    let lang = num(file_t.entity(fe).get("lang"));
    let Ok(src) = std::fs::read_to_string(&path) else {
        println!("# source を読めない: {path}");
        return;
    };
    let lines: Vec<&str> = src.lines().collect();
    if from > lines.len() {
        println!("# {path}:{from}-{to} は file の外 ({} 行しかない)", lines.len());
        return;
    }
    let to = to.min(lines.len());
    // 範囲が触れる物の列挙: Rust は定義 (行が重なる sym)、非 Rust は見出し (範囲内で始まる物 + 頭を囲む物)。
    let covered: Vec<String> = if lang == LANG_RUST {
        let mut v: Vec<(usize, String)> = sym_t
            .all()
            .where_ref("file", fe)
            .find()
            .unwrap()
            .into_iter()
            .filter_map(|e| {
                let er = sym_t.entity(e);
                let (s0, e0) = (num(er.get("line")) as usize, num(er.get("end_line")) as usize);
                (s0 <= to && from <= e0.max(s0)).then(|| (s0, sym_qual(&sym_t, e)))
            })
            .filter(|(_, q)| !q.is_empty())
            .collect();
        v.sort();
        v.into_iter().map(|(_, q)| q).collect()
    } else {
        let cs = text_containers(lang, &src);
        let head = cs.partition_point(|(l, _)| (*l as usize) <= from);
        cs.iter()
            .enumerate()
            .filter(|(i, (l, _))| (*i + 1 == head) || ((*l as usize) > from && (*l as usize) <= to))
            .map(|(_, (_, name))| name.clone())
            .collect()
    };
    let n = to + 1 - from;
    let what = match covered.len() {
        0 => String::new(),
        k if k <= 6 => format!(" — {}", covered.join(", ")),
        k => format!(" — {}, …+{}", covered[..6].join(", "), k - 6),
    };
    println!("{path}:{from}	{n} 行 ({from}-{to}){what}");
    print_lines(&path, &lines, from, to);
}

/// `read <path>:<line>` — その行を囲む item (Rust) / 見出し配下 (非 Rust)。囲む物が無ければ前後 20 行。
pub(crate) fn run_read_at(db_path: &str, path_arg: &str, line: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);
    let Some(fe) = find_file(&file_t, &paths, path_arg) else {
        println!("# file not found: {path_arg}");
        return;
    };
    let path = paths[&fe].clone();
    let lang = num(file_t.entity(fe).get("lang"));
    if lang == LANG_RUST {
        // 囲む item = line <= L <= end_line を満たす中で開始行が最大のもの (impl 内 method なら method)
        let best = sym_t
            .all()
            .where_ref("file", fe)
            .find()
            .unwrap()
            .into_iter()
            .filter(|&e| {
                let er = sym_t.entity(e);
                let (s, en) = (num(er.get("line")) as usize, num(er.get("end_line")) as usize);
                s <= line && line <= en.max(s)
            })
            .max_by_key(|&e| num(sym_t.entity(e).get("line")));
        if let Some(e) = best {
            print_sym_body(&sym_t, &paths, e);
            return;
        }
    }
    let Ok(src) = std::fs::read_to_string(&path) else {
        println!("# source を読めない: {path}");
        return;
    };
    let lines: Vec<&str> = src.lines().collect();
    if lang != LANG_RUST {
        let containers = text_containers(lang, &src);
        let idx = containers.partition_point(|(l, _)| (*l as usize) <= line);
        if idx > 0 {
            let (s, e) = section_range(&containers, idx - 1, lang, lines.len());
            println!("{path}:{s}\t(in {})", containers[idx - 1].1);
            print_lines(&path, &lines, s, e);
            return;
        }
    }
    println!("# {path}:{line} を囲む定義が無い → 前後 20 行");
    print_lines(&path, &lines, line.saturating_sub(20).max(1), line + 20);
}

/// `read <file>#<見出し>` — 見出し (toml は [table]、yaml はキー) 配下を出す。部分一致・大小無視。複数なら一覧。
pub(crate) fn run_read_section(db_path: &str, path_arg: &str, heading: &str) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let paths = file_paths(&file_t);
    let Some(fe) = find_file(&file_t, &paths, path_arg) else {
        println!("# file not found: {path_arg}");
        return;
    };
    let path = paths[&fe].clone();
    let lang = num(file_t.entity(fe).get("lang"));
    let Ok(src) = std::fs::read_to_string(&path) else {
        println!("# source を読めない: {path}");
        return;
    };
    let containers = text_containers(lang, &src);
    if containers.is_empty() {
        println!("# {path} には見出し構造が無い (md の #, toml の [table], yaml のキーのみ対応)。`read {path_arg}:<line>` で位置指定を。");
        return;
    }
    // 一致は「その見出し自身の題」で見る (階層全体だと親の題が子にも含まれて全部当たる)。
    let h = heading.to_lowercase();
    let hits: Vec<usize> = (0..containers.len()).filter(|&i| section_title(&containers[i].1, lang).to_lowercase().contains(&h)).collect();
    match hits.as_slice() {
        [] => {
            println!("# \"{heading}\" に一致する見出しが {path} に無い。ある見出し:");
            for (l, c) in containers.iter().take(50) {
                println!("{path}:{l}\t{c}");
            }
        }
        [i] => {
            let lines: Vec<&str> = src.lines().collect();
            let (s, e) = section_range(&containers, *i, lang, lines.len());
            println!("{path}:{s}\t{}", containers[*i].1);
            print_lines(&path, &lines, s, e);
        }
        many => {
            println!("# \"{heading}\" は {} 箇所。絞るか `read {path_arg}:<line>` で:", many.len());
            for &i in many {
                println!("{path}:{}\t{}", containers[i].0, containers[i].1);
            }
        }
    }
}

/// 階層文字列から見出し自身の題を取り出す (md: `A > B > C` の C、yaml: `a.b.c` の c、toml: そのまま)。
pub(crate) fn section_title(c: &str, lang: u32) -> &str {
    match lang {
        LANG_MD => c.rsplit(" > ").next().unwrap_or(c),
        LANG_YAML => c.rsplit('.').next().unwrap_or(c),
        _ => c,
    }
}

/// containers[i] の配下の行範囲 (1-indexed, 両端含む): 次の「同じか浅い深さ」の見出しの直前まで。
pub(crate) fn section_range(containers: &[(u32, String)], i: usize, lang: u32, n_lines: usize) -> (usize, usize) {
    let depth = |c: &str| match lang {
        LANG_MD => c.matches(" > ").count(),
        LANG_YAML => c.matches('.').count(),
        _ => 0,
    };
    let d = depth(&containers[i].1);
    let end = containers[i + 1..].iter().find(|(_, c)| depth(c) <= d).map(|(l, _)| *l as usize - 1).unwrap_or(n_lines);
    (containers[i].0 as usize, end.max(containers[i].0 as usize))
}

/// 一度に出す本体の上限行数。超える item は頭からここまで + 続きの Read 案内 (暴発防止)。
pub(crate) const READ_MAX_LINES: usize = 400;

pub(crate) fn run_read(db_path: &str, name: &str, container: Option<&str>, crate_f: Option<&str>, path_f: Option<&str>, all: bool, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);

    let mut defs = defs_of(&sym_t, name, container);
    if let Some(c) = crate_f {
        defs.retain(|&e| txt(sym_t.entity(e).get("crate_")) == c);
    }
    if let Some(p) = path_f {
        defs.retain(|&e| paths.get(&ref_of(sym_t.entity(e).get("file"))).is_some_and(|f| f.contains(p)));
    }
    if defs.is_empty() {
        let filt: Vec<String> = [container.map(|c| format!("container={c}")), crate_f.map(|c| format!("crate={c}")), path_f.map(|p| format!("path~{p}"))]
            .into_iter()
            .flatten()
            .collect();
        println!("# \"{name}\" の定義が index に無い{}。", if filt.is_empty() { String::new() } else { format!(" ({})", filt.join(", ")) });
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    if defs.len() > 1 && !all {
        println!(
            "# \"{name}\" は {} 定義 (同名)。絞る: `read {name} <container>` / `crate:<crate>` / `path:<path の一部>`、全部出すなら `--all`:",
            defs.len()
        );
        print_syms(&sym_t, &paths, &defs, limit, true);
        return;
    }
    for (i, &e) in defs.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print_sym_body(&sym_t, &paths, e);
    }
}

/// `find <substr>` — symbol 名の部分一致 (大文字小文字無視)。exact な `def` の補完 = 名前の発見用。
/// ファイル名 (basename) の部分一致も併せて返す (`find . -name '*X*'` 相当。symbol 軸だけだと
/// 「あのファイルどこ」に答えられず ls/find に落ちていた)。
/// tag 等値では引けないので sym 全走査 (数千件なので µs–ms)。
pub fn cmd_find(args: &[String]) {
    let o = parse_opts(args);
    let Some(needle) = o.pos.first() else {
        eprintln!("usage: kenning find <substr> [--db P] [--limit N]");
        return;
    };
    let needle = needle.to_lowercase();
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);
    let hits: Vec<EntityId> = sym_t
        .all()
        .find()
        .unwrap()
        .into_iter()
        .filter(|&e| txt(sym_t.entity(e).get("name")).to_lowercase().contains(&needle))
        .collect();
    println!("# {} symbols  [name ~ \"{needle}\"]", hits.len());
    print_syms(&sym_t, &paths, &hits, o.limit, false);
    print_file_hits(&file_t, &paths, &needle, o.limit);
}

/// `find` のファイル名一致部分 — basename 一致を `path<TAB>L loc` で (outline <dir> と同じ形)。
pub(crate) fn print_file_hits(file_t: &Table, paths: &HashMap<EntityId, String>, needle: &str, limit: usize) {
    let mut fhits: Vec<(&String, EntityId)> = paths
        .iter()
        .filter(|(_, p)| p.rsplit('/').next().unwrap_or(p).to_lowercase().contains(needle))
        .map(|(e, p)| (p, *e))
        .collect();
    if fhits.is_empty() {
        return;
    }
    fhits.sort();
    println!("# {} files  [file name ~ \"{needle}\"]", fhits.len());
    for (p, e) in fhits.iter().take(limit) {
        println!("{p}\t{} loc", num(file_t.entity(*e).get("loc")));
    }
    if fhits.len() > limit {
        println!("… (+{} 件省略、--limit で全部)", fhits.len() - limit);
    }
}

/// `search kind:fn vis:pub crate:… container:… async:1 …` — faceted 等値 AND。
pub fn cmd_search(args: &[String]) {
    let lexical = !args.iter().any(|a| a == "--no-lexical");
    let o = parse_opts(&args.iter().filter(|a| *a != "--no-lexical").cloned().collect::<Vec<_>>());
    if o.pos.is_empty() {
        eprintln!("usage: kenning search <facet...> [--no-lexical]  例: kind:method vis:pub calls:unwrap");
        eprintln!("  facet: name: kind:(fn|method|struct|enum|trait|const) vis:(pub|crate|restricted|priv)");
        eprintln!("         async:(0|1) test:(0|1) traitimpl:(0|1) crate: container: module: path:(部分一致)");
        eprintln!("         attr:<substr>  属性の部分一致 (deprecated / allow(dead_code) / serde / cfg(...))");
        eprintln!("         reachable:(0|1)  live root (pub/test/trait 実装/main/item マクロ) からの到達可能性");
        eprintln!("         callers:<n> namecalls:<n>  (被呼び出し数。`callers:0 namecalls:0` = 未使用候補)");
        eprintln!("         calls:<name>  (本体で <name> を呼ぶ sym に絞る = grep 不可の edge×facet AND)");
        return;
    }
    run_search_opt(&o.db, &o.pos, o.limit, false, lexical);
}

pub(crate) fn run_search(db_path: &str, facets: &[String], limit: usize, with_sig: bool) {
    run_search_opt(db_path, facets, limit, with_sig, true)
}

/// `lexical` = 未使用候補を字句照合で裏取りするか (`--no-lexical` で切る)。
pub(crate) fn run_search_opt(db_path: &str, facets: &[String], limit: usize, with_sig: bool, lexical: bool) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let mut q = sym_t.all();
    let mut applied: Vec<String> = Vec::new();
    let mut calls_filters: Vec<String> = Vec::new(); // calls:X = 本体で X を呼ぶ sym に絞る (edge facet)
    // 被呼び出し数の facet。`callers:0 namecalls:0` = 誰からも呼ばれていない = 未使用候補を 1 手で。
    let (mut want_callers, mut want_namecalls): (Option<usize>, Option<usize>) = (None, None);
    let mut want_reachable: Option<bool> = None;
    let mut path_filters: Vec<String> = Vec::new(); // path:X = 定義 file の部分一致 (複数は OR)
    let mut attr_filters: Vec<String> = Vec::new(); // attr:X = 属性文字列の部分一致 (複数は AND)
    for f in facets {
        let Some((k, v)) = f.split_once(':') else {
            eprintln!("# 無視: \"{f}\" (key:value 形式で)");
            continue;
        };
        match k {
            // name:Type::method も受ける (def の入口。container facet が別に来ていればそちらが優先)。
            "name" => {
                let (n, c) = split_qualified(v);
                q = q.where_eq("name", n);
                applied.push(format!("name={n}"));
                if let Some(c) = c.filter(|_| !facets.iter().any(|f| f.starts_with("container:"))) {
                    q = q.where_eq("container", c);
                    applied.push(format!("container={c}"));
                }
            }
            "kind" => match kind_code(v) {
                Some(c) => { q = q.where_eq("kind", c); applied.push(format!("kind={v}")); }
                None => eprintln!("# 無視: 未知 kind \"{v}\" (fn/method/struct/enum/trait/const)"),
            },
            "vis" => match vis_code(v) {
                Some(c) => { q = q.where_eq("vis", c); applied.push(format!("vis={v}")); }
                None => eprintln!("# 無視: 未知 vis \"{v}\" (pub/crate/restricted/priv)"),
            },
            "async" => { q = q.where_eq("is_async", bool01(v)); applied.push(format!("async={v}")); }
            "test" => { q = q.where_eq("is_test", bool01(v)); applied.push(format!("test={v}")); }
            // trait 実装の method は trait 経由で呼ばれるので `callers:0` が構造的に真になる。
            // `traitimpl:0` で外すと「本当に誰も使っていない候補」だけが残る。
            "traitimpl" => { q = q.where_eq("trait_impl", bool01(v)); applied.push(format!("traitimpl={v}")); }
            // 属性の部分一致 (`attr:deprecated` / `attr:allow(dead_code)` / `attr:serde`)。
            // 特定属性を特別扱いしないので、廃止予定 API の利用調査にも dead 判定の裏取りにも効く。
            "attr" => { attr_filters.push(v.to_lowercase()); applied.push(format!("attr~{v}")); }
            // 到達可能性 (live root からの前向き伝播)。`reachable:0` = 消せる候補の本命。
            // 入次数 0 (`callers:0`) と違い、鎖や相互再帰で繋がった dead な塊も 1 パスで出る。
            "reachable" => { want_reachable = Some(bool01(v) == 1); applied.push(format!("reachable={v}")); }
            "crate" => { q = q.where_eq("crate_", v); applied.push(format!("crate={v}")); }
            "container" => { q = q.where_eq("container", v); applied.push(format!("container={v}")); }
            "module" => { q = q.where_eq("module", v); applied.push(format!("module={v}")); }
            "calls" => { calls_filters.push(v.to_string()); applied.push(format!("calls={v}")); }
            // path: は text / read と同じ「file path の部分一致」。ここだけ無いと
            // 「graph.rs の pub fn」が 1 手で出せず grep + search の 2 手になっていた。
            "path" | "file" => { path_filters.push(v.to_string()); applied.push(format!("path={v}")); }
            // 逆向きの edge facet: 「何本の呼び出しに指されているか」。
            // callers = 確実 (callee_sym 逆引き、誤りなし) / namecalls = 名前一致 (未解決の候補込み)。
            // 両方 0 = 名前ごと誰も呼んでいない → 消せる候補。callers だけ 0 は「未解決の呼び出しかも」。
            "callers" | "namecalls" => match v.parse::<usize>() {
                Ok(n) => {
                    if k == "callers" { want_callers = Some(n) } else { want_namecalls = Some(n) }
                    applied.push(format!("{k}={n}"));
                }
                Err(_) => eprintln!("# 無視: {k}:\"{v}\" は数値で (例: {k}:0)"),
            },
            _ => eprintln!("# 無視: 未知 facet key \"{k}\" (name/kind/vis/async/test/crate/container/module/calls/path)"),
        }
    }
    let mut hits = q.find().unwrap();
    for f in &attr_filters {
        hits.retain(|&e| txt(sym_t.entity(e).get("attrs")).to_lowercase().contains(f.as_str()));
    }
    if !path_filters.is_empty() {
        hits.retain(|&e| {
            let p = paths.get(&ref_of(sym_t.entity(e).get("file"))).cloned().unwrap_or_default();
            path_filters.iter().any(|f| p.contains(f.as_str()))
        });
    }
    let mut dead_set: HashSet<EntityId> = HashSet::new();
    if let Some(want) = want_reachable {
        dead_set = dead_by_elimination(&call_t, &sym_t);
        hits.retain(|e| dead_set.contains(e) != want);
    }
    // 被呼び出し数で絞る。列は自動 index 済みなので 1 sym あたり等値 count の 2 発で済む
    // (全 call を舐めない)。267 sym の repo で数 ms。
    if want_callers.is_some() || want_namecalls.is_some() {
        hits.retain(|&e| {
            if let Some(n) = want_callers
                && call_t.where_eq("callee_sym", Value::Ref(e)).count().unwrap_or(0) != n
            {
                return false;
            }
            if let Some(n) = want_namecalls {
                let name = txt(sym_t.entity(e).get("name"));
                if call_t.where_eq("callee", name.as_str()).count().unwrap_or(0) != n {
                    return false;
                }
            }
            true
        });
    }
    // 「未使用候補」(callers:0 かつ namecalls:0) は **消す判断**に使われるので、call 表だけを根拠に
    // しない。定義以外に名前が 1 度でも字句として現れる物は落とす — parse できない DSL マクロや
    // 展開後に名前が合成される経路など、call 表に載らない使われ方をここで拾う (安全側に倒す)。
    // 候補は数十件なので、全名前を 1 本の regex に畳んで corpus を 1 回走査するだけで済む。
    let unused_query = want_reachable == Some(false) || (want_callers == Some(0) && want_namecalls == Some(0));
    if lexical && unused_query && !hits.is_empty() {
        let before = hits.len();
        let used = names_used_lexically(&paths, &sym_t, &hits, &dead_set);
        hits.retain(|e| !used.contains(&txt(sym_t.entity(*e).get("name"))));
        let dropped = before - hits.len();
        if dropped > 0 {
            applied.push(format!("字句照合で -{dropped}"));
            println!("# 字句照合で {dropped} 件除外 (定義以外に名前が出現 — コメント/文字列も含むので安全側に倒している)。除外分も見るなら `--no-lexical`");
        }
    }
    // edge facet: 本体が X を呼ぶ sym だけ残す (grep には表現できない sym facet × call edge の AND)。
    for cn in &calls_filters {
        let callers: HashSet<EntityId> = call_t
            .where_eq("callee", cn.as_str())
            .find()
            .unwrap()
            .into_iter()
            .map(|c| ref_of(call_t.entity(c).get("caller")))
            .collect();
        hits.retain(|e| callers.contains(e));
    }
    println!("# {} symbols  [{}]", hits.len(), applied.join(" "));
    if want_callers == Some(0) || want_namecalls == Some(0) || want_reachable == Some(false) {
        println!("# 呼ばれていない = この index の中での話。pub は外部 crate から、trait impl の method は動的に呼ばれ得る (test:0 で #[test] を除ける)");
        println!("# const / struct / enum は呼び出し edge を持たないので常に 0 → `kind:fn` / `kind:method` と併用する");
    }
    // name 等値で 0 件 = typo の可能性 → callers と同じ救済 (`def` の主な失敗はこれ)。
    // 他 facet で 0 になった場合は名前自体は在るので黙る (嘘の「無い」を出さない)。
    if hits.is_empty()
        && let Some(n) = facets.iter().find_map(|f| f.strip_prefix("name:"))
        && sym_t.where_eq("name", n).count().unwrap() == 0
    {
        suggest_similar(&sym_t, &paths, n);
    }
    print_syms(&sym_t, &paths, &hits, limit, with_sig);
}

/// `callers <name> [container]` — 精密 who-calls (名前解決した callee_sym の eid 逆引き)。
pub fn cmd_callers(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning callers <name> [container] [--db P] [--limit N]");
        return;
    };
    let container = o.pos.get(1).map(String::as_str);
    run_callers(&o.db, name, container, o.limit);
}

pub(crate) fn run_callers(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    // 修飾名で来た時は call 側 (callee は bare name) を bare で数える。
    let bare = split_qualified(name).0;
    let name_total = call_t.where_eq("callee", bare).count().unwrap();
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。名前一致の call = {name_total} 件 (外部/未解決)");
        suggest_similar(&sym_t, &paths, name);
        return;
    }

    // 同名定義が多いと caller を全部出すと冗長。container 指定が無く定義が複数なら
    // まず「定義ごとの精密 caller 数」の要約表だけ出して、絞り方を促す。
    if container.is_none() && defs.len() > 1 {
        let mut rows: Vec<(usize, EntityId)> = defs
            .iter()
            .map(|&d| (call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap(), d))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0)); // caller 数 降順
        let precise_sum: usize = rows.iter().map(|(n, _)| n).sum();
        println!("# \"{name}\" は {} 型が定義 (同名)。定義ごとの精密 caller 数:", defs.len());
        for (n, d) in rows.iter().take(limit) {
            let er = sym_t.entity(*d);
            let ct = txt(er.get("container"));
            let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
            println!("  {n:>5}  {}::{name}  ({})  {path}:{}", if ct.is_empty() { "·".into() } else { ct }, txt(er.get("crate_")), num(er.get("line")));
        }
        let unresolved = name_total.saturating_sub(precise_sum);
        println!("# 絞る: `callers {name} <container>` (例: {}) — 未確定の候補も位置付きで出る", txt(sym_t.entity(rows[0].1).get("container")));
        if unresolved > 0 {
            println!("# 名前一致 {name_total} 件中 {precise_sum} 件を確定。残り {unresolved} 件は未確定(候補、drill-in で位置表示)。");
        }
        return;
    }

    // 名前一致する全 call を 1 度引く (確実/候補の切り分けに使う)。
    let name_matches = call_t.where_eq("callee", name).find().unwrap();
    // callee ident をラベル整形するクロージャ (caller sym → "Container::name")。
    let caller_label = |c: EntityId| -> (String, u32, String) {
        let er = call_t.entity(c);
        let p = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
        let ln = num(er.get("line"));
        // caller 未 set = 関数の外 (item 直下のマクロ引数など)。空欄で出すと読めないので明示。
        let cq = sym_qual(&sym_t, ref_of(er.get("caller")));
        (p, ln, cq)
    };

    let mut src = SrcLines::new(); // 呼び出し行の本文 (「どう呼ばれてるか」を read-back なしで)
    let mut precise_sum = 0usize;
    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let calls = call_t.where_eq("callee_sym", Value::Ref(d)).find().unwrap();
        precise_sum += calls.len();
        let mut rows: Vec<(String, u32, String)> = calls.iter().map(|&c| caller_label(c)).collect();
        rows.sort();
        println!("  ← {} 確実 callers (callee_sym 逆引き、誤りなし):", rows.len());
        for (p, ln, cq) in rows.iter().take(limit) {
            println!("{}", append_src(format!("    {p}:{ln}\tin {cq}"), src.line(p, *ln)));
        }
        if rows.len() > limit {
            println!("    … (+{} 件省略)", rows.len() - limit);
        }
    }

    // ── 候補 (completeness backstop): 名前一致だが callee_sym 未 set (型推論待ち/cfg 非活性/
    //    外部同名)。確実集合の「見逃し」がここに全部いる = grep superset の未確認部分。これを
    //    出すことで Claude は「本当に全 caller を掴んだか」を grep に戻らず目視できる。
    let mut cand: Vec<(String, u32, String, u32)> = name_matches
        .iter()
        .copied()
        .filter(|&c| !matches!(call_t.entity(c).get("callee_sym"), Some(Value::Ref(_))))
        .map(|c| {
            let (p, ln, cq) = caller_label(c);
            (p, ln, cq, num(call_t.entity(c).get("res")))
        })
        .collect();
    cand.sort();
    let other = name_total.saturating_sub(precise_sum).saturating_sub(cand.len());
    if !cand.is_empty() {
        println!(
            "  ⚠ {} 候補 (名前一致だが未確定 — 要確認、確実集合の漏れはここに全部):",
            cand.len()
        );
        for (p, ln, cq, res) in cand.iter().take(limit) {
            let base = format!("    {p}:{ln}\tin {cq}  [{}]", RES_NAMES.get(*res as usize).unwrap_or(&"?"));
            println!("{}", append_src(base, src.line(p, *ln)));
        }
        if cand.len() > limit {
            println!("    … (+{} 件省略、--limit で全部)", cand.len() - limit);
        }
    }
    println!(
        "# 名前一致 {name_total} = 確実 {precise_sum} + 候補未確定 {} + 別の同名 sym に確定 {other}",
        cand.len()
    );
    // 0 件で終わる trait 実装 method は「使われていない」ではなく **trait 経由で呼ばれる** だけ。
    // 何も言わずに 0 を返すと、呼び出し側を grep で探し回るか、消してよいと誤読される
    // (enchudb では 17 method がこの形)。
    if name_total == 0 && other == 0 {
        for &d in &defs {
            if num(sym_t.entity(d).get("trait_impl")) != 1 {
                continue;
            }
            let ct = txt(sym_t.entity(d).get("container"));
            let traits = traits_implemented_at(&db, &sym_t, d);
            let list = if traits.is_empty() { "?".to_string() } else { traits.join(" / ") };
            println!("# {ct}::{bare} は trait 実装 → 呼び出しは trait 経由なので名前が現れない (0 件 ≠ 未使用)。");
            println!("# この file が {ct} に実装している trait: {list}");
            match traits.first() {
                Some(t) => println!("# 次: `callers {bare}` (同名 method 全体) / `impls {t}` (兄弟実装)"),
                None => println!("# 次: `callers {bare}` (同名 method 全体)"),
            }
            break;
        }
    }
}

/// `def` が属する impl block の trait 名 (同 file・同 型 の impl edge から引く)。
/// 行範囲は持っていないので file + type_name の一致まで。複数あれば全部返す (嘘をつかない)。
pub(crate) fn traits_implemented_at(db: &Database, sym_t: &Table, def: EntityId) -> Vec<String> {
    let Some(impl_t) = db.get_table("impl") else { return Vec::new() };
    let er = sym_t.entity(def);
    let (file, ct) = (ref_of(er.get("file")), txt(er.get("container")));
    let mut v: Vec<String> = impl_t
        .where_eq("type_name", ct.as_str())
        .find()
        .unwrap_or_default()
        .into_iter()
        .filter(|&e| ref_of(impl_t.entity(e).get("file")) == file)
        .map(|e| txt(impl_t.entity(e).get("trait_name")))
        .filter(|t| !t.is_empty())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// `edges` — 解決済み call edge をファイル間で集計して一括出力。
/// 行形式: `caller_path<TAB>callee_path<TAB>count`（cross-file のみ、path 昇順、stdout 純 TSV）。
/// モジュール/パッケージ単位の依存グラフを外部ツール (可視化 GUI 等) が組むための素材で、
/// per-symbol の callers/callees と違い repo 全体を 1 回で吐く。
pub fn cmd_edges(args: &[String]) {
    let o = parse_opts(args);
    run_edges(&o.db);
}

pub(crate) fn run_edges(db_path: &str) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    // callee 側は「定義があるファイル」= sym.file を引く
    let mut sym_file: HashMap<EntityId, EntityId> = HashMap::new();
    for e in sym_t.all().find().unwrap() {
        sym_file.insert(e, ref_of(sym_t.entity(e).get("file")));
    }

    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    let mut total = 0usize;
    for c in call_t.all().find().unwrap() {
        let er = call_t.entity(c);
        // callee_sym 未 set = 外部 crate / 未解決 → 依存グラフの素材にならないので除外
        let Some(Value::Ref(target)) = er.get("callee_sym") else { continue };
        let from_f = ref_of(er.get("file"));
        let Some(&to_f) = sym_file.get(&target) else { continue };
        if from_f == to_f {
            continue; // 同一ファイル内呼び出しはファイル間依存ではない
        }
        let (Some(fp), Some(tp)) = (paths.get(&from_f), paths.get(&to_f)) else { continue };
        *counts.entry((fp.clone(), tp.clone())).or_default() += 1;
        total += 1;
    }

    let mut rows: Vec<((String, String), usize)> = counts.into_iter().collect();
    rows.sort();
    for ((f, t), n) in &rows {
        println!("{f}\t{t}\t{n}");
    }
    eprintln!("# {} file-pair edges ({total} resolved cross-file calls)", rows.len());
}

/// `callees <name> [container]` — X が呼ぶ先 (outgoing calls、`callers` の鏡)。
/// rust-analyzer の callHierarchy/outgoingCalls 相当。「この fn は何に依存するか」。
pub fn cmd_callees(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning callees <name> [container] [--db P] [--limit N]");
        return;
    };
    run_callees(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

pub(crate) fn run_callees(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);
    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。");
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let calls = call_t.where_eq("caller", Value::Ref(d)).find().unwrap();
        // callee_sym が set = workspace 定義に確定、未 set = 外部/std/型推論待ち。
        let mut resolved: HashMap<EntityId, usize> = HashMap::new();
        let mut external: HashMap<String, usize> = HashMap::new();
        for &c in &calls {
            match call_t.entity(c).get("callee_sym") {
                Some(Value::Ref(t)) => *resolved.entry(t).or_default() += 1,
                _ => *external.entry(txt(call_t.entity(c).get("callee"))).or_default() += 1,
            }
        }
        let mut rows: Vec<(String, u32, EntityId, usize)> = resolved
            .iter()
            .map(|(&t, &n)| {
                let er = sym_t.entity(t);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), t, n)
            })
            .collect();
        rows.sort();
        println!("  → {} 確定 callees (呼ぶ先 workspace 定義、path:line = 定義位置):", rows.len());
        for (p, ln, t, n) in rows.iter().take(limit) {
            println!("    {p}:{ln}\t→ {}  (×{n})", sym_qual(&sym_t, *t));
        }
        if rows.len() > limit {
            println!("    … (+{} 件省略)", rows.len() - limit);
        }
        if !external.is_empty() {
            let mut ext: Vec<(&String, &usize)> = external.iter().collect();
            ext.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0))); // 呼び回数 降順
            let shown: Vec<String> = ext.iter().take(12).map(|(n, c)| format!("{n}(×{c})")).collect();
            println!(
                "  → 外部/未解決 {} 種 (std/dep/型推論待ち): {}{}",
                external.len(), shown.join(", "), if external.len() > 12 { " …" } else { "" }
            );
        }
    }
}

/// `impls <name>` — go-to-implementation。name が trait なら実装型、型なら実装 trait を出す。
pub fn cmd_impls(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning impls <trait|type> [--db P] [--limit N]");
        return;
    };
    run_impls(&o.db, name, o.limit);
}

pub(crate) fn run_impls(db_path: &str, name: &str, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let Some(impl_t) = db.get_table("impl") else {
        println!("# impl 情報が無い (旧 index)。`kenning index <dir>` で再 index すると使える。");
        return;
    };
    let paths = file_paths(&file_t);
    // path:line<TAB>相手名 で列挙するヘルパ (col = 出す相手の列名)。
    let dump = |eids: &[EntityId], col: &str| {
        let mut rows: Vec<(String, u32, String)> = eids
            .iter()
            .map(|&e| {
                let er = impl_t.entity(e);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), txt(er.get(col)))
            })
            .collect();
        rows.sort();
        for (p, ln, n) in rows.iter().take(limit) {
            println!("  {p}:{ln}\t{n}");
        }
        if rows.len() > limit {
            println!("  … (+{} 件省略、--limit で全部)", rows.len() - limit);
        }
    };
    let as_trait = impl_t.where_eq("trait_name", name).find().unwrap();
    let as_type = impl_t.where_eq("type_name", name).find().unwrap();
    if as_trait.is_empty() && as_type.is_empty() {
        // 「名前自体が無い」と「型は在るが trait 実装が無い (inherent impl だけ)」は別物。
        // 混ぜると、実在する型を調べに来た Claude が grep に戻る。
        let sym_t = db.get_table("sym").unwrap();
        let defs = sym_t.where_eq("name", name).find().unwrap();
        if defs.is_empty() {
            println!("# \"{name}\" という定義が index に無い。");
            suggest_similar(&sym_t, &paths, name);
        } else {
            println!("# \"{name}\" は定義済みだが trait 実装が無い (inherent impl のみ、or impl が外部・cfg 非活性)。");
            println!("{}", fmt_sym(&sym_t, &paths, defs[0]));
            println!("# メソッド一覧は `search container:{name}`、使われ方は `callers {name}` / `refs {name}`。");
        }
        return;
    }
    if !as_trait.is_empty() {
        println!("# trait \"{name}\" を実装する型 ({}):", as_trait.len());
        dump(&as_trait, "type_name");
    }
    if !as_type.is_empty() {
        println!("# 型 \"{name}\" が実装する trait ({}):", as_type.len());
        dump(&as_type, "trait_name");
    }
}

/// `text <term>... [-e] [--and] [--files] [path:S]` — 全文検索 (コメント/文字列も) + **enclosing symbol 注釈**。
/// grep superset with structure: どの関数の中のヒットかが 1 行で分かる = 追い Read を 1 個消す。
/// 検索対象は index 済みファイル (live に読む = 常に最新)。大小無視。
/// 複数語は既定 OR (`grep -E "a|b"`)、`--and` で全語を含む行だけ (`grep X | grep Y` の代わり)、
/// `-e` で各語を正規表現として扱う (`grep -E` 相当)、`--files` で file 別件数だけ (`rg -c` の代わり
/// = 広い語の triage)、`path:<substr>` で対象ファイルを絞る (`rg <pat> <dir>`)。
/// これらが無いと Claude が grep に戻る。末尾に `# N 件 / M files` を必ず出す。
pub fn cmd_text(args: &[String]) {
    let has = |f: &str| args.iter().any(|a| a == f);
    let (regex, and, files_only) = (has("-e") || has("--regex"), has("--and"), has("--files"));
    let flags = ["-e", "--regex", "--and", "--files"];
    let rest: Vec<String> = args.iter().filter(|a| !flags.contains(&a.as_str())).cloned().collect();
    let o = parse_opts(&rest);
    let (path_fs, terms): (Vec<&String>, Vec<&String>) = o.pos.iter().partition(|a| a.starts_with("path:"));
    let path_fs: Vec<&str> = path_fs.iter().map(|a| &a["path:".len()..]).filter(|f| !f.is_empty()).collect();
    let terms: Vec<String> = terms.into_iter().cloned().collect();
    if terms.is_empty() {
        eprintln!("usage: kenning text <term>... [-e] [--and] [--files] [path:<substr>] [--db P] [--limit N]");
        eprintln!("  複数語は既定 OR、--and で全語を含む行だけ。-e で正規表現 (大小無視、区別するなら (?-i)Foo)。");
        eprintln!("  --files で file 別件数だけ (広い語の triage)。path: で対象ファイルを絞る (複数は OR)");
        return;
    }
    let needle = terms.join(if and { " & " } else { " | " });
    let Some(m) = TextMatcher::new(&terms, regex, and) else { return };
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();

    // file eid → その file の sym (line 昇順)。enclosing = hit 行以前で最後の定義 (end 無しの近似)。
    let mut syms_by_file: HashMap<EntityId, Vec<(u32, EntityId)>> = HashMap::new();
    for s in sym_t.all().find().unwrap() {
        let er = sym_t.entity(s);
        syms_by_file.entry(ref_of(er.get("file"))).or_default().push((num(er.get("line")), s));
    }
    for v in syms_by_file.values_mut() {
        v.sort();
    }

    let mut files: Vec<(String, EntityId, u32)> = file_t
        .all()
        .find()
        .unwrap()
        .into_iter()
        .map(|e| {
            let er = file_t.entity(e);
            (txt(er.get("path")), e, num(er.get("lang")))
        })
        .filter(|(p, _, _)| path_fs.is_empty() || path_fs.iter().any(|f| p.contains(f)))
        .collect();
    files.sort();
    let mut shown = 0usize;
    let mut total = 0usize;
    let mut n_files = 0usize;
    let mut per_file: Vec<(usize, String)> = Vec::new(); // --files 用 (件数, path)
    // 読みが支配的 (tokio 770 files で逐次 11ms) なので thread で撒き、hit した file の本文だけ持ち帰る。
    // 注釈と出力は main thread で path 順 (決定的)。
    let srcs = par_map(&files, |(path, _, _)| std::fs::read_to_string(path).ok().filter(|s| m.hit(s)));
    for ((path, fe, lang), src) in files.iter().zip(srcs) {
        let Some(src) = src else { continue }; // 読めない or file 単位で不一致
        // --files: 行注釈も本文整形も要らないので、数えて次の file へ (広い語ほど効く)。
        if files_only {
            let n = src.lines().filter(|l| m.hit(l)).count();
            if n > 0 {
                n_files += 1;
                total += n;
                per_file.push((n, path.clone()));
            }
            continue;
        }
        // file 単位の hit は AND だと「全語がどこかに在る」= 行 AND の必要条件でしかないので、
        // 実際に行が当たった file だけ数える (足切り通過数を files に出すと嘘になる)。
        let mut file_hits = 0usize;
        let syms = syms_by_file.get(fe);
        // 非 Rust は sym を持たないので、本文から container を作る (見出し階層 / [table] / キーパス)。
        let containers = if *lang == LANG_RUST { Vec::new() } else { text_containers(*lang, &src) };
        for (i, line) in src.lines().enumerate() {
            if !m.hit(line) {
                continue;
            }
            total += 1;
            file_hits += 1;
            if shown >= o.limit {
                continue; // 件数は数え続ける
            }
            shown += 1;
            let ln = (i + 1) as u32;
            let text = line.trim();
            // enclosing symbol: 通常は「line <= ln の最後の定義」の中。`///` は次の定義の doc、
            // `//!` はモジュール doc (どの item のものでもない)。end 無しの近似。
            let is_mod_doc = *lang == LANG_RUST && text.starts_with("//!");
            let is_doc = text.starts_with("///");
            let encl = if *lang != LANG_RUST {
                let idx = containers.partition_point(|(l, _)| *l <= ln);
                (idx > 0).then(|| (containers[idx - 1].1.clone(), "in"))
            } else if is_mod_doc {
                None
            } else {
                syms.and_then(|v| {
                    let idx = v.partition_point(|(l, _)| *l <= ln);
                    if is_doc {
                        v.get(idx).map(|&(_, e)| (e, "doc of"))
                    } else if idx > 0 {
                        Some((v[idx - 1].1, "in"))
                    } else {
                        None
                    }
                })
                .map(|(e, rel)| (sym_qual(&sym_t, e), rel))
                .filter(|(q, _)| !q.is_empty())
            };
            let text: String = if text.chars().count() > 120 { text.chars().take(120).collect::<String>() + "…" } else { text.to_string() };
            match encl {
                Some((q, rel)) => println!("{path}:{ln}\t{text}\t({rel} {q})"),
                None if is_mod_doc => println!("{path}:{ln}\t{text}\t(module doc)"),
                None => println!("{path}:{ln}\t{text}"),
            }
        }
        n_files += usize::from(file_hits > 0);
    }
    if files_only {
        // 件数降順 → path 昇順。`rg -c` と違い「どの file を先に読むか」が 1 目で決まる順序。
        per_file.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        shown = per_file.len().min(o.limit);
        for (n, p) in per_file.iter().take(o.limit) {
            println!("{p}\t{n} 件");
        }
    }
    let scope = if path_fs.is_empty() { String::new() } else { format!(" (path: {} の {} files)", path_fs.join(" | "), files.len()) };
    if files_only {
        if total == 0 {
            println!("# \"{needle}\" は index 済みファイルに無い{scope}");
        } else {
            let more = if per_file.len() > shown { format!(" — 表示 {shown} file、`--limit {}` で全部", per_file.len()) } else { String::new() };
            println!("# {total} 件 / {} files{scope}{more} — 本文は path: で file を絞って再検索 / `read <path>:<line>`", per_file.len());
        }
    } else if total == 0 {
        println!("# \"{needle}\" は index 済みファイルに無い{scope} (.rs + テキスト全般。binary と >1MiB と gitignore 済みは対象外)");
    } else if total > shown {
        println!("# {total} 件 / {n_files} files{scope} — 表示 {shown}、`--limit {total}` で全部");
    } else {
        println!("# {total} 件 / {n_files} files{scope}");
    }
}

/// `text` の一致判定: 語は `regex::escape`、`-e` なら各語をそのまま正規表現として扱う。
/// 既定 (OR) は全語を 1 本の regex に畳む (語ごとに全 file を舐めるより速い: tokio 770 files で
/// 20ms → 10ms 台)。`--and` は語ごとに regex を持ち、全部当たった時だけ hit (`grep X | grep Y` の
/// 代わり — 畳めないので語数分の走査になる)。大小無視は `(?i)` 相当、`-e` 側は `(?-i)` で打ち消せる。
/// どちらの mode も判定は「全要素が当たるか」= OR は要素 1 個なので同じ式で書ける。
pub(crate) struct TextMatcher(pub(crate) Vec<regex::Regex>);

impl TextMatcher {
    pub(crate) fn new(terms: &[String], regex: bool, and: bool) -> Option<TextMatcher> {
        let pat = |t: &String| if regex { format!("(?:{t})") } else { regex::escape(t) };
        let pats: Vec<String> = if and { terms.iter().map(pat).collect() } else { vec![terms.iter().map(pat).collect::<Vec<_>>().join("|")] };
        let mut res = Vec::with_capacity(pats.len());
        for p in &pats {
            match regex::RegexBuilder::new(p).case_insensitive(true).build() {
                Ok(re) => res.push(re),
                Err(e) => {
                    println!("# 正規表現が不正: {}: {e}", terms.join(" | "));
                    return None;
                }
            }
        }
        Some(TextMatcher(res))
    }
    /// 文字列 (行でもファイル全体でも) が条件を満たすか。OR は畳んだ 1 本、AND は全語。
    /// file 全体に対する hit は AND でも「全語がその file のどこかに在る」= 行 AND の必要条件なので
    /// 前段の足切りとして正しい (行側で本当の AND を取る)。
    pub(crate) fn hit(&self, s: &str) -> bool {
        self.0.iter().all(|r| r.is_match(s))
    }
}

/// 行から行コメント (`//` 以降) と文字列リテラルの中身を落とす (簡易 — ブロックコメントや
/// raw string の複雑形は完全ではないが、doc コメントの誤検出を消すには十分)。
pub(crate) fn strip_comment_and_strings(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let (mut in_str, mut esc, mut prev) = (false, false, '\0');
    for c in line.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            out.push(' ');
            prev = c;
            continue;
        }
        if c == '"' {
            in_str = true;
            out.push(' ');
            prev = c;
            continue;
        }
        if c == '/' && prev == '/' {
            out.pop(); // 直前の '/' も落として行末まで打ち切り
            break;
        }
        out.push(c);
        prev = c;
    }
    out
}

/// 候補 sym の名前が **定義以外の場所に字句として現れるか** を 1 パスで判定する。
/// 返り値 = 「現れた = まだ使われている可能性がある」名前の集合。
/// 定義そのものの行は数えない (定義数だけ差し引く。同じ行に 2 度出る形は「使用あり」に倒す = 安全側)。
pub(crate) fn names_used_lexically(
    paths: &HashMap<EntityId, String>,
    sym_t: &Table,
    hits: &[EntityId],
    dead: &HashSet<EntityId>, // 既に dead と判定済みの定義。その本体の中の出現は「使用」に数えない
) -> HashSet<String> {
    let mut names: Vec<String> = hits.iter().map(|&e| txt(sym_t.entity(e).get("name"))).filter(|n| !n.is_empty()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return HashSet::new();
    }

    let pat = format!(r"\b(?:{})\b", names.iter().map(|n| regex::escape(n)).collect::<Vec<_>>().join("|"));
    let Ok(re) = regex::RegexBuilder::new(&pat).build() else { return HashSet::new() };
    // dead と判定済みの定義の行範囲 (file path → [(start, end)])。ここでの出現は
    // 「死んだコードの中で呼ばれているだけ」なので生存の根拠にしない
    // (これが無いと、dead な関数からしか呼ばれない関数を取り逃す — enchudb の `gallop_ge` が実例)。
    // 除外する行範囲 = 候補自身の定義本体 ∪ dead 判定済みの定義本体。
    // 前者は「自分の定義に自分の名前が出るのは当たり前」、後者は「死んだコードの中の呼び出しは
    // 生存の根拠にならない」(enchudb の `gallop_ge` が実例)。これで残った出現は全部「外からの使用」。
    let mut dead_ranges: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
    for &d in hits.iter().chain(dead.iter()) {
        let er = sym_t.entity(d);
        let Some(p) = paths.get(&ref_of(er.get("file"))) else { continue };
        let (s0, e0) = (num(er.get("line")), num(er.get("end_line")));
        dead_ranges.entry(p.clone()).or_default().push((s0, e0.max(s0)));
    }
    let files: Vec<String> = paths.values().cloned().collect();
    let counted = par_map(&files, |p| {
        let Ok(src) = std::fs::read_to_string(p) else { return HashMap::new() };
        let ranges = dead_ranges.get(p);
        let mut local: HashMap<String, usize> = HashMap::new();
        for (i, raw_line) in src.lines().enumerate() {
            let ln = i as u32 + 1;
            if ranges.is_some_and(|rs| rs.iter().any(|(s, e)| ln >= *s && ln <= *e)) {
                continue; // dead な定義の本体
            }
            // コメントと文字列リテラルは **使用の証拠にしない**。doc コメントは API 名を普通に挙げるので、
            // 数えると本当に dead な物が毎回生き残る (enchudb の `Engine::vocab` が実例)。
            let line = &strip_comment_and_strings(raw_line);
            for m in re.find_iter(line) {
                // `self.vocab` のような **同名フィールドへのアクセス** を method 呼び出しと取り違えない。
                // (enchudb の `Engine::vocab` は本当に dead なのに、field `self.vocab` が 260 箇所
                //  出るせいで「使用中」に見えていた)。`.name(` と `.name` を区別する。
                // 「使用」と数えないのは **束縛・宣言の形**。同名の field / 局所変数は珍しくない
                // (enchudb の `Engine::vocab` は field `vocab: Vocabulary` と local `let vocab = …` の
                //  せいで「使用中」に見えていた)。呼び出し `name(` / `.name(` と、値参照は残す。
                let head = &line[..m.start()];
                let before = head.chars().next_back();
                let rest = line[m.end()..].trim_start();
                let after = rest.chars().next();
                let is_field_access = before == Some('.') && after != Some('(');
                let is_binding = after == Some(':') && !rest.starts_with("::");
                let is_let = head.trim_end().ends_with("let") || head.trim_end().ends_with("mut");
                if is_field_access || is_binding || is_let {
                    continue;
                }
                *local.entry(m.as_str().to_string()).or_default() += 1;
            }
        }
        local
    });
    let mut seen: HashMap<String, usize> = HashMap::new();
    for local in counted {
        for (k, v) in local {
            *seen.entry(k).or_default() += v;
        }
    }
    // 定義本体の外に 1 度でも出れば「使われている可能性あり」= 候補から外す (安全側)。
    seen.into_iter().filter(|(_, c)| *c > 0).map(|(n, _)| n).collect()
}

/// `outline <path|dir>` — ファイルの symbol 一覧。Read せず構造を掴む (path は末尾一致でも可)。
/// dir なら配下の index 済み file を symbol 数 / 行数付きで列挙 (crate の地図。`ls -R` + wc の代わり)。
pub fn cmd_outline(args: &[String]) {
    let o = parse_opts(args);
    let Some(path_arg) = o.pos.first() else {
        eprintln!("usage: kenning outline <path|dir> [--db P]");
        return;
    };
    run_outline(&o.db, path_arg, o.limit);
}

/// outline の本体 (`read <path>` からも使う)。
pub(crate) fn run_outline(db_path: &str, path_arg: &str, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let paths = file_paths(&file_t);

    let Some(fe) = find_file(&file_t, &paths, path_arg) else {
        outline_dir(&file_t, &sym_t, &paths, path_arg, limit);
        return;
    };
    let path = paths.get(&fe).map(String::as_str).unwrap_or("?");
    // 非 Rust は見出し構造 (md の # 階層 / toml の [table] / yaml のキーパス) を出す。`read <file>#<見出し>` の目次。
    if num(file_t.entity(fe).get("lang")) != LANG_RUST {
        let Ok(src) = std::fs::read_to_string(path) else {
            eprintln!("# source を読めない: {path}");
            return;
        };
        let containers = text_containers(num(file_t.entity(fe).get("lang")), &src);
        println!("# {path} : {} sections", containers.len());
        for (l, c) in containers.iter().take(limit) {
            println!("{path}:{l}\t{c}");
        }
        if containers.len() > limit {
            println!("… (+{} 件省略、--limit で全部)", containers.len() - limit);
        }
        return;
    }
    let syms = sym_t.all().where_ref("file", fe).find().unwrap();
    println!("# {path} : {} symbols", syms.len());
    print_syms(&sym_t, &paths, &syms, limit, true);
}

/// `outline <dir>` — 配下の index 済み file を `path<TAB>N symbols / L loc` で列挙。相対 (`src`、
/// `crates/foo`) は path 中の `/<dir>/` 一致、絶対は前方一致。`.` / `..` / `./x` は cwd 基準で
/// 解決してから絶対前方一致 (`outline .` = repo の地図)。無ければ file not found。
pub(crate) fn outline_dir(file_t: &Table, sym_t: &Table, paths: &HashMap<EntityId, String>, dir_arg: &str, limit: usize) {
    let pref = dir_arg.trim_end_matches('/');
    let (abs, inner) = (format!("{pref}/"), format!("/{pref}/"));
    // cwd 基準の実 dir (`.` を含む) は canonicalize して絶対 prefix にする。index の path は絶対なので
    // これが無いと `.` は永久に当たらない。
    let canon = std::fs::canonicalize(if pref.is_empty() { "." } else { pref })
        .ok()
        .filter(|c| c.is_dir())
        .map(|c| format!("{}/", c.to_string_lossy()));
    let mut files: Vec<(&String, EntityId)> = paths
        .iter()
        .filter(|(_, p)| {
            canon.as_deref().is_some_and(|c| p.starts_with(c)) || (!pref.is_empty() && (p.starts_with(&abs) || p.contains(&inner)))
        })
        .map(|(e, p)| (p, *e))
        .collect();
    if files.is_empty() {
        eprintln!("# file not found: {dir_arg}");
        return;
    }
    // 表示名: `.` のような cwd 相対は解決後の絶対 dir を出す (どこを数えたか曖昧にしない)。
    let pref = match canon.as_deref() {
        Some(c) if pref.is_empty() || pref.starts_with('.') => c.trim_end_matches('/').to_string(),
        _ => pref.to_string(),
    };
    files.sort();
    let mut n_syms: HashMap<EntityId, usize> = HashMap::new();
    for s in sym_t.all().find().unwrap() {
        *n_syms.entry(ref_of(sym_t.entity(s).get("file"))).or_default() += 1;
    }
    println!("# {pref} : {} files (詳細は outline <path>)", files.len());
    for (p, e) in files.iter().take(limit) {
        let er = file_t.entity(*e);
        let loc = num(er.get("loc"));
        match n_syms.get(e) {
            Some(n) if num(er.get("lang")) == LANG_RUST => println!("{p}\t{n} symbols / {loc} loc"),
            _ => println!("{p}\t{loc} loc"),
        }
    }
    if files.len() > limit {
        println!("… (+{} 件省略、--limit で全部)", files.len() - limit);
    }
}

/// `stats` — index の規模と名前解決率。
pub fn cmd_stats(args: &[String]) {
    let o = parse_opts(args);
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let n_call = call_t.all().count().unwrap();
    // file 表には非 Rust テキストも載る。合算だけ出すと「Rust の規模」に見えてしまうので分けて出す。
    let n_rust = file_t.where_eq("lang", LANG_RUST).count().unwrap();
    let n_text = file_t.all().count().unwrap() - n_rust;
    println!(
        "index: {} files ({} rust + {} text) / {} symbols / {} call-sites",
        n_rust + n_text,
        n_rust,
        n_text,
        sym_t.all().count().unwrap(),
        n_call
    );
    // 枠使用率: with_capacity は create 時固定なので、満杯に近い table を可視化 (満杯は拒否 → 再 index)。
    let usage: Vec<String> = ["file", "sym", "call", "ref", "impl", "extref"]
        .iter()
        .filter_map(|t| db.table_eid_usage(t).map(|u| format!("{t} {}%", u.allocated as u64 * 100 / u.capacity.max(1) as u64)))
        .collect();
    println!("capacity: {} (残 eid {})", usage.join(" "), db.remaining_eid_capacity());
    // bake の有無は **精度そのもの**なのに、今までは capacity 行の `ref x%` から読むしかなかった。
    // (再 index で SCIP が落ちても気付けず、impact が静かに縮む事故が起きる)
    let n_ref = db.get_table("ref").map(|t| t.all().count().unwrap_or(0)).unwrap_or(0);
    match (scip_stale_files(&db), n_ref) {
        (Some(u), n) if u >= SCIP_STALE_FILES => println!("bake: 済み ({n} refs) だが bake 後 {u} ファイル変更 → `kenning bake` で焼き直しを"),
        (Some(_), n) => println!("bake: 済み ({n} refs) = refs / callers は rust-analyzer 同等精度"),
        (None, _) => println!("bake: 無し = syn 層のみ (確実 edge は控えめ)。`kenning bake` で精密化"),
    }
    // 解決率は **外部呼び出しを分母から外して**出す。std / 依存 crate への呼び出しは index に
    // 定義が無く構造的に解決不能で、混ぜると「率が低い = 精度が低い」に読めてしまうため
    // (実際に測れるのは corpus の外部依存率)。精度は repo 内呼び出しの確定率の方。
    // `stats path:<substr>` — repo の一部だけの確定率 (どこなら確定を信じてよいかが分かる)。
    let filter = o.pos.iter().find_map(|a| a.strip_prefix("path:").filter(|s| !s.is_empty()));
    let rs = match filter {
        Some(needle) => ResolveStats::of_path(&call_t, &file_paths(&file_t), needle),
        None => ResolveStats::of(&call_t),
    };
    if let Some(needle) = filter {
        println!("(path:{needle} で絞り込み: {} call-sites)", rs.total);
    }
    print!("resolve:");
    for (r, nm) in RES_NAMES.iter().enumerate() {
        print!(" {nm}={}", rs.counts[r]);
    }
    println!();
    println!(
        "  repo 内 {} 件 (外部/std {} を除く) の確定率 {:.1}% — 未確定 {}: 受け手不明の method {} / 同名複数 {} / 値渡し {} / マクロ {} / 未解決 {}",
        rs.local(),
        rs.n(R_EXTERNAL),
        rs.local_pct(),
        rs.local() - rs.confirmed(),
        rs.n(R_METHOD),
        rs.n(R_AMBIG),
        rs.n(R_VALUE),
        rs.n(R_MACRO),
        rs.n(R_UNRESOLVED),
    );

    // crate 別の symbol 数 (新しい repo での当たり付け = どこにコードの重心があるか)。
    let mut set = std::collections::BTreeSet::new();
    for e in file_t.all().find().unwrap() {
        set.insert(txt(file_t.entity(e).get("crate_")));
    }
    let mut rows: Vec<(usize, String)> = set
        .into_iter()
        .map(|c| (sym_t.where_eq("crate_", c.as_str()).count().unwrap(), c))
        .collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    println!("crates ({}):", rows.len());
    for (n, c) in rows.iter().take(20) {
        println!("  {n:>5}  {c}");
    }
    if rows.len() > 20 {
        println!("  … (+{} crates)", rows.len() - 20);
    }
}

// ─────────────────────────── bench (デモ / 計測) ───────────────────────────

/// best-of-N latency で count クエリを計測して 1 行出す。
pub(crate) fn timed<F: Fn() -> usize>(label: &str, f: F) -> usize {
    let mut best = Duration::MAX;
    let mut cnt = 0;
    for _ in 0..50 {
        let t = Instant::now();
        cnt = f();
        best = best.min(t.elapsed());
    }
    println!("  {:<52} = {:>7} 件  [{:?}]", label, cnt, best);
    cnt
}
