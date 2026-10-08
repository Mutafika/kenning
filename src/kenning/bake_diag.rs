use super::*;

// ── bake の失敗を覚えて、普段の query で言う (#18) ──
// bake は失敗しても stderr (自動 bake なら log) に出して終わるだけだった。普段の query は未 bake の精度で
// 黙って答え続け、enchudb では 7 週間誰も気づかなかった。失敗は `<db>.bake-fail` に残し、成功で消す。
// query 側は file を 1 つ読むだけ (遅くしない)。

pub(crate) fn bake_fail_marker(db: &str) -> String {
    format!("{db}.bake-fail") // `<db>.` prefix = cache prune の道連れ対象
}

/// 記録された失敗。`since` = 連続失敗の最初、`last` = 直近、`last_ok` = 最後に成功した bake (無ければ 0)。
#[derive(Debug, PartialEq)]
pub(crate) struct BakeFailure {
    pub since: u64,
    pub last: u64,
    pub last_ok: u64,
    pub reason: String,
}

pub(crate) fn read_bake_failure(db: &str) -> Option<BakeFailure> {
    let s = std::fs::read_to_string(bake_fail_marker(db)).ok()?;
    let field = |k: &str| s.lines().find_map(|l| l.strip_prefix(k)?.strip_prefix('\t')).map(str::to_string);
    let n = |k: &str| field(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(BakeFailure { since: n("since"), last: n("last"), last_ok: n("last_ok"), reason: field("reason").unwrap_or_default() })
}

/// 失敗を記録 (連続失敗なら `since` は最初の時刻のまま)。`last_ok` は db の meta の baked_at。
pub(crate) fn record_bake_failure(db: &str, reason: &str) {
    let now = u64::from(now_secs());
    let since = read_bake_failure(db).map(|f| f.since).filter(|&s| s > 0).unwrap_or(now);
    let last_ok = Database::open_readonly(db)
        .ok()
        .and_then(|d| {
            let mt = d.get_table("meta")?;
            let e = mt.all().find().ok()?.into_iter().next()?;
            Some(num(mt.entity(e).get("baked_at")) as u64)
        })
        .unwrap_or(0);
    let reason = reason.lines().next().unwrap_or("").replace('\t', " ");
    let _ = std::fs::write(bake_fail_marker(db), format!("since\t{since}\nlast\t{now}\nlast_ok\t{last_ok}\nreason\t{reason}\n"));
}

pub(crate) fn clear_bake_failure(db: &str) {
    let _ = std::fs::remove_file(bake_fail_marker(db));
}

/// rust-analyzer の stderr から、失敗の理由を 1 行に (panic なら panic の本文、無ければ最初の error 行)。
pub(crate) fn ra_failure_reason(errs: &str) -> String {
    let lines: Vec<&str> = errs.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if let Some(i) = lines.iter().position(|l| l.contains("panicked at")) {
        let msg = lines.get(i + 1).copied().unwrap_or("");
        return format!("rust-analyzer panicked: {}", msg.trim_end_matches('.'));
    }
    match lines.iter().find(|l| l.to_ascii_lowercase().contains("error")).or(lines.last()) {
        Some(l) => format!("rust-analyzer failed: {l}"),
        None => "rust-analyzer failed without output".to_string(),
    }
}

/// UNIX 秒 → `YYYY-MM-DD` (UTC)。日付ライブラリを足さないための days-from-civil の逆。
pub(crate) fn ymd(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// query の先頭で言う 1 行 (失敗の記録が無ければ None)。
pub(crate) fn bake_failure_warning(db: &str) -> Option<String> {
    let f = read_bake_failure(db)?;
    let state = if f.last_ok > 0 {
        format!("last good bake {}; files changed since then are at syn (heuristic) precision", ymd(f.last_ok))
    } else {
        "never baked successfully; everything is at syn (heuristic) precision".to_string()
    };
    Some(format!("# ⚠ bake failing since {} ({}) — {state}. details: `kenning bake`", ymd(f.since), f.reason))
}

// ── 「file emitted multiple times」の原因 file を特定する (#19) ──
// rust-analyzer の SCIP 出力は、同じ file が複数の crate に属すると panic する (file 名は出さない)。
// cargo の定番 `tests/common/mod.rs` を複数の integration test が `mod common;` すると踏む。
// 各 crate root (lib / main / bin / tests / examples / benches / build.rs / Cargo.toml の path =) から
// `mod x;` を辿り、2 つ以上の root から届く file を出す。bake が失敗した時だけ走る (syn で読み直す)。

/// `dir` 以下の各 package の crate root (cargo の自動検出 + Cargo.toml の `path = "..."`)。
pub(crate) fn crate_roots_under(dir: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let manifests: Vec<PathBuf> = index_walk(&dir.to_string_lossy())
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_name() == "Cargo.toml")
        .map(|e| e.path().to_path_buf())
        .collect();
    for m in manifests {
        let Ok(toml) = std::fs::read_to_string(&m) else { continue };
        if !toml.lines().any(|l| l.trim() == "[package]") {
            continue; // workspace だけの manifest
        }
        let pkg = m.parent().unwrap_or(dir).to_path_buf();
        for f in ["src/lib.rs", "src/main.rs", "build.rs"] {
            roots.push(pkg.join(f));
        }
        for sub in ["src/bin", "tests", "examples", "benches"] {
            let Ok(rd) = std::fs::read_dir(pkg.join(sub)) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "rs") {
                    roots.push(p);
                } else if p.is_dir() {
                    roots.push(p.join("main.rs")); // tests/foo/main.rs 形
                }
            }
        }
        // [lib] / [[bin]] / [[test]] / [[example]] / [[bench]] の `path = "..."`
        let mut in_target = false;
        for l in toml.lines().map(str::trim) {
            if l.starts_with('[') {
                in_target = matches!(l, "[lib]" | "[[bin]]" | "[[test]]" | "[[example]]" | "[[bench]]");
            } else if in_target
                && let Some(v) = l.strip_prefix("path").map(str::trim_start).and_then(|r| r.strip_prefix('='))
            {
                roots.push(pkg.join(v.trim().trim_matches('"')));
            }
        }
    }
    let mut seen = HashSet::new();
    roots.retain(|r| r.is_file() && seen.insert(r.clone()));
    roots
}

