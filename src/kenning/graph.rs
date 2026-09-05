//! グラフ系クエリ (across / refs / impact / tests / path) と近い名前の提案。

use super::*;

/// `across <name>` — **全 repo 横断**: cache 内の全 repo db を走査し、name の定義・利用を鳥瞰する。
/// さらに SCIP symbol (グローバル一意) を extref と突き合わせ、**repo を跨いだ精密参照**
/// (例: 利用側 repo が enchudb::finish_with_oplog を使う行) を出す。RA は single-workspace なので出せない問い。
pub fn cmd_across(args: &[String]) {
    // db は使わない (cache 全走査) ので parse_opts の auto 魔法は通さない。
    let mut limit = 20usize;
    let mut name: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => { i += 1; if let Some(v) = args.get(i) { limit = v.parse().unwrap_or(20); } }
            other => name = Some(other.to_string()),
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("usage: kenning across <name> [--limit N]");
        return;
    };
    let Some(cache) = cache_dir() else { return };
    let mut dbs: Vec<String> = std::fs::read_dir(&cache)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path().to_string_lossy().to_string())
                .filter(|p| p.ends_with(".db"))
                .collect()
        })
        .unwrap_or_default();
    dbs.sort();
    if dbs.is_empty() {
        println!("# cache に index が無い (~/.cache/kenning)。各 repo で一度 kenning を叩くと増える。");
        return;
    }

    // pass1: repo ごとの定義 / 確実 callers / 名前一致。定義の SCIP symbol を集める (pass2 の鍵)。
    let mut symbols: Vec<String> = Vec::new(); // 定義側のグローバル一意 symbol
    let mut opened = 0usize;
    println!("# across \"{name}\" — {} repo index を走査:", dbs.len());
    // db は互いに独立 (open は syscall 支配) なので thread で撒く。出力は入力順に戻すので決定的。
    let pass1: Vec<(String, Vec<String>, bool)> = par_dbs(&dbs, |dbp| {
        use std::fmt::Write as _;
        let mut out = String::new();
        let mut syms: Vec<String> = Vec::new();
        let Ok(db) = Database::open_readonly(dbp) else { return (out, syms, false) };
        let repo = read_meta(&db)
            .map(|(r, _)| r.rsplit('/').next().unwrap_or(&r).to_string())
            .unwrap_or_else(|| dbp.rsplit('/').next().unwrap_or(dbp).to_string());
        let (Some(file_t), Some(sym_t), Some(call_t)) = (db.get_table("file"), db.get_table("sym"), db.get_table("call")) else {
            return (out, syms, true);
        };
        let defs = sym_t.where_eq("name", name.as_str()).find().unwrap_or_default();
        let name_calls = call_t.where_eq("callee", name.as_str()).count().unwrap_or(0);
        if defs.is_empty() && name_calls == 0 {
            return (out, syms, true);
        }
        let paths = file_paths(&file_t);
        let mut precise = 0usize;
        for &d in &defs {
            precise += call_t.where_eq("callee_sym", Value::Ref(d)).count().unwrap_or(0);
            let s = txt(sym_t.entity(d).get("symbol"));
            if !s.is_empty() {
                syms.push(s);
            }
        }
        let _ = writeln!(out, "{repo}: 定義 {} / 確実 callers {} / 名前一致 call {}", defs.len(), precise, name_calls);
        for &d in defs.iter().take(3) {
            let _ = writeln!(out, "  {}", fmt_sym(&sym_t, &paths, d));
        }
        (out, syms, true)
    });
    for (out, syms, ok) in pass1 {
        if ok {
            opened += 1;
        }
        print!("{out}");
        symbols.extend(syms);
    }

    // pass2: 定義 symbol を全 repo の extref と突き合わせ = repo を跨いだ精密参照。
    symbols.sort();
    symbols.dedup();
    if !symbols.is_empty() {
        let mut n_x = 0usize;
        let mut shown = 0usize;
        // pass1 と同じく db 単位で撒く。limit の適用は入力順に戻してから (出力は逐次版と同じ)。
        let pass2: Vec<Vec<String>> = par_dbs(&dbs, |dbp| {
            use std::fmt::Write as _;
            let mut lines: Vec<String> = Vec::new();
            let Ok(db) = Database::open_readonly(dbp) else { return lines };
            let Some(extref_t) = db.get_table("extref") else { return lines };
            let Some(file_t) = db.get_table("file") else { return lines };
            let repo = read_meta(&db)
                .map(|(r, _)| r.rsplit('/').next().unwrap_or(&r).to_string())
                .unwrap_or_default();
            let paths = file_paths(&file_t);
            for s in &symbols {
                for r in extref_t.where_eq("symbol", s.as_str()).find().unwrap_or_default() {
                    let er = extref_t.entity(r);
                    let p = paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default();
                    let mut line = String::new();
                    let _ = write!(line, "  ✕ {repo} → {name}: {p}:{}\t[{}]", num(er.get("line")), role_name(num(er.get("role"))));
                    lines.push(line);
                }
            }
            lines
        });
        for lines in pass2 {
            for line in lines {
                n_x += 1;
                if shown >= limit {
                    continue;
                }
                shown += 1;
                println!("{line}");
            }
        }
        if n_x > 0 {
            println!("# repo 跨ぎ精密参照 (extref×SCIP symbol): {n_x} 件");
            if n_x > shown {
                println!("  … (+{} 件省略)", n_x - shown);
            }
        } else {
            println!("# repo 跨ぎ精密参照: 0 件 (利用側 repo が bake 済みの時だけ出る)");
        }
    }
    eprintln!("# ({opened}/{} db を走査。各 repo の鮮度は個別 query 時に自動 update)", dbs.len());
}

/// cache 内の db を thread で撒いて処理し、**入力順**で結果を返す。
/// 1 db の open は syscall (open/fstat/mmap) 支配で CPU をほぼ使わないので、
/// 逐次だと db 数 × open 代がそのまま wall-clock に乗る。
pub(crate) fn par_dbs<T: Send>(dbs: &[String], f: impl Fn(&String) -> T + Sync) -> Vec<T> {
    let n = std::thread::available_parallelism().map(|v| v.get()).unwrap_or(4).min(dbs.len());
    if n <= 1 {
        return dbs.iter().map(f).collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let slots: Vec<std::sync::Mutex<Option<T>>> = (0..dbs.len()).map(|_| std::sync::Mutex::new(None)).collect();
    std::thread::scope(|sc| {
        for _ in 0..n {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(dbp) = dbs.get(i) else { break };
                let v = f(dbp);
                *slots[i].lock().unwrap() = Some(v);
            });
        }
    });
    slots.into_iter().map(|m| m.into_inner().unwrap().expect("slot filled")).collect()
}

/// SCIP role bit → 表示名。
pub(crate) fn role_name(r: u32) -> &'static str {
    if r & 1 != 0 { "def" }
    else if r & 4 != 0 { "write" }
    else if r & 2 != 0 { "import" }
    else if r & 8 != 0 { "read" }
    else { "ref" }
}