/// root から `mod x;` を辿って届く file (root 自身を含む)。`#[path]` と inline module の入れ子も辿る。
pub(crate) fn module_files(root: &Path) -> HashSet<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::new();
    let mut stack = vec![(root.to_path_buf(), true)];
    while let Some((file, is_root)) = stack.pop() {
        if !out.insert(file.clone()) {
            continue;
        }
        let Some(ast) = std::fs::read_to_string(&file).ok().and_then(|s| syn::parse_file(&s).ok()) else { continue };
        let dir = file.parent().unwrap_or(Path::new("")).to_path_buf();
        let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // 子 module の置き場所: root / mod.rs なら同じ dir、それ以外は file 名の dir の下。
        let base = if is_root || stem == "mod" { dir.clone() } else { dir.join(stem) };
        collect_mods(&ast.items, &base, &dir, &mut stack);
    }
    out
}

fn collect_mods(items: &[syn::Item], base: &Path, path_dir: &Path, stack: &mut Vec<(PathBuf, bool)>) {
    for it in items {
        let syn::Item::Mod(m) = it else { continue };
        let path_attr = m.attrs.iter().find(|a| a.path().is_ident("path")).and_then(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        });
        let name = m.ident.to_string();
        match &m.content {
            Some((_, inner)) => collect_mods(inner, &base.join(&name), &path_dir.join(&name), stack),
            None => {
                let target = match path_attr {
                    Some(p) => path_dir.join(p),
                    None if base.join(format!("{name}.rs")).is_file() => base.join(format!("{name}.rs")),
                    None => base.join(&name).join("mod.rs"),
                };
                stack.push((target, false));
            }
        }
    }
}

/// 2 つ以上の crate root から届く file → それを含む root 一覧 (どちらも path 昇順)。
pub(crate) fn files_shared_by_crates(dir: &Path) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut by_file: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
    for r in crate_roots_under(dir) {
        for f in module_files(&r) {
            if f != r {
                by_file.entry(f).or_default().push(r.clone());
            }
        }
    }
    let mut out: Vec<(PathBuf, Vec<PathBuf>)> = by_file.into_iter().filter(|(_, rs)| rs.len() > 1).collect();
    for (_, rs) in out.iter_mut() {
        rs.sort();
    }
    // 入口 (`mod.rs`) を先に: 直すのは `mod common;` の側なので、その下の file より先に名指しする。
    out.sort_by_key(|(f, _)| (f.file_name().is_none_or(|n| n != "mod.rs"), f.clone()));
    out
}

/// bake 失敗の案内: 複数 crate に属する file を名指しする。(案内の数行, 記録用の 1 行要約)。無ければ None。
pub(crate) fn shared_file_report(dir: &Path) -> Option<(String, String)> {
    let shared = files_shared_by_crates(dir);
    let base = dir.to_string_lossy().to_string();
    let rel = |p: &Path| rel_of(&p.to_string_lossy(), &base);
    let line = |(f, roots): &(PathBuf, Vec<PathBuf>)| format!("{}  ← {}", rel(f), roots.iter().map(|r| rel(r)).collect::<Vec<_>>().join(", "));
    let first = shared.first()?;
    let mut s = String::from("# rust-analyzer can't emit SCIP for a file shared by multiple crates:\n");
    for e in shared.iter().take(8) {
        s.push_str(&format!("#   {}\n", line(e)));
    }
    s.push_str("# fix: keep one includer (or move the shared helpers into a crate / one test target),\n");
    s.push_str("#   or update rust-analyzer (`rustup update`): 1.93 panics on this layout, 1.94.1 baked it fine");
    Some((s, format!("{} file(s) shared by multiple crates: {}", shared.len(), line(first))))
}