/// `refs <name> [container]` — 正確な find-all-refs (SCIP 全 occurrence の逆引き、要 --scip index)。
/// who-calls の上位互換: 呼び出しだけでなく読み/書き/型参照も含む全参照。
pub fn cmd_refs(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning refs <name> [container] [--db P] [--limit N]");
        return;
    };
    let container = o.pos.get(1).map(String::as_str);
    let Some(db) = open_ro(&o.db) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let ref_t = db.get_table("ref").unwrap();
    let paths = file_paths(&file_t);

    if ref_t.all().count().unwrap() == 0 {
        println!("# ref table が空 = SCIP 無しで index された。`index <dir> --scip <f>` で正確 refs が使える。");
        return;
    }

    let mut defs = sym_t.where_eq("name", name).find().unwrap();
    if let Some(c) = container {
        defs.retain(|&e| txt(sym_t.entity(e).get("container")) == c);
    }
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い。");
        suggest_similar(&sym_t, &paths, name);
        return;
    }

    // 同名多数 & container 未指定 → 定義ごとの参照数の要約 + 絞り方。
    if container.is_none() && defs.len() > 1 {
        let mut rows: Vec<(usize, EntityId)> = defs
            .iter()
            .map(|&d| (ref_t.where_eq("symbol_sym", Value::Ref(d)).count().unwrap(), d))
            .collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0));
        println!("# \"{name}\" は {} 型が定義。定義ごとの参照数:", defs.len());
        for (n, d) in rows.iter().take(o.limit) {
            let er = sym_t.entity(*d);
            let ct = txt(er.get("container"));
            println!("  {n:>5}  {}::{name}  ({})", if ct.is_empty() { "·".into() } else { ct }, txt(er.get("crate_")));
        }
        println!("# 絞る: `refs {name} <container>`");
        return;
    }

    for &d in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, d));
        let refs = ref_t.where_eq("symbol_sym", Value::Ref(d)).find().unwrap();
        let mut rows: Vec<(String, u32, u32)> = refs
            .iter()
            .map(|&r| {
                let er = ref_t.entity(r);
                (paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")), num(er.get("role")))
            })
            .collect();
        rows.sort();
        println!("  {} refs:", rows.len());
        for (p, ln, role) in rows.iter().take(o.limit) {
            println!("    {p}:{ln}\t[{}]", role_name(*role));
        }
        if rows.len() > o.limit {
            println!("    … (+{} 件省略)", rows.len() - o.limit);
        }
    }
    // refs は SCIP 確定のみ = 精密だが cfg 非活性/未解析域は落ちる。superset が要るなら find/grep。
    println!("# SCIP 確定参照のみ (誤りなし)。cfg 非活性/未解析域は含まない → superset は `find {name}` / grep。");
}

// ─────────────────────────── 多段 graph (推移的 callers / 呼び出し経路) ───────────────────────────
// call table = caller(sym) → callee_sym(sym) の有向 edge。確定 edge (callee_sym set) だけを辿る。
// grep も単発 LSP も出せない「推移的な影響範囲 / 経路」を edge 逆引き µs で出す。

/// 名前が index に無い時の発見導線: 近い定義名を数件提案する。
/// `foo_bar` を `_` 分割し、長さ 4+ の token を含む定義名を拾う (typo/部分名を救う)。
/// 例: `finish_oplog` → token [finish, oplog] → `finish_with_oplog` が引っかかる。
/// 近い名前の提案に使う token / 定義名の最小長。これ未満 (`_` / `Op` / `id`) は
/// 何にでも含まれるので「近い」の根拠にならない。
pub(crate) const SUGGEST_MIN_TOKEN: usize = 4;

/// `name` を提案用 token に分解 (小文字化済み、`_`/記号区切り、短い断片は捨てる)。
pub(crate) fn suggest_tokens(lname: &str) -> Vec<&str> {
    lname.split(|c: char| c == '_' || !c.is_alphanumeric()).filter(|t| t.len() >= SUGGEST_MIN_TOKEN).collect()
}

/// 定義名 `ldn` (小文字) が問い合わせ `lname` にどれだけ近いか。一致 token 数 + substring 全体一致 1 点。
/// 逆包含 (`lname` が `ldn` を含む) は `ldn` が十分長い時だけ — 短い定義名は何にでも含まれる
/// (`copy_sparse` の候補に `_` や `Op` が出ていた)。
pub(crate) fn suggest_score(tokens: &[&str], lname: &str, ldn: &str) -> u32 {
    let mut score = tokens.iter().filter(|t| ldn.contains(**t)).count() as u32;
    if ldn.contains(lname) || (ldn.len() >= SUGGEST_MIN_TOKEN && lname.contains(ldn)) {
        score += 1;
    }
    score
}

pub(crate) fn suggest_similar(sym_t: &Table, paths: &HashMap<EntityId, String>, name: &str) {
    let lname = name.to_lowercase();
    let tokens = suggest_tokens(&lname);
    let mut best: HashMap<String, (u32, EntityId)> = HashMap::new(); // name → (score, 代表 eid)
    for e in sym_t.all().find().unwrap() {
        let dn = txt(sym_t.entity(e).get("name"));
        let score = suggest_score(&tokens, &lname, &dn.to_lowercase());
        if score > 0 {
            let slot = best.entry(dn).or_insert((0, e));
            if score > slot.0 { *slot = (score, e); }
        }
    }
    if best.is_empty() {
        println!("# 近い名前なし。`find <部分文字列>` で探せる。");
        return;
    }
    let mut ranked: Vec<(u32, String, EntityId)> = best.into_iter().map(|(n, (s, e))| (s, n, e)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1))); // score 降順 → 名前昇順
    println!("# \"{name}\" は無い。近い名前:");
    for (_, n, e) in ranked.iter().take(8) {
        let er = sym_t.entity(*e);
        let ct = txt(er.get("container"));
        let qn = if ct.is_empty() { n.clone() } else { format!("{ct}::{n}") };
        let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
        println!("  {qn}\t{path}:{}", num(er.get("line")));
    }
}

/// name[+container] → 定義 eid 群 (callers/refs/impact/path 共通、DRY)。
pub(crate) fn defs_of(sym_t: &Table, name: &str, container: Option<&str>) -> Vec<EntityId> {
    let mut defs = sym_t.where_eq("name", name).find().unwrap();
    if let Some(c) = container {
        defs.retain(|&e| txt(sym_t.entity(e).get("container")) == c);
    }
    defs
}

/// 逆方向 BFS: 起点 = 全定義。depth 別に確定 caller を層状に集める (impact / tests 共用)。
/// 返り値 [depth1 の層, depth2 の層, …] (起点は含まない)。
pub(crate) fn caller_bfs(call_t: &Table, defs: &[EntityId]) -> Vec<Vec<EntityId>> {
    let mut depth: HashMap<EntityId, u32> = defs.iter().map(|&d| (d, 0)).collect();
    let mut frontier: Vec<EntityId> = defs.to_vec();
    let mut by_depth: Vec<Vec<EntityId>> = Vec::new();
    let mut d = 0u32;
    while !frontier.is_empty() && d < 64 {
        let mut next = Vec::new();
        for &s in &frontier {
            for caller in direct_callers(call_t, s) {
                if let std::collections::hash_map::Entry::Vacant(e) = depth.entry(caller) {
                    e.insert(d + 1);
                    next.push(caller);
                }
            }
        }
        if !next.is_empty() {
            by_depth.push(next.clone());
        }
        frontier = next;
        d += 1;
    }
    by_depth
}

/// s を確定 edge で呼ぶ直接 caller の sym eid 群 (逆方向)。
pub(crate) fn direct_callers(call_t: &Table, s: EntityId) -> Vec<EntityId> {
    call_t
        .where_eq("callee_sym", Value::Ref(s))
        .find()
        .unwrap()
        .into_iter()
        .map(|c| ref_of(call_t.entity(c).get("caller")))
        .collect()
}

/// s が確定 edge で呼ぶ直接 callee の sym eid 群 (前方)。
pub(crate) fn direct_callees(call_t: &Table, s: EntityId) -> Vec<EntityId> {
    call_t
        .where_eq("caller", Value::Ref(s))
        .find()
        .unwrap()
        .into_iter()
        .filter_map(|c| match call_t.entity(c).get("callee_sym") {
            Some(Value::Ref(e)) => Some(e),
            _ => None,
        })
        .collect()
}

/// sym eid 群を (path, line) 昇順で `path:line<TAB>詳細` 出力 (limit 超は件数明示)。
pub(crate) fn print_sym_layer(sym_t: &Table, paths: &HashMap<EntityId, String>, eids: &[EntityId], limit: usize, indent: &str) {
    let mut rows: Vec<(String, u32, EntityId)> = eids
        .iter()
        .map(|&e| (paths.get(&ref_of(sym_t.entity(e).get("file"))).cloned().unwrap_or_default(), num(sym_t.entity(e).get("line")), e))
        .collect();
    rows.sort();
    for (_, _, e) in rows.iter().take(limit) {
        println!("{indent}{}", fmt_sym(sym_t, paths, *e));
    }
    if rows.len() > limit {
        println!("{indent}… (+{} 件省略、--limit で全部)", rows.len() - limit);
    }
}

/// `impact <name> [container]` — 推移的 callers (逆方向 BFS)。「これを変えると壊れる範囲」。
pub fn cmd_impact(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning impact <name> [container] [--db P] [--limit N]");
        return;
    };
    run_impact(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

pub(crate) fn run_impact(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い{}。", container.map(|c| format!(" (container={c})")).unwrap_or_default());
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    let by_depth = caller_bfs(&call_t, &defs);
    for &def in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, def));
    }
    let total: usize = by_depth.iter().map(|v| v.len()).sum();
    for (i, layer) in by_depth.iter().enumerate() {
        println!("  depth {} ({} sym){}:", i + 1, layer.len(), if i == 0 { " = 直接 callers" } else { "" });
        print_sym_layer(&sym_t, &paths, layer, limit, "    ");
    }
    println!("# 推移的 callers: {total} sym (確定 edge のみ = 影響の下界。候補/未解決 edge は未算入 → `callers` で確認)");
}

/// `tests <name> [container]` — この sym を (推移的に) 呼ぶテスト = impact ∩ is_test。
/// 「これを変えたらどのテストを回すか」を 1 コマンドで。
pub fn cmd_tests(args: &[String]) {
    let o = parse_opts(args);
    let Some(name) = o.pos.first() else {
        eprintln!("usage: kenning tests <name> [container] [--db P] [--limit N]");
        return;
    };
    run_tests(&o.db, name, o.pos.get(1).map(String::as_str), o.limit);
}

/// `cargo test -- <filter...>` のヒントに載せる最大テスト数 (多すぎたら cargo test 全部が早い)。
pub(crate) const TESTS_HINT_MAX: usize = 8;

pub(crate) fn run_tests(db_path: &str, name: &str, container: Option<&str>, limit: usize) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let defs = defs_of(&sym_t, name, container);
    if defs.is_empty() {
        println!("# \"{name}\" の定義が index に無い{}。", container.map(|c| format!(" (container={c})")).unwrap_or_default());
        suggest_similar(&sym_t, &paths, name);
        return;
    }
    for &def in &defs {
        println!("{}", fmt_sym(&sym_t, &paths, def));
    }
    // BFS 全層から is_test=1 だけ拾う (depth = 起点からの呼び出し距離)。
    let mut tests: Vec<(u32, EntityId)> = Vec::new();
    for (i, layer) in caller_bfs(&call_t, &defs).iter().enumerate() {
        for &e in layer {
            if num(sym_t.entity(e).get("is_test")) == 1 {
                tests.push((i as u32 + 1, e));
            }
        }
    }
    if tests.is_empty() {
        println!("# {name} に届くテストなし (確定 edge のみ。候補 edge の見逃しは `callers {name}` の⚠で確認)");
        return;
    }
    tests.sort_by_key(|&(d, e)| {
        let er = sym_t.entity(e);
        (d, paths.get(&ref_of(er.get("file"))).cloned().unwrap_or_default(), num(er.get("line")))
    });
    for (d, e) in tests.iter().take(limit) {
        println!("  [d{d}] {}", fmt_sym(&sym_t, &paths, *e));
    }
    if tests.len() > limit {
        println!("  … (+{} 件省略、--limit で全部)", tests.len() - limit);
    }
    println!("# {name} に届くテスト: {} 件 (確定 edge のみ = 下界)", tests.len());
    // そのまま貼れる実行ヒント (libtest は複数 filter を OR で受ける)。
    let names: Vec<String> = tests.iter().map(|&(_, e)| txt(sym_t.entity(e).get("name"))).collect::<HashSet<_>>().into_iter().collect();
    if names.len() <= TESTS_HINT_MAX {
        let mut ns = names;
        ns.sort();
        println!("# 実行: cargo test -- {}", ns.join(" "));
    }
}

/// `path <from> <to>` — from が to を(推移的に)呼ぶ最短経路を 1 本 (前方 BFS)。
pub fn cmd_path(args: &[String]) {
    let o = parse_opts(args);
    let (Some(from), Some(to)) = (o.pos.first(), o.pos.get(1)) else {
        eprintln!("usage: kenning path <from> <to> [--db P]");
        return;
    };
    run_path(&o.db, from, to);
}

pub(crate) fn run_path(db_path: &str, from: &str, to: &str) {
    let Some(db) = open_ro(db_path) else { return };
    let file_t = db.get_table("file").unwrap();
    let sym_t = db.get_table("sym").unwrap();
    let call_t = db.get_table("call").unwrap();
    let paths = file_paths(&file_t);

    let srcs = defs_of(&sym_t, from, None);
    let tgts: HashSet<EntityId> = defs_of(&sym_t, to, None).into_iter().collect();
    if srcs.is_empty() || tgts.is_empty() {
        // どちらが無いのかを名指しし、callers と同じ typo 救済を出す (`(ok / X)` は読めなかった)。
        let missing: Vec<&str> = [(srcs.is_empty(), from), (tgts.is_empty(), to)].iter().filter(|(e, _)| *e).map(|(_, n)| *n).collect();
        println!("# 定義が index に無い: {}", missing.join(", "));
        for m in &missing {
            suggest_similar(&sym_t, &paths, m);
        }
        return;
    }
    // 多始点前方 BFS。parent で経路復元。始点が既に target ならそれ自身が経路。
    let mut parent: HashMap<EntityId, EntityId> = HashMap::new();
    let mut visited: HashSet<EntityId> = srcs.iter().copied().collect();
    let mut queue: std::collections::VecDeque<EntityId> = srcs.iter().copied().collect();
    let mut hit = srcs.iter().copied().find(|s| tgts.contains(s));
    while hit.is_none() {
        let Some(s) = queue.pop_front() else { break };
        for callee in direct_callees(&call_t, s) {
            if !visited.contains(&callee) {
                visited.insert(callee);
                parent.insert(callee, s);
                if tgts.contains(&callee) {
                    hit = Some(callee);
                    break;
                }
                queue.push_back(callee);
            }
        }
    }
    let Some(t) = hit else {
        println!("# {from} → {to}: 確定 edge で経路なし (候補/未解決 edge 経由なら在るかも)");
        return;
    };
    // t から parent を遡って経路復元 → 反転。
    let mut chain = vec![t];
    let mut cur = t;
    while let Some(&p) = parent.get(&cur) {
        chain.push(p);
        cur = p;
    }
    chain.reverse();
    for (i, &e) in chain.iter().enumerate() {
        let er = sym_t.entity(e);
        let path = paths.get(&ref_of(er.get("file"))).map(String::as_str).unwrap_or("?");
        println!("  {}{}\t{}:{}", if i == 0 { "" } else { "→ " }, sym_qual(&sym_t, e), path, num(er.get("line")));
    }
    println!("# {} hops (確定 edge のみ・最短)", chain.len() - 1);
}

/// sym eid → "Container::name" (経路の compact ラベル)。
pub(crate) fn sym_qual(sym_t: &Table, eid: EntityId) -> String {
    let er = sym_t.entity(eid);
    let ct = txt(er.get("container"));
    let nm = txt(er.get("name"));
    if ct.is_empty() { nm } else { format!("{ct}::{nm}") }
}
