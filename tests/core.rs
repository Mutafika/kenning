//! Integration tests for kenning's core semantic queries.
//!
//! Runs the real binary against a generated fixture crate (syn layer only — no
//! rust-analyzer, so these are deterministic and CI-independent). Each test writes
//! a small crate with a *known* call structure to a temp dir, `index`es it to an
//! explicit db (explicit db = no auto-index), then asserts on query stdout.
//!
//! The fixture is designed so the syn resolver's behavior is unambiguous:
//! - bare unique-name calls (`target()`) resolve to *confirmed* edges,
//! - a deliberate `dup` name collision (free fn vs `C::dup` method) exercises the
//!   confirmed/candidate split and proves the confirmed set has no false positives.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

const CARGO_TOML: &str = "[package]\nname = \"fix\"\nversion = \"0.0.0\"\nedition = \"2021\"\n";

// Unique-name chain (target←mid←top←reaches_target, caller in other.rs) gives clean
// confirmed edges; the dup/C::dup collision is the candidate / no-false-positive case.
const LIB_RS: &str = r#"mod other;

pub fn target() {}

pub fn mid() {
    target();
    target();
}

pub fn top() {
    mid();
}

pub trait T {
    fn m(&self);
}

pub struct A;
pub struct B;

impl T for A {
    fn m(&self) {}
}
impl T for B {
    fn m(&self) {}
}

// name collision: free fn `dup` vs method `C::dup`
pub fn dup() {}

pub struct C;
impl C {
    pub fn dup(&self) {}
    // `self.f()` は受け手 = 今いる impl の型と分かるので syn 層でも確定できる唯一の method 形。
    pub fn dup_via_self(&self) {
        self.dup();
    }
}

pub fn ambig_user() {
    let c = C;
    c.dup(); // method call -> C::dup, must NOT be counted as a caller of the free dup
}

#[cfg(test)]
#[test]
fn reaches_target() {
    top(); // a test that transitively reaches top -> mid -> target
}
"#;

const OTHER_RS: &str = "pub fn caller() {\n    target();\n}\n";

static SEQ: AtomicU32 = AtomicU32::new(0);

/// 過去の実行が残した fixture を消す (test は失敗・中断で後片付けを飛ばすので、放置すると溜まる —
/// 実際に 534 個 / 6.6GB 溜めてディスクを埋めた)。名前の pid が生きていない物だけ = 並列実行中の他の
/// test process には触らない。1 process で 1 回。
fn sweep_dead_fixtures() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let me = std::process::id().to_string();
        let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) else { return };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(pid) = name.strip_prefix("kenning_it_").and_then(|r| r.split('_').next()) else { continue };
            let alive = pid == me || Command::new("kill").args(["-0", pid]).stderr(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
            if !alive {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    });
}

/// Fresh, empty temp dir with a `src/` subdir.
fn tmp() -> PathBuf {
    sweep_dead_fixtures();
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let p = std::env::temp_dir().join(format!("kenning_it_{}_{}", std::process::id(), n));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(p.join("src")).unwrap();
    p
}

fn write_fixture(dir: &Path, other: &str) {
    std::fs::write(dir.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(dir.join("src/lib.rs"), LIB_RS).unwrap();
    std::fs::write(dir.join("src/other.rs"), other).unwrap();
}

fn kenning() -> Command {
    Command::new(env!("CARGO_BIN_EXE_kenning"))
}

fn index(dir: &Path, db: &Path) {
    let out = kenning()
        .args(["index", dir.to_str().unwrap(), db.to_str().unwrap()])
        .output()
        .expect("spawn kenning index");
    assert!(out.status.success(), "index failed: {out:?}");
}

fn query(cmd: &[&str], db: &Path) -> String {
    query_in(cmd, db, None)
}

/// `query` の cwd 指定版 — `outline .` のように cwd 基準で解決する引数を試すため。
fn query_in(cmd: &[&str], db: &Path, cwd: Option<&Path>) -> String {
    let mut c = kenning();
    c.args(cmd).args(["--db", db.to_str().unwrap()]).env("KENNING_NO_STALE", "1");
    if let Some(d) = cwd {
        c.current_dir(d);
    }
    let out = c.output().expect("spawn kenning query");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A fixture indexed with the default `other.rs`; returns (dir, db). `dir` is kept
/// alive by the caller so the temp files outlive the queries.
fn indexed() -> (PathBuf, PathBuf) {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    let db = dir.join("k.db");
    index(&dir, &db);
    (dir, db)
}

#[test]
fn callers_confirmed_set_is_exact() {
    let (_dir, db) = indexed();
    let out = query(&["callers", "target"], &db);
    // mid() calls target() twice, caller() (other file) once — all bare unique-name.
    assert!(out.contains("3 確実 callers"), "expected 3 confirmed:\n{out}");
    assert_eq!(out.matches("in mid").count(), 2, "mid calls target twice:\n{out}");
    assert!(out.contains("in caller"), "cross-file caller must be confirmed:\n{out}");
    assert!(out.contains("候補未確定 0"), "no candidates expected:\n{out}");
}

#[test]
fn callers_has_no_false_positive_across_name_collision() {
    // `ambig_user` calls the METHOD `c.dup()`. The free fn `dup` must get 0 confirmed
    // callers — the method call must not be misattributed to it.
    let (_dir, db) = indexed();
    let out = query(&["callers", "dup"], &db);
    assert!(out.contains("2 型が定義"), "dup should have two same-named defs:\n{out}");
    let method = out.lines().find(|l| l.contains("C::dup")).expect("C::dup row");
    let free = out
        .lines()
        .find(|l| l.contains("::dup") && !l.contains("C::dup"))
        .expect("free dup row");
    assert!(method.trim_start().starts_with('1'), "method owns the caller: {method}");
    assert!(free.trim_start().starts_with('0'), "free dup must be 0: {free}");
}

#[test]
fn edges_reports_cross_file_call() {
    let (_dir, db) = indexed();
    let out = query(&["edges"], &db);
    let line = out
        .lines()
        .find(|l| l.contains("other.rs") && l.contains("lib.rs"))
        .unwrap_or_else(|| panic!("no cross-file edge:\n{out}"));
    let cols: Vec<&str> = line.split('\t').collect();
    assert_eq!(cols.len(), 3, "edges row is from\\tto\\tcount: {line}");
    assert!(cols[0].ends_with("other.rs"), "caller side: {line}");
    assert!(cols[1].ends_with("lib.rs"), "callee side: {line}");
    assert_eq!(cols[2], "1", "one confirmed cross-file call: {line}");
}

#[test]
fn impact_reaches_full_transitive_set() {
    let (_dir, db) = indexed();
    let out = query(&["impact", "target"], &db);
    // target <- mid, caller <- top <- reaches_target
    assert!(out.contains("4 sym"), "expected 4 transitive callers:\n{out}");
    for sym in ["mid", "caller", "top", "reaches_target"] {
        assert!(out.contains(sym), "impact missing {sym}:\n{out}");
    }
}

#[test]
fn impls_lists_both_implementors() {
    let (_dir, db) = indexed();
    let out = query(&["impls", "T"], &db);
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| l.ends_with("\tA") || l.ends_with("\tB"))
        .collect();
    assert_eq!(rows.len(), 2, "T has exactly two implementors:\n{out}");
    assert!(rows.iter().any(|l| l.ends_with("\tA")), "impl A missing:\n{out}");
    assert!(rows.iter().any(|l| l.ends_with("\tB")), "impl B missing:\n{out}");
}

#[test]
fn faceted_search_is_kind_and_vis_scoped() {
    let (_dir, db) = indexed();
    let out = query(&["search", "kind:fn", "vis:pub"], &db);
    for f in ["target", "mid", "top", "dup", "ambig_user", "caller"] {
        assert!(out.contains(&format!("pub fn {f}")), "missing pub fn {f}:\n{out}");
    }
    // reaches_target is private; C::dup is a method — both excluded by the facets.
    assert!(!out.contains("reaches_target"), "private fn leaked into kind:fn vis:pub:\n{out}");
    assert!(!out.contains("C::dup"), "method leaked into kind:fn:\n{out}");
}

#[test]
fn tests_command_finds_reaching_test() {
    let (_dir, db) = indexed();
    let out = query(&["tests", "target"], &db);
    // impact ∩ is_test = the `reaches_target` test.
    assert!(out.contains("reaches_target"), "reaching test missing:\n{out}");
    assert!(out.contains("#test"), "test marker missing:\n{out}");
}

/// `text` covers every text file, not just `.rs` — with per-format context annotation,
/// and with generated / binary / oversize files deliberately left out of the index.
#[test]
fn text_searches_non_rust_files_with_context() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    std::fs::write(dir.join("DESIGN.md"), "# Design\nintro\n## Storage\nuses zzmarker here\n").unwrap();
    std::fs::write(dir.join("conf.toml"), "[server]\nendpoint = \"zzmarker\"\n").unwrap();
    std::fs::write(dir.join("ci.yml"), "jobs:\n  build:\n    run: zzmarker\n").unwrap();
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\necho zzmarker\n").unwrap(); // 拡張子なしでも通る
    std::fs::write(dir.join("Cargo.lock"), "# zzmarker\n").unwrap(); // 生成物 = 索引しない
    std::fs::write(dir.join("blob.bin"), b"zzmarker\0\0".as_slice()).unwrap(); // binary = 索引しない
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["text", "zzmarker"], &db);
    // .md は見出し階層、.toml は [table]、.yml はキーパスが container になる。
    assert!(out.contains("DESIGN.md:4\tuses zzmarker here\t(in Design > Storage)"), "md:\n{out}");
    assert!(out.contains("conf.toml:2"), "toml missing:\n{out}");
    assert!(out.contains("(in server)"), "toml table container missing:\n{out}");
    assert!(out.contains("ci.yml:3"), "yaml missing:\n{out}");
    assert!(out.contains("(in jobs.build.run)"), "yaml key path missing:\n{out}");
    // 抽出関数を持たない形式は注釈なしで通る (索引はされる)
    assert!(out.contains("run.sh:2"), "extension-less text file missing:\n{out}");
    // 除外されるべきもの
    assert!(!out.contains("Cargo.lock"), "generated lock must not be indexed:\n{out}");
    assert!(!out.contains("blob.bin"), "binary must not be indexed:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 非 Rust の編集も増分 update が拾う (拾えないと md が黙って古いまま残る)。
#[test]
fn incremental_update_picks_up_markdown_edit() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    std::fs::write(dir.join("NOTES.md"), "# Notes\nbefore\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    assert!(query(&["text", "zzafter"], &db).contains("index 済みファイルに無い"));

    std::fs::write(dir.join("NOTES.md"), "# Notes\n## Later\nzzafter\n").unwrap();
    let upd = kenning()
        .args(["update", dir.to_str().unwrap(), db.to_str().unwrap()])
        .output()
        .expect("spawn kenning update");
    assert!(upd.status.success(), "update failed: {upd:?}");

    let out = query(&["text", "zzafter"], &db);
    assert!(out.contains("NOTES.md:3\tzzafter\t(in Notes > Later)"), "md edit not picked up:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn incremental_update_matches_full_reindex() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    let db_upd = dir.join("upd.db");
    index(&dir, &db_upd);

    // Add a new confirmed caller of target, then incrementally update the existing db.
    let other2 = "pub fn caller() {\n    target();\n}\n\npub fn extra() {\n    target();\n}\n";
    std::fs::write(dir.join("src/other.rs"), other2).unwrap();
    let upd = kenning()
        .args(["update", dir.to_str().unwrap(), db_upd.to_str().unwrap()])
        .output()
        .expect("spawn kenning update");
    assert!(upd.status.success(), "update failed: {upd:?}");

    // A fresh full index of the same (modified) tree.
    let db_full = dir.join("full.db");
    index(&dir, &db_full);

    // Same dir => same paths => byte-identical query output iff update == full reindex.
    let a = query(&["callers", "target"], &db_upd);
    let b = query(&["callers", "target"], &db_full);
    assert_eq!(a, b, "incremental update diverged from a full reindex");
    assert!(a.contains("in extra"), "update did not pick up the new caller:\n{a}");
}

/// `index`/`update` は探索系と同じ `--db P` を受ける。以前は位置引数扱いで `--db` という名の
/// db (+ 256MB の .oplog) を cwd に作っていた。知らない flag は db 名に化ける前に拒否。
#[test]
fn index_and_update_accept_db_flag_and_never_create_literal_db_file() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    let db = dir.join("flag.db");
    let out = kenning()
        .args(["index", ".", "--db", db.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "index --db failed: {out:?}");
    assert!(db.exists(), "--db の値に index されていない");
    assert!(!dir.join("--db").exists() && !dir.join("--db.oplog").exists(), "`--db` という名の db を作った");
    assert!(query(&["def", "target"], &db).contains("src/lib.rs:3\t"));

    let out = kenning()
        .args(["update", ".", "--db", db.to_str().unwrap()])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "update --db failed: {out:?}");
    assert!(!dir.join("--db").exists(), "update が `--db` db を作った");

    let out = kenning().args(["index", ".", "--bogus"]).current_dir(&dir).output().unwrap();
    assert!(!out.status.success(), "不明な flag を db 名として受理した");
    assert!(!dir.join("--bogus").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// `cache ls|prune`: 自動導出 db の棚卸し。repo が消えた index だけ (sidecar ごと) 掃除する。
/// HOME を差し替えて本物の ~/.cache/kenning には触らない。
#[test]
fn cache_ls_and_prune_drop_index_whose_repo_is_gone() {
    let home = tmp();
    let cache = home.join(".cache/kenning");
    let repo = tmp();
    write_fixture(&repo, OTHER_RS);
    let run = |args: &[&str], cwd: &Path| {
        let out = kenning()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env_remove("KENNING_DB")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?} failed: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // 儀式ゼロ: repo 内で聞くだけで $HOME/.cache/kenning に auto-index される
    assert!(run(&["def", "target"], &repo).contains("src/lib.rs:3\t"));
    let ls = run(&["cache", "ls"], &home);
    assert!(ls.contains("\tok"), "auto-index された db が ok で載る: {ls}");
    assert!(ls.contains(&*std::fs::canonicalize(&repo).unwrap().to_string_lossy()), "root 列: {ls}");
    assert!(ls.contains("# 1 index"), "{ls}");

    std::fs::remove_dir_all(&repo).unwrap();
    let dry = run(&["cache", "prune", "--dry-run"], &home);
    assert!(dry.contains("削除予定 (root missing"), "{dry}");
    assert_eq!(std::fs::read_dir(&cache).unwrap().filter_map(|e| e.ok()).filter(|e| e.path().extension().is_some_and(|x| x == "db")).count(), 1, "dry-run で消えた");
    let pr = run(&["cache", "prune"], &home);
    assert!(pr.contains("# 1 index") && pr.contains("を回収 (残 0)"), "{pr}");
    let left: Vec<String> = std::fs::read_dir(&cache).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert!(left.is_empty(), "sidecar が残った: {left:?}");
    assert!(run(&["cache"], &home).contains("# cache に index が無い"));
    let _ = std::fs::remove_dir_all(&home);
}

/// `across` は cache 内の db を **thread で撒いて** 開くが、出力は入力順 (db パス順) に
/// 戻す。2 repo を auto-index して、両方が載ること・順序が db パス順で決定的なことを見る。
/// (並列化前と同じ出力であることの回帰テスト — 逐次に戻しても通る。)
#[test]
fn across_lists_every_repo_in_deterministic_order() {
    let home = tmp();
    // db 名は `<repo 名>-<hash>.db` なので、repo 名で db パス順が決まる。
    let (a, b) = (home.join("aaa_repo"), home.join("zzz_repo"));
    for r in [&a, &b] {
        std::fs::create_dir_all(r.join("src")).unwrap();
        write_fixture(r, OTHER_RS);
    }
    let run = |args: &[&str], cwd: &Path| {
        let out = kenning()
            .args(args)
            .current_dir(cwd)
            .env("HOME", &home)
            .env_remove("KENNING_DB")
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?} failed: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    // 儀式ゼロ: 各 repo で一度聞くと $HOME/.cache/kenning に db が増える
    for r in [&a, &b] {
        assert!(run(&["def", "target"], r).contains("src/lib.rs:3\t"));
    }

    let out = run(&["across", "target"], &home);
    assert!(out.contains("# across \"target\" — 2 repo index を走査:"), "{out}");
    let (ia, ib) = (out.find("aaa_repo:"), out.find("zzz_repo:"));
    assert!(ia.is_some() && ib.is_some(), "両 repo が載る: {out}");
    assert!(ia < ib, "db パス順で決定的 (並列でも入力順に戻す): {out}");
    // 同じ入力なら何度回しても同じ出力 (thread の完走順に依存しない)
    assert_eq!(out, run(&["across", "target"], &home), "出力が非決定的");
    let _ = std::fs::remove_dir_all(&home);
}

/// 同名 symbol (free fn `dup` / `C::dup`) の絞り込み: container / `path:` / `crate:` / `--all`。
/// これが無いと Claude は同名に当たるたび grep + sed に戻っていた。
#[test]
fn read_disambiguates_same_named_symbols() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["read", "dup"], &db);
    assert!(out.contains("2 定義") && out.contains("--all"), "曖昧時の案内:\n{out}");
    let out = query(&["read", "dup", "C"], &db);
    assert!(out.contains("pub fn dup(&self) {}") && !out.contains("pub fn dup() {}"), "container 絞り:\n{out}");
    let out = query(&["read", "dup", "--all"], &db);
    assert!(out.contains("pub fn dup(&self) {}") && out.contains("pub fn dup() {}"), "--all で両方:\n{out}");
    let out = query(&["read", "dup", "crate:fix"], &db);
    assert!(out.contains("2 定義"), "crate は両方に当たるので曖昧のまま:\n{out}");
    let out = query(&["read", "dup", "crate:nope"], &db);
    assert!(out.contains("定義が index に無い"), "crate 不一致:\n{out}");
    let out = query(&["read", "dup", "path:lib.rs", "--all"], &db);
    assert!(out.contains("pub fn dup() {}"), "path 絞り:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `read <path>:<line>` はその行を囲む item の本体 (`grep -n "fn X"` → `sed -n` の代わり)。非 Rust は見出し配下。
#[test]
fn read_at_path_line_prints_enclosing_item_or_section() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    std::fs::write(dir.join("DESIGN.md"), "# Design\nintro\n## Storage\nuses zzmarker here\nmore\n## Next\nother section\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["read", "src/lib.rs:6"], &db); // 6 行目は mid の本体 (fixture は item 間に空行あり)
    assert!(out.contains("fn mid") && out.contains("target();") && !out.contains("fn top"), "囲む item:\n{out}");
    let out = query(&["read", "DESIGN.md:5"], &db);
    assert!(out.contains("uses zzmarker here") && out.contains("more") && !out.contains("## Next") && !out.contains("intro"), "見出し配下:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `read <file>#<見出し>` と `outline <file.md>` (CHANGELOG を awk で切る代わり)。
#[test]
fn read_markdown_section_by_heading_and_outline_lists_headings() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    std::fs::write(dir.join("DESIGN.md"), "# Design\nintro\n## Storage\nuses zzmarker here\nmore\n## Next\nother section\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["read", "DESIGN.md#storage"], &db);
    assert!(out.contains("Design > Storage") && out.contains("uses zzmarker here") && !out.contains("## Next"), "見出し配下:\n{out}");
    let out = query(&["read", "DESIGN.md#nope"], &db);
    assert!(out.contains("一致する見出しが") && out.contains("Design > Storage"), "無い時は目次:\n{out}");
    let out = query(&["outline", "DESIGN.md"], &db);
    assert!(out.contains("3 sections") && out.contains("DESIGN.md:3\tDesign > Storage"), "outline md:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `text` の複数語 OR と `-e` 正規表現 (`grep -E "a|b"` の代わり)。
#[test]
fn text_supports_or_terms_and_regex() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    std::fs::write(dir.join("DESIGN.md"), "# Design\nuses zzmarker here\n").unwrap();
    std::fs::write(dir.join("conf.toml"), "[server]\nendpoint = \"qqother\"\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["text", "zzmarker", "qqother"], &db);
    assert!(out.contains("DESIGN.md:2") && out.contains("conf.toml:2"), "OR:\n{out}");
    let out = query(&["text", "-e", "zz(marker|nothing)", "qq[a-z]+er"], &db);
    assert!(out.contains("DESIGN.md:2") && out.contains("conf.toml:2"), "regex:\n{out}");
    let out = query(&["text", "-e", "zz("], &db);
    assert!(out.contains("正規表現が不正"), "不正 regex の案内:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// grep / ls に落ちる残りの場面: `text` の `path:` 絞りと件数行、`read <path>` は outline へ、
/// `outline <dir>` は配下 file の地図 (symbol 数 / loc)。
#[test]
fn text_path_facet_count_line_and_outline_dir() {
    let dir = tmp();
    write_fixture(&dir, "pub fn caller() {\n    target(); // zzterm\n}\n");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/A.md"), "# A\nzzterm one\n").unwrap();
    std::fs::write(dir.join("docs/B.md"), "# B\nzzterm two\nzzterm three\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["text", "zzterm"], &db);
    assert!(out.contains("# 4 件 / 3 files"), "件数行:\n{out}");
    let out = query(&["text", "zzterm", "path:docs/"], &db);
    assert!(out.contains("A.md:2") && out.contains("B.md:2") && !out.contains("other.rs"), "path: 絞り:\n{out}");
    assert!(out.contains("# 3 件 / 2 files (path: docs/ の 2 files)"), "絞った件数行:\n{out}");
    let out = query(&["text", "zzterm", "path:docs/", "--limit", "1"], &db);
    assert!(out.contains("`--limit 3` で全部"), "省略の案内:\n{out}");
    // -e は大小無視だが (?-i) で区別できる
    assert!(query(&["text", "-e", "(?-i)ZZTERM"], &db).contains("index 済みファイルに無い"));
    assert!(query(&["text", "-e", "ZZTERM"], &db).contains("# 4 件"));

    // read <path> (行も見出しも無し) は outline へ。「定義が無い」+ 近い symbol 名は返さない
    let out = query(&["read", "src/lib.rs"], &db);
    assert!(out.contains("src/lib.rs : ") && out.contains("\tpub fn target"), "read <path>:\n{out}");
    assert!(!out.contains("定義が index に無い"), "{out}");

    // outline <dir>: 相対 dir は path 中の /<dir>/ 一致
    let out = query(&["outline", "src"], &db);
    assert!(out.contains("# src : 2 files"), "{out}");
    assert!(out.contains("src/lib.rs\t") && out.contains(" symbols / ") && out.contains(" loc"), "{out}");
    let out = query(&["outline", "docs"], &db);
    assert!(out.contains("# docs : 2 files") && out.contains("A.md\t2 loc"), "{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 自動 index / 自動 update は stderr を要点だけに畳む (賑やかだと Claude が `2>/dev/null` を付けて
/// ⚠ まで捨てる)。初回 = 2 行 (理由 + 完了)、編集後の query = 1 行、最新なら 0 行。`--version` も。
#[test]
fn auto_paths_keep_stderr_terse_and_version_prints() {
    let out = kenning().arg("--version").output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), format!("kenning {}", env!("CARGO_PKG_VERSION")));

    let home = tmp();
    let repo = tmp();
    write_fixture(&repo, OTHER_RS);
    let run = |args: &[&str]| {
        let out = kenning().args(args).current_dir(&repo).env("HOME", &home).env_remove("KENNING_DB").output().unwrap();
        assert!(out.status.success(), "{args:?} failed: {out:?}");
        (String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    };
    let (o, e) = run(&["def", "target"]);
    assert!(o.contains("src/lib.rs:3\t"), "{o}");
    let lines: Vec<&str> = e.lines().collect();
    assert_eq!(lines.len(), 2, "初回 auto index の stderr は 2 行:\n{e}");
    assert!(lines[1].starts_with("# full index 完了: "), "{e}");
    let (_, e) = run(&["def", "target"]);
    assert!(e.is_empty(), "最新なら stderr は空:\n{e}");

    std::thread::sleep(std::time::Duration::from_millis(1100)); // built_at は秒粒度
    std::fs::write(repo.join("src/other.rs"), "pub fn caller() {\n    target();\n    target();\n}\n").unwrap();
    let (_, e) = run(&["def", "target"]);
    assert_eq!(e.lines().count(), 1, "編集後の auto update は 1 行:\n{e}");
    assert!(e.contains("→ 自動 update: 1 再 index"), "{e}");
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_dir_all(&repo);
}

/// ls / find に落ちていた 3 場面と、command ごとにバラついていた typo 救済。
/// (`outline .` は "file not found: ." だった / `find` は symbol 名しか見なかった /
///  近い名前の提案は `callers` にしか無かった)
#[test]
fn outline_dot_finds_by_file_name_and_typo_rescue_is_uniform() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    let db = dir.join("k.db");
    index(&dir, &db);

    // `outline .` = cwd の地図。表示名は解決後の絶対 dir (どこを数えたか曖昧にしない)
    let out = query_in(&["outline", "."], &db, Some(&dir));
    assert!(out.contains(" files (詳細は outline <path>)"), "outline .:\n{out}");
    assert!(out.contains("src/lib.rs\t") && out.contains("src/other.rs\t"), "outline . の中身:\n{out}");
    assert!(!out.contains("# . :"), "表示名は解決後の絶対 dir:\n{out}");

    // find はファイル名 (basename) でも引ける = `find . -name '*other*'` 相当
    let out = query(&["find", "other"], &db);
    assert!(out.contains("# 1 files  [file name ~ \"other\"]"), "find のファイル名一致:\n{out}");
    assert!(out.contains("src/other.rs\t") && out.contains(" loc"), "file 行:\n{out}");
    // symbol 側の結果は消えていない (両方出す)
    assert!(query(&["find", "targ"], &db).contains("pub fn target"), "symbol 一致は従来通り");

    // def / path も callers と同じ近い名前を出す
    let out = query(&["def", "targe"], &db);
    assert!(out.contains("# 0 symbols") && out.contains("\"targe\" は無い。近い名前:") && out.contains("target"), "def の救済:\n{out}");
    let out = query(&["path", "top", "targe"], &db);
    assert!(out.contains("# 定義が index に無い: targe"), "path は欠けた名前を名指し:\n{out}");
    assert!(out.contains("近い名前:") && out.contains("target"), "path の救済:\n{out}");
    // 在る名前が他 facet で 0 件になった時は「無い」と言わない
    let out = query(&["search", "name:target", "kind:struct"], &db);
    assert!(out.contains("# 0 symbols") && !out.contains("は無い。近い名前"), "facet 0 件で嘘を言わない:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// stdout ではなく stderr を見る版 (未知 flag の警告など、データでない案内はこちら)。
fn query_err(cmd: &[&str], db: &Path) -> String {
    let out = kenning()
        .args(cmd)
        .args(["--db", db.to_str().unwrap()])
        .env("KENNING_NO_STALE", "1")
        .output()
        .expect("spawn kenning query");
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// `grep X | grep Y` (AND) と `rg -c` (file 別件数) — grep に戻る最後の 2 イディオム。
/// AND では「file に両語がある」だけでは足りない (行で AND) ことと、件数行の files が
/// 足切り通過数でなく実際に当たった file 数であることを固定する。
#[test]
fn text_and_mode_and_files_mode() {
    let dir = tmp();
    write_fixture(&dir, OTHER_RS);
    // 同じ行に両方 (AND で残る) / 別々の行に片方ずつ (file 単位では通るが行 AND では落ちる)
    std::fs::write(dir.join("both.md"), "# both\nzzalpha and zzbeta together\nzzalpha alone\n").unwrap();
    std::fs::write(dir.join("split.md"), "# split\nzzalpha here\nzzbeta there\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);

    let or = query(&["text", "zzalpha", "zzbeta"], &db);
    assert!(or.contains("# 4 件 / 2 files"), "OR は 4 行 / 2 files:\n{or}");

    let and = query(&["text", "--and", "zzalpha", "zzbeta"], &db);
    assert!(and.contains("both.md:2"), "同じ行に両語のある行は残る:\n{and}");
    assert!(!and.contains("split.md"), "行 AND を満たさない file は出ない:\n{and}");
    assert!(and.contains("# 1 件 / 1 files"), "件数は行 AND 基準:\n{and}");

    let files = query(&["text", "--files", "zzalpha"], &db);
    assert!(files.contains("both.md\t2 件"), "件数降順の file 表:\n{files}");
    assert!(files.contains("split.md\t1 件"), "{files}");
    let (b, s) = (files.find("both.md").unwrap(), files.find("split.md").unwrap());
    assert!(b < s, "件数の多い file が先:\n{files}");
    assert!(files.contains("# 3 件 / 2 files"), "{files}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 未知 flag を黙って検索語 / 名前に格下げしない (typo した flag が「効いたように見える」事故)。
#[test]
fn unknown_flag_is_reported_not_silently_searched() {
    let (_dir, db) = indexed();
    let err = query_err(&["text", "--file", "target"], &db);
    assert!(err.contains("未知 flag") && err.contains("--file"), "警告が出ない:\n{err}");
    let out = query(&["text", "--file", "target"], &db);
    assert!(!out.contains("--file"), "flag を語として検索している:\n{out}");
    assert!(out.contains("lib.rs"), "残りの語での検索は続く:\n{out}");
}

/// `search` の path: facet (text / read と同じ意味)。無いと「この file の pub fn」が 2 手になる。
#[test]
fn search_filters_by_path() {
    let (_dir, db) = indexed();
    let out = query(&["search", "kind:fn", "path:other.rs"], &db);
    assert!(out.contains("other.rs") && out.contains("caller"), "other.rs の fn:\n{out}");
    assert!(!out.contains("lib.rs"), "path 外が混じっている:\n{out}");
    assert!(out.contains("path=other.rs"), "適用済み facet を自己申告する:\n{out}");
}

/// `impls` の空振りは 2 種類ある: 名前が無い / 型は在るが trait 実装が無い。
/// 混ぜると実在する型を調べに来た側が grep に戻るので、別の文言 + 次の一手を出す。
#[test]
fn impls_distinguishes_unknown_name_from_type_without_trait_impl() {
    let (_dir, db) = indexed();
    let out = query(&["impls", "C"], &db); // C は inherent impl (C::dup) だけ持つ型
    assert!(out.contains("定義済みだが trait 実装が無い"), "型は在ると言うべき:\n{out}");
    assert!(out.contains("search container:C"), "次の一手を出す:\n{out}");
    let out = query(&["impls", "Cc"], &db);
    assert!(out.contains("定義が index に無い"), "名前が無い側:\n{out}");
    assert!(out.contains("近い名前"), "typo 救済に繋ぐ:\n{out}");
    let out = query(&["impls", "T"], &db); // trait T は A/B が実装
    assert!(out.contains("実装する型") && out.contains("A") && out.contains("B"), "正常系:\n{out}");
}

/// SCIP 無しの index で `refs` が 0 を返す時、それが「未参照」ではなく「未計測」だと言う。
#[test]
fn refs_without_scip_says_it_is_not_a_zero_reference_answer() {
    let (_dir, db) = indexed(); // syn 層のみ = ref table は空
    let out = query(&["refs", "target"], &db);
    assert!(out.contains("refs は常に 0"), "0 を答えとして返さない:\n{out}");
    assert!(out.contains("kenning bake") && out.contains("callers target"), "次の一手:\n{out}");
}

/// 書き方違い (snake↔camel / 大小) と 1 文字 typo を CLI 経由で救えるか。
#[test]
fn typo_rescue_covers_case_style_and_one_char_slips() {
    let (_dir, db) = indexed();
    for q in ["AmbigUser", "ambiguser", "ambig_usr"] {
        let out = query(&["def", q], &db);
        assert!(out.contains("ambig_user"), "{q} から ambig_user を提案できない:\n{out}");
    }
}

/// `read <path>:<from>-<to>` — `sed -n 'A,Bp'` の代わり。単なる切り出しでは grep 系と同じなので、
/// **範囲が跨ぐ定義 / 見出しを頭に列挙する**ところまでが仕様 (今どこを読んでいるかが 1 行で分かる)。
#[test]
fn read_line_range_lists_what_the_range_covers() {
    let (dir, db) = indexed();
    std::fs::write(dir.join("NOTES.md"), "# Top\nintro\n## Sub\nbody\n").unwrap();
    index(&dir, &db);

    // lib.rs の target/mid/top は連続して定義されている → 範囲は複数 item を跨ぐ
    let out = query(&["read", "src/lib.rs:3-11"], &db);
    let head = out.lines().next().unwrap();
    assert!(head.contains("src/lib.rs:3") && head.contains("9 行 (3-11)"), "見出し行:\n{out}");
    for f in ["target", "mid", "top"] {
        assert!(head.contains(f), "跨ぐ定義 {f} が列挙されていない:\n{head}");
    }
    assert!(out.contains("\n    3\t") || out.contains("  3\t"), "行番号付きで出す:\n{out}");

    // 非 Rust は見出しを列挙 (範囲の頭を囲む物 + 範囲内で始まる物)
    let out = query(&["read", "NOTES.md:2-4"], &db);
    let head = out.lines().next().unwrap();
    assert!(head.contains("Top") && head.contains("Sub"), "見出しの列挙:\n{head}");

    // file の外は「0 行」ではなく明示する
    let out = query(&["read", "src/lib.rs:9000-9010"], &db);
    assert!(out.contains("file の外"), "{out}");

    // 既存の単一行形は変わらない (回帰)
    let out = query(&["read", "src/lib.rs:6"], &db);
    assert!(out.contains("pub fn mid") || out.contains("fn mid"), "path:line は囲む item のまま:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// kenning の出力は修飾名 (`C::dup`) を出す。**それをそのまま次のコマンドに渡せる**こと。
/// 以前は `def C::dup` が「無い。近い名前: C::dup」と自己矛盾していた (出力→入力の往復が閉じていなかった)。
#[test]
fn qualified_name_from_output_is_accepted_as_input() {
    let (_dir, db) = indexed();
    // 出力側: callers dup の要約は C::dup という表記を出す
    let listed = query(&["callers", "dup"], &db);
    assert!(listed.contains("C::dup"), "出力が修飾名を出す前提:\n{listed}");

    // 入力側: その表記を各コマンドが受ける
    for cmd in [
        vec!["def", "C::dup"],
        vec!["read", "C::dup"],
        vec!["callers", "C::dup"],
        vec!["callees", "C::dup"],
        vec!["impact", "C::dup"],
        vec!["refs", "C::dup"],
    ] {
        let out = query(&cmd, &db);
        assert!(!out.contains("定義が index に無い"), "{cmd:?} が修飾名を受けない:\n{out}");
        assert!(!out.contains("0 symbols"), "{cmd:?} が修飾名を受けない:\n{out}");
    }

    // 同名の別 symbol (free fn dup) を巻き込まない
    let out = query(&["callers", "C::dup"], &db);
    assert!(out.contains("C::dup") && out.contains("1 確実"), "method 側だけを見る:\n{out}");

    // 明示 container が優先 (矛盾する指定なら 0 件で正しい)
    let out = query(&["read", "C::dup", "C"], &db);
    assert!(out.contains("fn dup"), "明示 container と一致:\n{out}");
}

/// `callees` / `def` も read / callers と同じ絞り込み (`path:` / `crate:` / container) を受ける。
/// 以前 `callees X path:S` は `path:S` を container と取り違えて「定義が無い」、`def X path:S` は黙って無視していた。
/// 同名が残る時は「同名 N 定義を分けて出す」と先に言う (混ぜて 1 つの関数の呼び先と読まれないように)。
#[test]
fn callees_and_def_honor_narrowing() {
    let dir = tmp();
    write_fixture(&dir, "pub fn dup() {\n    helper();\n}\nfn helper() {}\n");
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["callees", "dup"], &db);
    assert!(out.contains("同名 3 定義"), "同名を分けて出すと言う:\n{out}");
    let out = query(&["callees", "dup", "path:other.rs"], &db);
    assert!(out.contains("helper") && !out.contains("lib.rs") && !out.contains("同名"), "path 絞り:\n{out}");
    let out = query(&["callees", "dup", "crate:nope"], &db);
    assert!(out.contains("定義が index に無い (crate=nope)"), "絞った結果が 0 なら絞り込みごと言う:\n{out}");

    let out = query(&["def", "dup", "path:other.rs"], &db);
    assert!(out.contains("1 symbols") && out.contains("other.rs") && !out.contains("lib.rs"), "def の path 絞り:\n{out}");
    let out = query(&["def", "dup", "C"], &db);
    assert!(out.contains("1 symbols") && out.contains("C::dup"), "def の container 絞り:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// macro 引数の中の呼び出し (`println!("{}", target())`) も call graph に入ること。
/// ここが抜けていると「確実 + 候補 + 別 sym の合計 = 全 caller」という kenning の中心的な約束が破れ、
/// 実際に使われている関数が `callers 0` に見える (この repo の append_src / role_name がそうだった)。
#[test]
fn calls_inside_macro_arguments_are_recorded() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"macro_rules! twice { ($e:expr) => { { $e; $e } }; }

pub fn caller() {
    target();
}

pub fn shouty() {
    println!("{}", format!("{:?}", target()));
    assert!(true, "{}", mid_ok());
}

fn mid_ok() -> u8 { 0 }

pub fn dsl_user() {
    twice!(target()); // parse できない DSL macro でも壊れないこと
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["callers", "target"], &db);
    assert!(out.contains("in shouty"), "println! 引数内の呼び出しが落ちている:\n{out}");
    let out = query(&["callers", "mid_ok"], &db);
    assert!(out.contains("in shouty"), "assert! 引数内の呼び出しが落ちている:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 「誰からも呼ばれていない定義」を 1 手で。callers=確実 / namecalls=名前一致 の 2 facet で、
/// 「消せる候補 (両方 0)」と「未解決の呼び出しがあるかも (確実だけ 0)」を撃ち分ける。
#[test]
fn search_filters_by_incoming_call_counts() {
    let (_dir, db) = indexed();
    let unused = query(&["search", "kind:fn", "callers:0", "namecalls:0", "test:0", "--limit", "50"], &db);
    assert!(!unused.contains("\ttarget") && !unused.contains(" target "), "呼ばれている関数が未使用に出る:\n{unused}");
    assert!(unused.contains("ambig_user"), "誰も呼ばない関数が出ない:\n{unused}");
    assert!(unused.contains("呼ばれていない = この index の中での話"), "解釈の注意書きが要る:\n{unused}");

    // callers:1 は確実 caller がちょうど 1 本の定義 (mid は top から 1 本)
    let one = query(&["search", "name:mid", "callers:1"], &db);
    assert!(one.contains("fn mid"), "確実 caller 数での等値絞りが効かない:\n{one}");
    let zero = query(&["search", "name:mid", "callers:0"], &db);
    assert!(zero.contains("# 0 symbols"), "{zero}");
}

/// 関数を **値として渡す** 形 (`map(f)` / `any(f)`) も「使われている」証拠として残ること。
/// ここが抜けていると、高階関数経由でしか使われない定義が「誰からも呼ばれていない」に見える
/// (kenning 自身の sig_text / is_cfg_test がそう見えていた — rustc は dead_code を出していないのに)。
/// ただし裸の path は局所変数と区別が付かないので **確実 (callee_sym) には昇格させない**。
#[test]
fn function_passed_as_value_counts_as_usage_but_not_as_a_confirmed_call() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn caller() {
    target();
}

fn helper(x: u8) -> u8 { x }

pub fn hof_user(v: Vec<u8>) -> Vec<u8> {
    let local = 1u8;
    consume(local); // 局所変数は記録しない (定義表に無い名前)
    v.into_iter().map(helper).collect()
}

fn consume(_x: u8) {}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["callers", "helper"], &db);
    assert!(out.contains("0 確実 callers"), "値渡しを確実に昇格させてはいけない:\n{out}");
    assert!(out.contains("[value-ref]") && out.contains("in hof_user"), "値渡しが候補に出ない:\n{out}");

    // 「誰からも呼ばれていない」からは外れる = 消す判断を誤らせない
    let unused = query(&["search", "kind:fn", "callers:0", "namecalls:0", "test:0", "--limit", "50"], &db);
    assert!(!unused.contains("helper"), "値渡しで使われている定義が未使用に出る:\n{unused}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 関数参照は `map(f)` だけでなく `&f` / `Some(f)` / `as` / 構造体フィールド / macro 引数にも現れる。
/// 「呼び出しの引数」限定だと 4 形とも取り逃し、生きている定義が「未使用」に見えた。
/// 一方で裸の path は局所変数と区別が付かないので、**同じボディで束縛された名前は候補にしない**
/// (これが無いと common な名前で候補が桁違いに膨れる: enchudb で 14,738 → 4,003 行)。
#[test]
fn function_references_in_every_value_position_count_as_usage() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn by_ref(x: u8) -> u8 { x }
pub fn by_option(x: u8) -> u8 { x }
pub fn by_field(x: u8) -> u8 { x }
pub fn by_vec(x: u8) -> u8 { x }
pub fn shadowed(x: u8) -> u8 { x }

struct Holder { f: fn(u8) -> u8 }

// 仮引数が同名の関数を隠す形。引数名は signature 側にあるので body だけ見ると漏れ、
// 「関数への参照」に化ける (tokio の `registration` で候補が汚れていた実例)。
pub fn shadow_param(by_vec: u8) -> u8 { by_vec }

pub fn uses(v: Vec<u8>) -> usize {
    let _ = v.iter().map(&by_ref).count();
    let _ = Some(by_option as fn(u8) -> u8);
    let h = Holder { f: by_field };
    let fs: Vec<fn(u8) -> u8> = vec![by_vec];
    let shadowed = 1u8; // 同名の局所変数 → 参照として数えない
    let _ = shadowed;
    (h.f)(1) as usize + fs.len()
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    // ここは call graph 層の検査なので、字句照合 (--no-lexical で切る) は挟まない
    let unused = query(&["search", "kind:fn", "callers:0", "namecalls:0", "--no-lexical", "--limit", "50"], &db);
    for f in ["by_ref", "by_option", "by_field", "by_vec"] {
        assert!(!unused.contains(f), "{f} の参照を取り逃している:\n{unused}");
    }
    assert!(unused.contains("shadowed"), "局所変数の束縛を参照として数えている:\n{unused}");

    // 仮引数 `by_vec` を返すだけの関数が、同名の fn への参照として記録されないこと
    let refs_to_by_vec = query(&["callers", "by_vec"], &db);
    assert!(refs_to_by_vec.contains("in uses"), "本物の参照 (vec![by_vec]) は残る:\n{refs_to_by_vec}");
    assert!(!refs_to_by_vec.contains("shadow_param"), "仮引数を関数参照と誤認している:\n{refs_to_by_vec}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// trait 実装の method は呼び出し側に名前が出ない (trait 経由 / dyn) ので `callers:0` が構造的に真。
/// `traitimpl:0` で外せないと、未使用判定が Display::fmt 等で埋まって使い物にならない
/// (enchudb では 73 件中 15 件がこれ)。
#[test]
fn trait_impl_methods_are_distinguishable() {
    let (_dir, db) = indexed();
    let ti = query(&["search", "kind:method", "traitimpl:1"], &db);
    assert!(ti.contains("A::m") && ti.contains("B::m"), "trait 実装の method:\n{ti}");
    assert!(!ti.contains("C::dup"), "inherent impl が混ざっている:\n{ti}");

    let inherent = query(&["search", "kind:method", "traitimpl:0"], &db);
    assert!(inherent.contains("C::dup") && !inherent.contains("A::m"), "inherent 側:\n{inherent}");
}

/// 「変えると壊れる範囲」は **見落としの方が危険** なので、`impact` / `tests` は既定で
/// 値渡し参照 (`map(f)`、名前が一意な時だけ) も辿る。確定 edge だけ見たいときは `--confirmed-only`。
/// これが無いと、高階関数越しにしか呼ばれない定義の impact が 0 sym と出る
/// (kenning 自身の `sig_text` が 0 と出ていた。実際は 45 sym)。
#[test]
fn impact_follows_value_references_unless_confirmed_only() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn leaf(x: u8) -> u8 { x }

pub fn middle(v: Vec<u8>) -> usize {
    v.into_iter().map(leaf).count()
}

pub fn topmost(v: Vec<u8>) -> usize {
    middle(v)
}

#[cfg(test)]
mod t {
    #[test]
    fn reaches_leaf() {
        super::topmost(vec![]);
    }
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["impact", "leaf"], &db);
    assert!(out.contains("middle") && out.contains("topmost"), "値参照の先が影響範囲に出ない:\n{out}");
    assert!(out.contains("値渡し参照"), "値参照経由だと明示する:\n{out}");

    let strict = query(&["impact", "leaf", "--confirmed-only"], &db);
    assert!(strict.contains("0 sym"), "--confirmed-only は確定 edge だけ:\n{strict}");

    let t = query(&["tests", "leaf"], &db);
    assert!(t.contains("reaches_leaf"), "値参照越しのテストが見つからない:\n{t}");
    let t2 = query(&["tests", "leaf", "--confirmed-only"], &db);
    assert!(t2.contains("届くテストなし"), "--confirmed-only:\n{t2}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// `super::f()` / `crate::f()` は **型修飾ではなく module 相対の道順**。型として突き合わせると必ず外れ、
/// テスト module の定番の呼び方が 1 件も確定しなかった (候補どまり) — `tests <name>` が効かない原因。
#[test]
fn module_relative_paths_resolve_like_bare_calls() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn only_here() -> u8 { 1 }

#[cfg(test)]
mod t {
    #[test]
    fn via_super() { super::only_here(); }
    #[test]
    fn via_crate() { crate::only_here(); }
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["callers", "only_here"], &db);
    assert!(out.contains("2 確実 callers"), "super:: / crate:: が確定していない:\n{out}");
    let t = query(&["tests", "only_here"], &db);
    assert!(t.contains("via_super") && t.contains("via_crate"), "テスト特定に届かない:\n{t}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// bake の有無は **精度そのもの** なので stats が明示すること。
/// (再 index で SCIP facts が落ちても気付けず、impact が静かに縮む事故を防ぐ)
#[test]
fn stats_states_whether_the_index_is_baked() {
    let (_dir, db) = indexed(); // syn 層のみ
    let out = query(&["stats"], &db);
    assert!(out.contains("bake: 無し"), "bake の有無が出ない:\n{out}");
    assert!(out.contains("kenning bake"), "次の一手が無い:\n{out}");
}

/// trait 実装の method で `callers` が 0 件になるのは「未使用」ではなく **trait 経由で呼ばれる** から。
/// 黙って 0 を返すと、呼び出し側を grep で探し回るか、消してよいと誤読される
/// (enchudb では 17 method がこの形で、何の説明も出ていなかった)。
#[test]
fn callers_explains_trait_impl_dead_ends() {
    let (_dir, db) = indexed(); // fixture: `impl T for A { fn m(&self) {} }`
    let out = query(&["callers", "A::m"], &db);
    assert!(out.contains("trait 実装"), "trait 経由である説明が無い:\n{out}");
    assert!(out.contains("0 件 ≠ 未使用"), "0 の意味を言っていない:\n{out}");
    assert!(out.contains("impls T"), "兄弟実装への導線が無い:\n{out}");

    // inherent impl の method では出ない (誤った説明を足さない)
    let plain = query(&["callers", "C::dup"], &db);
    assert!(!plain.contains("trait 実装"), "inherent impl に trait の説明が出ている:\n{plain}");
}

/// item 直下のマクロ (`criterion_group!(benches, bench_tie, …)`) の中の参照も拾うこと。
/// 関数の外なので caller となる sym が無く、CallCollector は関数本体しか歩かないため丸ごと
/// 抜けていた — enchudb の bench 関数 9 本が「誰からも呼ばれていない」に出ていた。
#[test]
fn references_inside_item_level_macros_are_recorded() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn registered_a() {}
pub fn registered_b() {}

macro_rules! group { ($name:ident, $($f:path),* $(,)?) => {}; }

group!(all, registered_a, registered_b,);
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["callers", "registered_a"], &db);
    assert!(out.contains("value-ref"), "item 直下マクロの参照が拾えていない:\n{out}");
    assert!(out.contains("item 直下"), "caller 無しの行に説明が無い:\n{out}");

    let unused = query(&["search", "kind:fn", "callers:0", "namecalls:0", "--limit", "50"], &db);
    for f in ["registered_a", "registered_b"] {
        assert!(!unused.contains(f), "{f} が未使用扱いのまま:\n{unused}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 未使用候補は **消す判断**に使われるので、call 表だけを根拠にしない。
/// 呼び出しの形をしていない使われ方 (DSL マクロに名前だけ渡す等) は字句照合で拾って候補から落とす。
/// ただし **コメント / 文字列 / 束縛 (field 宣言・局所変数) は使用に数えない** — doc コメントは API 名を
/// 普通に挙げるので、数えると本当に dead な物が毎回生き残る (enchudb の `Engine::vocab` が実例)。
#[test]
fn unused_candidates_are_cross_checked_lexically() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn named_in_code() {}
pub fn only_in_comment() {}
pub fn truly_unused() {}

macro_rules! reg { ($($t:tt)*) => {}; }

// only_in_comment はコメントで名前が出るだけ (使用ではない)
reg! {
    #[test]
    fn generated(v in 0..3u8) {
        let _f = named_in_code; // 呼び出しの形をしていない = 字句照合でしか拾えない
    }
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["search", "kind:fn", "callers:0", "namecalls:0"], &db);
    assert!(!out.contains("named_in_code"), "コード上の出現を見落としている:\n{out}");
    assert!(out.contains("only_in_comment"), "コメントを使用と数えている:\n{out}");
    assert!(out.contains("truly_unused"), "本当に未使用の定義まで落としている:\n{out}");
    assert!(out.contains("字句照合で"), "除外したことを言っていない:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 未使用は「入次数 0」ではなく **到達不能** の問題。dead な塊が鎖で繋がっていると入次数は 0 に
/// ならず、1 段しか見ない `callers:0` は「消す → 再実行 → また 1 件」を繰り返す
/// (enchudb で実際に 3 周した。3 件 → 2 件 → 手で 1 件)。`reachable:0` は保守規則の反復で
/// 鎖ごと 1 パスで出す。root = pub / #[test] / trait 実装 / main / item 直下マクロからの参照。
#[test]
fn reachable_zero_finds_dead_chains_in_one_pass() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn live_entry() {
    live_helper();
}

fn live_helper() {}

fn dead_head() {
    dead_tail();
}

fn dead_tail() {}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["search", "reachable:0"], &db);
    assert!(out.contains("dead_head") && out.contains("dead_tail"), "鎖の両方が出ない:\n{out}");
    assert!(!out.contains("live_helper") && !out.contains("live_entry"), "生きている物が混ざる:\n{out}");

    // 入次数だけ見ると、鎖の下流 (dead_tail) は caller が居るので出てこない
    let in_deg = query(&["search", "kind:fn", "callers:0", "namecalls:0", "--no-lexical"], &db);
    assert!(!in_deg.contains("dead_tail"), "この前提が崩れたらテストの意味が無い:\n{in_deg}");
}

/// `proptest! { #[test] fn f(x in strategy) { helper(); } }` のように、**中身が item 定義とその本体**の
/// マクロは式としても item としても parse できない (DSL 構文)。呼び出しは call graph に載らないので、
/// 字句照合が最後の砦になる。dead 判定済みの本体はスキップするが、**マクロ invocation の中は
/// スキップしない** — ここを間違えると生きているヘルパを「未使用」と言ってしまう。
#[test]
fn helpers_used_only_inside_unparseable_macros_are_not_reported_unused() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"fn helper_used_in_dsl(x: u8) -> u8 { x }
fn helper_really_unused(x: u8) -> u8 { x }

macro_rules! dsl_block { ($($t:tt)*) => {}; }

dsl_block! {
    #[test]
    fn generated(v in 0..10u8) {
        let _ = helper_used_in_dsl(v);
    }
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["search", "reachable:0"], &db);
    assert!(!out.contains("helper_used_in_dsl"), "DSL マクロ内の使用を見落としている:\n{out}");
    assert!(out.contains("helper_really_unused"), "本当に未使用の物まで落としている:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// parse できない DSL マクロ (`proptest!` 等) の中の呼び出しを **字句走査**で拾い、
/// `[macro-token]` の候補として記録する。確定 edge には昇格させないので「確実 = 誤りなし」は保つ。
/// これが入ると、字句照合を切っても未使用判定が正しくなる (最後の砦に頼らない)。
#[test]
fn calls_inside_unparseable_macros_are_recorded_as_candidates() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"fn helper_used_in_dsl(x: u8) -> u8 { x }
fn helper_really_unused(x: u8) -> u8 { x }

macro_rules! dsl_block { ($($t:tt)*) => {}; }

dsl_block! {
    #[test]
    fn generated(v in 0..10u8) {
        let _ = helper_used_in_dsl(v);
    }
}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);

    let out = query(&["callers", "helper_used_in_dsl"], &db);
    assert!(out.contains("[macro-token]"), "DSL マクロ内の呼び出しが候補に出ない:\n{out}");
    assert!(out.contains("0 確実 callers"), "当て推量を確定に昇格させてはいけない:\n{out}");

    // 字句照合を切っても未使用にならない = call graph 側で解決できている
    let strict = query(&["search", "reachable:0", "--no-lexical"], &db);
    assert!(!strict.contains("helper_used_in_dsl"), "字句照合なしだと誤検出する:\n{strict}");
    assert!(strict.contains("helper_really_unused"), "本当に未使用の物が出ない:\n{strict}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// cfg で分岐した同名定義 (`#[cfg(unix)] fn f` / `#[cfg(not(unix))] fn f`) は、syn が両方読む一方で
/// SCIP は活性側しか知らないため、非活性側に流入が付かず必ず「未使用」に見える。索引が cfg-blind で
/// あることの副作用なので候補から外す (enchudb で 5 件の偽陽性)。
#[test]
fn cfg_split_twins_are_not_reported_unused() {
    let dir = tmp();
    write_fixture(
        &dir,
        r#"pub fn entry() {
    twin();
}

#[cfg(unix)]
fn twin() {}

#[cfg(not(unix))]
fn twin() {}

fn lonely_cfg() {}
"#,
    );
    let db = dir.join("k.db");
    index(&dir, &db);
    let out = query(&["search", "reachable:0", "--no-lexical"], &db);
    assert!(!out.contains("twin"), "cfg 双子を未使用と誤判定している:\n{out}");
    assert!(out.contains("lonely_cfg"), "同名でない物まで除外している:\n{out}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// crate 名修飾 (`fix::target()`) は **module 相対の道順**であって外部呼び出しではない。
/// ここを落とすと多 crate repo の workspace 内呼び出しが丸ごと「外部」に消える
/// (実際 kenning 自身の main.rs → lib の dispatch が 20 本まるごと消えていた)。
#[test]
fn crate_name_qualifier_resolves_like_a_local_call() {
    let d = tmp();
    write_fixture(&d, "pub fn via_crate() {\n    fix::target();\n    crate::target();\n}\n");
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "target"], &db);
    let confirmed = out.split("候補").next().unwrap_or("");
    assert_eq!(
        confirmed.matches("in via_crate").count(),
        2,
        "crate 修飾 / crate:: の両方が確実 caller になるはず: {out}"
    );
}

/// 解決率の分母から **外部呼び出し (std / 依存 crate)** を外す。外部は index に定義が無く
/// 構造的に解決不能なので、混ぜると「率が低い = 精度が低い」に読めてしまう。
#[test]
fn stats_separates_external_calls_from_the_resolve_rate() {
    let d = tmp();
    write_fixture(&d, "pub fn uses_std() {\n    let v: Vec<u8> = Vec::new();\n    let _ = v.len();\n    target();\n}\n");
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["stats"], &db);
    let ext: usize = out
        .split("external=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    assert!(ext >= 2, "Vec::new / .len() が外部として数えられていない: {out}");
    let line = out.lines().find(|l| l.contains("repo 内")).unwrap_or_else(|| panic!("repo 内の行が無い: {out}"));
    assert!(line.contains("外部/std") && line.contains("確定率"), "内訳行の形が違う: {line}");
    // 外部を除いた分母は全 call-site より必ず小さい (= 分母から外れている証拠)
    let local: usize = line.split("repo 内 ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).unwrap();
    let total: usize = out.split(" / ").nth(2).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).unwrap();
    assert!(local + ext == total, "repo 内 {local} + 外部 {ext} が call-site 総数 {total} に一致しない");
}

/// 受け手の型が分からない method 呼び (`d.ping()`) を名前一致だけで確定すると、std/dep の
/// 同名 method (`.next()` / `.len()`) を自前の定義の caller として並べてしまう。
/// **確定するのは `self.f()` と、受け手の型がソースに書いてある物だけ**、他は候補どまり —
/// これが「確実 = 誤りなし」の境界。ここでの `d` は添字 `v[0]` の値で、式の型推論なしには分からない。
#[test]
fn only_self_receiver_method_calls_are_confirmed() {
    let d = tmp();
    write_fixture(
        &d,
        "pub struct D;\n\
         impl D {\n\
             pub fn ping(&self) {}\n\
             pub fn go(&self) { self.ping(); }\n\
         }\n\
         pub fn outside(v: Vec<D>) { v[0].ping(); }\n",
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "ping"], &db);
    let (confirmed, candidates) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    assert!(confirmed.contains("in D::go"), "self.ping() は確定するはず:\n{out}");
    assert!(!confirmed.contains("in outside"), "受け手不明の d.ping() を確定してはいけない:\n{out}");
    assert!(candidates.contains("in outside"), "確定しない呼び出しは候補に残すはず:\n{out}");
    assert!(candidates.contains("[method-name]"), "候補の理由をラベルで出すはず:\n{out}");
}

/// 出力に出た修飾名 (`D::ping`) はそのまま引数に渡せる約束 — `callers ping D` と同じ候補が出ること。
/// call 側は bare name で持っているので、修飾名のまま名前一致を引くと候補が黙って 0 件になっていた
/// (tokio の `Sleep::reset` で「候補未確定 0」と出て、`tests` の取りこぼしを隠した)。
#[test]
fn qualified_name_shows_same_candidates_as_container_form() {
    let d = tmp();
    write_fixture(
        &d,
        "pub struct D;\n\
         impl D {\n\
             pub fn ping(&self) {}\n\
             pub fn go(&self) { self.ping(); }\n\
         }\n\
         pub fn outside(v: Vec<D>) { v[0].ping(); }\n",
    );
    let db = d.join("k.db");
    index(&d, &db);
    let qualified = query(&["callers", "D::ping"], &db);
    let container = query(&["callers", "ping", "D"], &db);
    assert!(qualified.contains("in outside"), "修飾名でも候補が出るはず:\n{qualified}");
    assert_eq!(qualified, container, "修飾名と container 指定で結果が違う");
}

/// 同名の自由関数は container が空なので `callers <name> <container>` では絞れない。`read` と同じ
/// `path:` / `crate:` で絞れて、要約表の「絞る」例も path: を出す (例が `(例: )` と空だった)。
#[test]
fn callers_narrows_same_named_free_fns_by_path() {
    let d = tmp();
    write_fixture(&d, "pub fn target() {}\npub fn use_local() { target(); }\n");
    let db = d.join("k.db");
    index(&d, &db);
    let summary = query(&["callers", "target"], &db);
    assert!(summary.contains("2 型が定義"), "同名 2 定義の要約表のはず:\n{summary}");
    assert!(summary.contains("path:"), "自由関数の絞り方に path: を出すはず:\n{summary}");
    let narrowed = query(&["callers", "target", "path:other.rs"], &db);
    assert!(narrowed.contains("in use_local"), "path: で絞った定義の caller が出るはず:\n{narrowed}");
    assert!(!narrowed.contains("in mid"), "lib.rs の target の caller が混ざった:\n{narrowed}");
}

/// `mod::f()` の module は file の置き場所からも決まる (`src/time/inner.rs` の f は time の配下)。子 module に
/// 実体を置いて `pub use` で出す形 (tokio の `time::sleep`) を確定し、同じ名前の module が 2 つある
/// (`task` と `runtime::task`) 時は道順を決められないので確定しない (tokio の yield_now で RA と照合した形)。
#[test]
fn module_qualified_calls_use_file_location() {
    let d = tmp();
    write_fixture(&d, "pub fn user() { time::later(); task::yield_now(); }\n");
    for (p, body) in [
        ("src/time/inner.rs", "pub fn later() {}\n"),
        ("src/task/mod.rs", "pub fn yield_now() {}\n"),
        ("src/runtime/task/mod.rs", "pub fn yield_now() {}\n"),
    ] {
        std::fs::create_dir_all(d.join(p).parent().unwrap()).unwrap();
        std::fs::write(d.join(p), body).unwrap();
    }
    let db = d.join("k.db");
    index(&d, &db);
    let later = query(&["callers", "later"], &db);
    assert!(later.contains("1 確実 callers"), "子 module の実体に確定しない:\n{later}");
    let y = query(&["callers", "yield_now", "path:src/task/mod.rs"], &db);
    assert!(y.contains("0 確実 callers"), "同名 module が 2 つあるのに確定した:\n{y}");
}

/// 別の同名 sym に確定した呼び出しも、行き先の定義ごとの件数を出す (件数だけだと agent は grep で
/// 数え直していた)。ここでは method `C::dup` に絞った時、free fn `dup` 側に確定した呼び出しがそこに出る。
#[test]
fn callers_lists_where_other_same_named_calls_resolved() {
    let d = tmp();
    write_fixture(&d, "pub fn free_user() { crate::dup(); }\n");
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "C::dup"], &db);
    assert!(out.contains("件は別の同名 sym に確定"), "別の同名に確定した分の内訳が無い:\n{out}");
    let row = out.lines().find(|l| l.contains("lib.rs:") && l.trim_start().starts_with(|c: char| c.is_ascii_digit()) && l.contains("dup")).unwrap_or_else(|| panic!("行き先の定義の行が無い:\n{out}"));
    assert!(!row.contains("C::dup"), "行き先は free fn の dup のはず: {row}");
}

/// `#[cfg(..)] mod x;` の中の定義 (条件付き) も、外の実体と入れ替わる枝が無ければ外から確定する
/// (tokio の `cfg_rt!` の中の JoinSet — loom の test から spawn が 1 件も確定しなかった)。同じ名前を cfg 付きの
/// `use` で repo の外 (std) から持ってくる枝があれば入れ替わり得るので確定しない (parking_lot::Condvar ↔ std)。
#[test]
fn cond_defs_are_confirmed_unless_an_external_cfg_alternative_exists() {
    let d = tmp();
    write_fixture(
        &d,
        "use crate::rt::JoinSet;\nuse crate::pl::Condvar;\n\
         pub fn user() { let s = JoinSet::new(); s.spawn(); let c = Condvar::new(); c.notify_all(); }\n",
    );
    std::fs::write(
        d.join("src/lib.rs"),
        "#[cfg(feature = \"rt\")]\nmod rt;\n#[cfg(feature = \"pl\")]\nmod pl;\n\
         #[cfg(not(feature = \"pl\"))]\nuse std::sync::Condvar;\nmod other;\n",
    )
    .unwrap();
    std::fs::write(d.join("src/rt.rs"), "pub struct JoinSet;\nimpl JoinSet {\n    pub fn new() -> Self { JoinSet }\n    pub fn spawn(&self) {}\n}\n").unwrap();
    std::fs::write(d.join("src/pl.rs"), "pub struct Condvar;\nimpl Condvar {\n    pub fn new() -> Self { Condvar }\n    pub fn notify_all(&self) {}\n}\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let spawn = query(&["callers", "JoinSet::spawn"], &db);
    assert!(spawn.contains("1 確実 callers"), "入れ替わる相手の無い条件付きの定義に確定しない:\n{spawn}");
    let notify = query(&["callers", "Condvar::notify_all"], &db);
    assert!(notify.contains("0 確実 callers"), "std と入れ替わり得る定義に確定した:\n{notify}");
}

// ── changes: 前回 snapshot からの意味的な差分 ──

fn update(dir: &Path, db: &Path) {
    let out = kenning().args(["update", dir.to_str().unwrap(), db.to_str().unwrap()]).output().expect("spawn kenning update");
    assert!(out.status.success(), "update failed: {out:?}");
}

/// 1 つの crate を書き換えて update → changes を 1 手で。`other.rs` だけ差し替える。
fn edit_and_changes(dir: &Path, db: &Path, other: &str, cursor: &str) -> String {
    std::fs::write(dir.join("src/other.rs"), other).unwrap();
    update(dir, db);
    query(&["changes", "--cursor", cursor], db)
}

const CHG_BASE: &str = "pub fn entry() {\n    helper(1);\n}\n\nfn helper(x: u32) {\n    leaf();\n}\n\nfn leaf() {}\n";

fn changes_fixture() -> (PathBuf, PathBuf) {
    let dir = tmp();
    write_fixture(&dir, CHG_BASE);
    let db = dir.join("k.db");
    index(&dir, &db);
    let first = query(&["changes", "--cursor", "c"], &db);
    assert!(first.contains("baseline を作った"), "初回は baseline を作るはず:\n{first}");
    (dir, db)
}

/// 何も変えなければ差分は 0 — full 再 index を挟んでも (key は eid ではなく名前なので) 0。
#[test]
fn changes_is_empty_without_edits_even_across_full_reindex() {
    let (dir, db) = changes_fixture();
    let out = query(&["changes", "--cursor", "c"], &db);
    assert!(out.contains("broken 0 / sig 0 / dead 0 / revived 0 / callers 0"), "無変更で差分が出た:\n{out}");
    let _ = std::fs::remove_dir_all(&db);
    index(&dir, &db);
    let out = query(&["changes", "--cursor", "c"], &db);
    assert!(out.contains("broken 0 / sig 0 / dead 0 / revived 0 / callers 0"), "再 index で偽の差分が出た:\n{out}");
}

#[test]
fn changes_reports_signature_change_with_caller_count() {
    let (dir, db) = changes_fixture();
    let out = edit_and_changes(&dir, &db, &CHG_BASE.replace("fn helper(x: u32)", "fn helper(x: u32, y: bool)"), "c");
    assert!(out.contains("\tsig\thelper: fn helper(x: u32) → fn helper(x: u32, y: bool)  (callers 1)"), "sig 変更が出ない:\n{out}");
}

/// 定義を消したのに呼び出しが残る = 壊れた参照。位置は**残っている呼び出し**を指す (直す場所)。
#[test]
fn changes_reports_removed_definition_with_remaining_calls() {
    let (dir, db) = changes_fixture();
    let out = edit_and_changes(&dir, &db, &CHG_BASE.replace("fn leaf() {}\n", ""), "c");
    assert!(out.contains("other.rs:6\tbroken\tleaf の定義が消えたが、呼び出しが 1 件残る"), "壊れた参照が出ない:\n{out}");
}

/// 唯一の呼び元を切ると鎖ごと dead、戻すと revived。足しただけで誰も呼ばない物は「繋ぎ忘れ」。
#[test]
fn changes_tracks_dead_revived_and_unwired_additions() {
    let (dir, db) = changes_fixture();
    let cut = CHG_BASE.replace("    helper(1);\n", "");
    let out = edit_and_changes(&dir, &db, &cut, "c");
    assert!(out.contains("\tdead\thelper が live root から届かなくなった"), "鎖の頭が dead にならない:\n{out}");
    assert!(out.contains("\tdead\tleaf が live root から届かなくなった"), "鎖の下流が dead にならない:\n{out}");
    // 0 になった物は既定で出す (重複定義に呼び出しを奪われた等の兆候)
    assert!(out.contains("\tcallers\thelper: callers 1 → 0 (確定の呼び元が無くなった)"), "0 になった callers が出ない:\n{out}");

    let out = edit_and_changes(&dir, &db, CHG_BASE, "c");
    assert!(out.contains("\trevived\thelper") && out.contains("\trevived\tleaf"), "戻したのに revived が出ない:\n{out}");
    // 増えただけの物は件数だけ (--all で行も)
    let since = out.lines().find_map(|l| l.strip_prefix("# changes since ")).unwrap().split(':').next().unwrap().to_string();
    assert!(out.contains("# callers の増減 ") && !out.contains("\tcallers\thelper: callers 0 → 1"), "増えただけの callers が既定で出た:\n{out}");
    let all = query(&["changes", "--since", &since, "--all"], &db);
    assert!(all.contains("\tcallers\thelper: callers 0 → 1"), "--all で呼び元数の増加が出ない:\n{all}");

    let out = edit_and_changes(&dir, &db, &format!("{CHG_BASE}\nfn orphan() {{}}\n"), "c");
    assert!(out.contains("\tdead\torphan を足したが live root から届かない"), "繋ぎ忘れが出ない:\n{out}");
}

/// cursor は呼び手ごとに独立 (別 session が進めても自分の起点は動かない)。--since は token 直指定。
#[test]
fn changes_cursors_are_independent_and_since_takes_a_token() {
    let (dir, db) = changes_fixture();
    let base = query(&["changes", "--cursor", "other"], &db);
    let token = base.lines().find_map(|l| l.strip_prefix("# token: ")).unwrap().to_string();
    let out = edit_and_changes(&dir, &db, &CHG_BASE.replace("fn helper(x: u32)", "fn helper(x: u64)"), "c");
    assert!(out.contains("\tsig\t"), "cursor c で差分が出ない:\n{out}");
    // cursor c は進んだが other は据え置き → other から見ても同じ差分がまだ見える
    let other = query(&["changes", "--cursor", "other"], &db);
    assert!(other.contains("\tsig\t"), "別 cursor の起点が動いてしまった:\n{other}");
    let since = query(&["changes", "--since", &token], &db);
    assert!(since.contains("\tsig\t"), "--since <token> で差分が出ない:\n{since}");
}

#[test]
fn changes_json_is_one_object_per_line_and_ends_with_token() {
    let (dir, db) = changes_fixture();
    std::fs::write(dir.join("src/other.rs"), CHG_BASE.replace("fn helper(x: u32)", "fn helper(x: u8)")).unwrap();
    update(&dir, &db);
    let out = query(&["changes", "--cursor", "c", "--json"], &db);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.iter().all(|l| l.starts_with('{') && l.ends_with('}')), "NDJSON でない:\n{out}");
    assert!(lines.iter().any(|l| l.contains("\"kind\":\"sig\"") && l.contains("\"old\":\"fn helper(x: u32)\"")), "sig が無い:\n{out}");
    assert!(lines.last().unwrap().starts_with("{\"kind\":\"token\""), "最後の行が token でない:\n{out}");
}

/// 消えた token を「差分なし」と読ませない (警告して新しい baseline に)。
#[test]
fn changes_unknown_token_warns_instead_of_reporting_no_changes() {
    let (_dir, db) = changes_fixture();
    let out = kenning().args(["changes", "--since", "0000000000000000beef", "--db", db.to_str().unwrap()]).env("KENNING_NO_STALE", "1").output().unwrap();
    let (so, se) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(se.contains("snapshot 0000000000000000beef が無い"), "警告が出ない:\n{se}");
    assert!(so.contains("baseline を作った") && !so.contains("changes since"), "差分なしと誤読される出力:\n{so}");
}

/// 放置された cursor (終わった session の hook が作った物) は TTL で捨てられ、snapshot を pin し続けない。
#[test]
fn changes_drops_idle_cursors() {
    let (_dir, db) = changes_fixture();
    let dir = PathBuf::from(format!("{}.changes", db.display()));
    let stale = dir.join("cursor-old-session");
    std::fs::write(&stale, "0000000000000000dead").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options().write(true).open(&stale).unwrap().set_modified(old).unwrap();
    query(&["changes", "--cursor", "c"], &db);
    assert!(!stale.exists(), "30 日放置の cursor が残っている");
    assert!(dir.join("cursor-c").exists(), "使用中の cursor まで消えた");
}

fn git_in(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

/// `--since <git ref>` は状態を持たない: commit していない作業の差分 (= git diff の意味版)。
/// db は repo の外に置く (repo 内だと tracked でない db が作業ツリー側にだけ居て非対称になる)。
#[test]
fn changes_since_git_ref_diffs_uncommitted_work_without_state() {
    let dir = tmp();
    write_fixture(&dir, CHG_BASE);
    git_in(&dir, &["init", "-q"]);
    git_in(&dir, &["add", "-A"]);
    git_in(&dir, &["commit", "-qm", "base"]);
    let db = tmp().join("k.db");
    index(&dir, &db);
    let clean = query(&["changes", "--since", "HEAD"], &db);
    assert!(clean.contains("broken 0 / sig 0 / dead 0 / revived 0 / callers 0"), "無変更で差分:\n{clean}");
    assert!(!clean.contains("# token:"), "git ref 起点は状態を持たない (token を出さない):\n{clean}");

    std::fs::write(dir.join("src/other.rs"), CHG_BASE.replace("fn helper(x: u32)", "fn helper(x: i64)").replace("fn leaf() {}\n", "")).unwrap();
    update(&dir, &db);
    let out = query(&["changes", "--since", "HEAD"], &db);
    assert!(out.contains("\tsig\thelper: fn helper(x: u32) → fn helper(x: i64)"), "sig が出ない:\n{out}");
    assert!(out.contains("\tbroken\tleaf の定義が消えた"), "broken が出ない:\n{out}");
    assert!(out.contains(&format!("{}/src/other.rs:", dir.display())), "位置が作業ツリーの path でない:\n{out}");

    let bad = kenning().args(["changes", "--since", "no-such-ref", "--db", db.to_str().unwrap()]).env("KENNING_NO_STALE", "1").output().unwrap();
    assert!(!bad.status.success() && String::from_utf8_lossy(&bad.stderr).contains("git ref でもない"), "解けない ref を黙って通した: {bad:?}");
}

/// 同名の free fn が複数 file にある時 (tests/*.rs の `tmp()` など)、file が 1 つ増えただけで
/// 別 file の同名同士を突き合わせてはいけない (enchudb の作業ツリーで偽の sig 3 件を出した)。
#[test]
fn changes_does_not_pair_same_named_fns_across_files_when_a_file_is_added() {
    let dir = tmp();
    write_fixture(&dir, "pub fn a_entry() { tmp(); }\nfn tmp() {}\n");
    std::fs::write(dir.join("src/zeta.rs"), "pub fn z_entry() { tmp(1); }\nfn tmp(x: u8) {}\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    query(&["changes", "--cursor", "c"], &db);
    // path 順で間に入る file を足す (通し番号で区別していると、ここで zeta の tmp とずれる)
    std::fs::write(dir.join("src/mid.rs"), "pub fn m_entry() { tmp(\"s\"); }\nfn tmp(s: &str) {}\n").unwrap();
    update(&dir, &db);
    let out = query(&["changes", "--cursor", "c"], &db);
    assert!(out.contains("sig 0"), "file を足しただけで偽の sig が出た:\n{out}");
}

/// 旧形式の snapshot (key の振り方が違う) は読まない — 読むと key がずれて偽の差分になる。
#[test]
fn changes_ignores_snapshots_of_an_older_format() {
    let (_dir, db) = changes_fixture();
    let dir = PathBuf::from(format!("{}.changes", db.display()));
    let token = std::fs::read_to_string(dir.join("cursor-c")).unwrap();
    let snap = dir.join(format!("{}.tsv", token.trim()));
    let body = std::fs::read_to_string(&snap).unwrap();
    std::fs::write(&snap, body.lines().skip(1).collect::<Vec<_>>().join("\n")).unwrap(); // header 無し = 旧形式
    let out = kenning().args(["changes", "--cursor", "c", "--db", db.to_str().unwrap()]).env("KENNING_NO_STALE", "1").output().unwrap();
    assert!(String::from_utf8_lossy(&out.stderr).contains("旧形式"), "旧形式を黙って読んだ: {out:?}");
}

/// 同名の定義を別 file に足すと (DRY 違反の重複)、元の定義の呼び出しが曖昧になって確定を失う。
/// key が「一意 → path 付き」に変わっても同じ定義として突き合わせ、callers → 0 と原因を出す
/// (kenning 自身の `json_str` 重複がこの形。path で区別する修正の直後はここが見えなくなっていた)。
#[test]
fn changes_flags_a_duplicate_definition_that_steals_resolution() {
    let dir = tmp();
    // 呼び出し (other.rs) と定義 (a.rs) が別 file。同じ file の定義なら重複があってもそちらに決まる (Rust の規則)。
    write_fixture(&dir, "pub fn entry() { enc(\"x\"); }\n");
    std::fs::write(dir.join("src/a.rs"), "pub fn enc(s: &str) -> String { s.into() }\n").unwrap();
    let db = dir.join("k.db");
    index(&dir, &db);
    query(&["changes", "--cursor", "c"], &db);
    std::fs::write(dir.join("src/dup.rs"), "pub fn other() {}\npub fn enc(s: &str) -> String { s.to_string() }\n").unwrap();
    update(&dir, &db);
    let out = query(&["changes", "--cursor", "c"], &db);
    assert!(out.contains("a.rs:1\tcallers\tenc: callers 1 → 0"), "重複で確定を失ったのが出ない:\n{out}");
    assert!(out.contains("同名の定義が増えた"), "原因 (同名の定義の追加) が出ない:\n{out}");
}

/// 起点と今の間に bake が入ると確定の精度が違う → 確定 callers 数の差はコードの変化ではないので出さない。
/// (テストでは bake できないので、起点 snapshot の bake 時刻を書き換えて「間に bake が入った」を作る)
#[test]
fn changes_suppresses_caller_diffs_across_a_bake() {
    let (dir, db) = changes_fixture();
    let cdir = PathBuf::from(format!("{}.changes", db.display()));
    let token = std::fs::read_to_string(cdir.join("cursor-c")).unwrap();
    let snap = cdir.join(format!("{}.tsv", token.trim()));
    let body = std::fs::read_to_string(&snap).unwrap();
    assert!(body.lines().nth(1) == Some("# baked_at 0"), "2 行目に bake 時刻が無い:\n{body}");
    std::fs::write(&snap, body.replacen("# baked_at 0", "# baked_at 1700000000", 1)).unwrap();
    let out = edit_and_changes(&dir, &db, &CHG_BASE.replace("    helper(1);\n", ""), "c");
    assert!(out.contains("bake が入った"), "精度が変わった注記が無い:\n{out}");
    assert!(!out.contains("\tcallers\t"), "精度差を含む callers を出した:\n{out}");
    assert!(out.contains("\tdead\thelper"), "callers 以外は出るはず:\n{out}");
}

/// 受け手の型が**ソースに書いてある**時だけ `x.f()` を確定する (res=typed)。書いていない・
/// シャドーイングで分からなくなった・型引数・戻り値が Self でない物は候補どまり (誤確定しない)。
#[test]
fn method_calls_on_receivers_with_written_types_are_confirmed() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct Db;
pub struct Other;
impl Db {
    pub fn new() -> Self { Db }
    pub fn open(p: &str) -> Result<Self, String> { let _ = p; Ok(Db) }
    pub fn peer() -> Other { Other }
    pub fn get(&self) {}
}
impl Other {
    pub fn get(&self) {}
}
pub fn by_param(db: &Db) { db.get(); }
pub fn by_let_type() { let db: Db = Db; db.get(); }
pub fn by_struct_lit() { let db = Db {}; db.get(); }
pub fn by_ctor() { let db = Db::new(); db.get(); }
pub fn by_try_ctor() -> Result<(), String> { let db = Db::open("x")?; db.get(); Ok(()) }
pub fn not_self_ret() { let o = Db::peer(); o.get(); }
pub fn shadowed(db: &Db) { let db = pick::<Other>(); db.get(); }
pub fn generic<T>(db: T) { let _ = db; }
pub fn generic_call<Db2: Fn()>(db: Db2) { db.get(); }
pub fn inner_scope(db: &Db) { { let db = make(); let _ = db; } db.get(); }
fn make() -> Db { Db }
fn pick<T: Default>() -> T { T::default() }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "get", "Db"], &db);
    let (confirmed, rest) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    for f in ["by_param", "by_let_type", "by_struct_lit", "by_ctor", "by_try_ctor", "inner_scope"] {
        assert!(confirmed.contains(&format!("in {f}\t")), "{f} の db.get() が確定しない:\n{out}");
    }
    for f in ["not_self_ret", "shadowed", "generic_call"] {
        assert!(!confirmed.contains(&format!("in {f}\t")), "{f} を Db::get に誤確定した:\n{out}");
    }
    assert!(rest.contains("in shadowed"), "シャドーイングで型が分からない呼び出しは候補に残るはず:\n{out}");
    let stats = query(&["stats"], &db);
    assert!(stats.contains("typed="), "stats に typed の内訳が出ない:\n{stats}");
}

/// 自由関数の戻り値 (`let s = sleep(..)`、実体が子 module で `pub use` した `time::later()` も) と smart pointer 越し (`Box::pin(x).as_mut().f()`) の受け手も、
/// 書いてある型から確定する (tokio の `Sleep::reset` を呼ぶテストが `tests` から漏れていた形)。ただし
/// 外から `use` した関数の戻り値と、包んだ側の method が先に当たり得る物 (Pin の `set`、trait の中の
/// `Pin::new(self)` = Pin が実装する trait の method) は確定しない。
#[test]
fn free_fn_returns_and_smart_pointer_receivers_are_confirmed() {
    let d = tmp();
    write_fixture(
        &d,
        r#"use std::sync::Arc;
use ext::make_ext;
pub mod time {
    pub struct Sleep;
    impl Sleep {
        pub fn reset(&mut self) {}
        pub fn set(&mut self) {}
    }
    pub fn sleep() -> Sleep { Sleep }
    pub mod inner { pub fn later() -> super::Sleep { super::Sleep } }
    pub use inner::later;
}
pub fn make_ext() -> time::Sleep { time::Sleep }
pub fn by_mod_fn() { let mut s = time::sleep(); s.reset(); }
pub fn by_reexport() { let mut s = time::later(); s.reset(); }
pub fn by_boxed() { let mut s = Box::pin(time::sleep()); s.as_mut().reset(); }
pub fn by_arc_chain() { Arc::new(time::sleep()).reset(); }
pub fn pin_own_name() { let mut s = Box::pin(time::sleep()); s.set(); }
pub fn external_fn() { let mut s = make_ext(); s.reset(); }
pub trait BufExt {
    fn consume(&mut self) {}
    fn via_pin(&mut self) where Self: Unpin { std::pin::Pin::new(self).consume(); }
}
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "Sleep::reset"], &db);
    let (confirmed, rest) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    for f in ["by_mod_fn", "by_reexport", "by_boxed", "by_arc_chain"] {
        assert!(confirmed.contains(&format!("in {f}\t")), "{f} の reset() が確定しない:\n{out}");
    }
    assert!(!confirmed.contains("in external_fn"), "外から use した関数の戻り値で確定した:\n{out}");
    assert!(rest.contains("in external_fn"), "確定しない呼び出しは候補に残るはず:\n{out}");
    let consume = query(&["callers", "BufExt::consume"], &db);
    assert!(!consume.split_once("候補").map_or(consume.as_str(), |x| x.0).contains("in BufExt::via_pin"), "trait の中の Pin::new(self) を trait 自身の method に確定した:\n{consume}");
    let set = query(&["callers", "Sleep::set"], &db);
    assert!(!set.split_once("候補").map_or(set.as_str(), |x| x.0).contains("in pin_own_name"), "Pin::set に先に当たる s.set() を確定した:\n{set}");
}

/// 出どころが repo の外 (`walkdir::DirEntry` / `use std::io::Error`) の型名は、同名の自前の型と
/// 結び付けない (ripgrep を RA と突き合わせて見つけた誤確定の形)。`use crate::..` や repo の module
/// から来た型は内側として確定する。
#[test]
fn external_types_with_repo_lookalike_names_are_not_confirmed() {
    let d = tmp();
    write_fixture(
        &d,
        r#"use std::io::Error;
use crate::other::Db as MyDb;
pub struct DirEntry;
impl DirEntry { pub fn file_type(&self) {} }
pub struct Db;
impl Db { pub fn get(&self) {} }
pub fn ext_param(e: &walkdir::DirEntry) { e.file_type(); }
pub fn ext_fs(e: &std::fs::DirEntry) { e.file_type(); }
pub fn own_param(e: &DirEntry) { e.file_type(); }
pub fn std_error() { let _ = Error::new(1); }
pub fn own_renamed(db: &MyDb) { db.get(); }
"#,
    );
    std::fs::write(d.join("src/errs.rs"), "pub struct Error;\nimpl Error { pub fn new(x: u8) -> Self { let _ = x; Error } }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let ft = query(&["callers", "file_type"], &db);
    let (confirmed, _) = ft.split_once("候補").unwrap_or((ft.as_str(), ""));
    assert!(confirmed.contains("in own_param"), "自前の DirEntry は確定するはず:\n{ft}");
    assert!(!confirmed.contains("in ext_param") && !confirmed.contains("in ext_fs"), "外部の DirEntry を自前に誤確定:\n{ft}");
    let new = query(&["callers", "new", "Error"], &db);
    assert!(!new.split_once("候補").map(|x| x.0).unwrap_or(&new).contains("in std_error"), "std の Error::new を自前に誤確定:\n{new}");
    let get = query(&["callers", "get", "Db"], &db);
    assert!(get.split_once("候補").map(|x| x.0).unwrap_or(&get).contains("in own_renamed"), "use crate:: で別名にした自前の型は確定するはず:\n{get}");
}

/// 修飾なしの `f()` が repo の fn でない形: 束縛したクロージャ / 関数の中で外から `use` した fn /
/// 同名の **method** しか無い (`f()` では method を呼べない)。どれも確定しない (ripgrep で RA と突き合わせた形)。
#[test]
fn bare_calls_to_closures_inner_imports_and_methods_are_not_confirmed() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct T;
impl T { pub fn cmd(&self) {} }
pub fn select(x: u8) -> u8 { x }
fn symlink() {}
pub fn uses_closure() { let select = |x: u8| x; let _ = select(1); }
pub fn uses_inner_use() { use std::os::unix::fs::symlink; let _ = symlink("a", "b"); }
pub fn calls_method_name() { cmd(); }
pub fn real_call() { let _ = select(2); symlink(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |name: &str| {
        let out = query(&["callers", name], &db);
        out.split_once("候補").map(|x| x.0.to_string()).unwrap_or(out)
    };
    let sel = confirmed("select");
    assert!(sel.contains("in real_call") && !sel.contains("in uses_closure"), "クロージャ呼びを select に誤確定:\n{sel}");
    let sym = confirmed("symlink");
    assert!(sym.contains("in real_call") && !sym.contains("in uses_inner_use"), "関数内 use の std の symlink を自前に誤確定:\n{sym}");
    let cmd = confirmed("cmd");
    assert!(!cmd.contains("in calls_method_name"), "修飾なしの cmd() を method に誤確定:\n{cmd}");
}

const CHAIN_RS: &str = r#"pub struct Db;
impl Db { pub fn get(&self) {} }
pub struct Other;
impl Other { pub fn get(&self) {} }
pub struct Svc<T> { db: Db, g: T, raw: std::fs::File }
impl<T> Svc<T> {
    pub fn cfg(&self) -> &Db { &self.db }
    pub fn open_db(&self) -> Result<Db, String> { Ok(Db) }
    pub fn io(&self) -> std::io::Error { std::io::Error::other("x") }
    pub fn by_field(&self) { self.db.get(); }
    pub fn by_ret(&self) { self.cfg().get(); }
    pub fn by_let_ret(&self) { let c = self.cfg(); c.get(); }
    pub fn by_try(&self) -> Result<(), String> { self.open_db()?.get(); Ok(()) }
    pub fn generic_field(&self) { self.g.get(); }
    pub fn external_ret(&self) { self.io().get(); }
    pub fn external_field(&self) { self.raw.get(); }
}
"#;

/// 受け手が連鎖 (`self.field` / `x.m()` / `?`) でも、1 段ずつ書いてある型をたどって確定する。
/// 型引数の field / repo の外の型を返す段が挟まると推定をやめる。
#[test]
fn method_calls_through_fields_and_return_types_are_confirmed() {
    let d = tmp();
    write_fixture(&d, CHAIN_RS);
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "get", "Db"], &db);
    let (confirmed, _) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    for f in ["by_field", "by_ret", "by_let_ret", "by_try"] {
        assert!(confirmed.contains(&format!("in Svc::{f}\t")), "{f} の連鎖が確定しない:\n{out}");
    }
    for f in ["generic_field", "external_ret", "external_field"] {
        assert!(!confirmed.contains(&format!("in Svc::{f}\t")), "{f} を Db::get に誤確定:\n{out}");
    }
}

/// field の型を書き換えたら、それを通る連鎖の解決も増分 update で付け替わる。
#[test]
fn chain_resolution_follows_field_type_changes_incrementally() {
    let d = tmp();
    write_fixture(&d, CHAIN_RS);
    let db = d.join("k.db");
    index(&d, &db);
    std::fs::write(d.join("src/other.rs"), CHAIN_RS.replace("db: Db, g: T", "db: Other, g: T").replace("-> &Db { &self.db }", "-> &Other { &self.db }")).unwrap();
    update(&d, &db);
    let on_db = query(&["callers", "get", "Db"], &db);
    let on_other = query(&["callers", "get", "Other"], &db);
    assert!(!on_db.split_once("候補").map(|x| x.0).unwrap_or(&on_db).contains("in Svc::by_field\t"), "型を変えたのに古い解決が残る:\n{on_db}");
    assert!(on_other.split_once("候補").map(|x| x.0).unwrap_or(&on_other).contains("in Svc::by_field\t"), "新しい型に付け替わらない:\n{on_other}");
}

/// 受け手が型引数 (`S: Store`、where 句も) / `dyn Store` / `impl Store` なら、`s.put()` は trait で宣言された
/// put にしか解決されない (inherent は無い)。目印の trait (Send 等) は数えず、境界が 2 つ以上なら推定しない。
#[test]
fn trait_bounded_receivers_resolve_to_the_trait_method() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub trait Store { fn put(&self); }
pub trait Other { fn put(&self); }
pub struct Mem;
impl Store for Mem { fn put(&self) {} }
pub fn by_generic<S: Store>(s: &S) { s.put(); }
pub fn by_where<S>(s: S) where S: Store { s.put(); }
pub fn by_dyn(s: &dyn Store) { s.put(); }
pub fn by_impl(s: impl Store) { s.put(); }
pub fn with_marker<S: Store + Send>(s: S) { s.put(); }
pub fn two_bounds<S: Store + Other>(s: S) { s.put(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "put", "Store"], &db);
    let (confirmed, _) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    for f in ["by_generic", "by_where", "by_dyn", "by_impl", "with_marker"] {
        assert!(confirmed.contains(&format!("in {f}\t")), "{f} の s.put() が Store::put に確定しない:\n{out}");
    }
    assert!(!confirmed.contains("in two_bounds\t"), "境界が 2 つあるのに確定した:\n{out}");
}

/// クロージャの引数の型 (item 直下のマクロ引数の中も) と `*x` の参照外しでも受け手の型をたどる。
#[test]
fn closure_params_and_derefs_carry_receiver_types() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct Cmd;
impl Cmd { pub fn arg(&mut self) -> &mut Cmd { self } pub fn run(&self) {} }
macro_rules! t { ($name:ident, $f:expr) => { pub fn $name() { let _ = $f; } }; }
t!(in_macro, |mut cmd: Cmd| { cmd.arg().run(); });
pub fn in_fn() { let f = |c: &Cmd| c.run(); let _ = f; }
pub fn by_deref(c: &&Cmd) { (**c).run(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "run", "Cmd"], &db);
    let (confirmed, _) = out.split_once("候補").unwrap_or((out.as_str(), ""));
    assert!(confirmed.contains("in in_fn\t"), "クロージャ引数の型で確定しない:\n{out}");
    assert!(confirmed.contains("in by_deref\t"), "参照外しで確定しない:\n{out}");
    assert!(confirmed.contains("in (item 直下)"), "マクロ引数のクロージャ (連鎖 arg().run()) で確定しない:\n{out}");
}

/// `impl Tr for &M` の中の `(*self).f()` は Self (= &M) ではなく M の f — 自分自身に確定しない。
#[test]
fn deref_of_self_in_pointer_impls_is_not_the_same_type() {
    let d = tmp();
    write_fixture(
        &d,
        "pub trait Tr { fn f(&self); }\n\
         impl<'a, M: Tr> Tr for &'a M { fn f(&self) { (*self).f() } }\n\
         impl<S: Tr> Tr for Box<S> { fn f(&self) { (**self).f() } }\n",
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "f"], &db);
    for line in out.lines().filter(|l| l.contains("in ")) {
        let caller_line: u32 = line.split(':').nth(1).and_then(|s| s.split('\t').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
        assert!(!(line.contains("確実") && caller_line == 2), "(*self).f() を自分に確定:\n{out}");
    }
    let confirmed = out.split("候補").next().unwrap_or("");
    assert!(!confirmed.contains("other.rs:2\tin"), "(*self).f() を &M の f 自身に確定:\n{out}");
    assert!(!confirmed.contains("other.rs:3\tin"), "(**self).f() を Box の f 自身に確定:\n{out}");
}

/// `cfg_x! { .. }` のように item / impl の中身を包むマクロの中の定義も見える。見えないままだと、
/// 同名の「見えている方」を一意と思い込んで誤確定する (tokio の cfg_rt! 等で実際に起きていた)。
/// ここでは呼び出しと同じ file のマクロの中の `twin` が呼び先 — 見えていなければ別 file の twin.rs に確定してしまう。
#[test]
fn definitions_wrapped_in_item_macros_are_indexed() {
    let d = tmp();
    write_fixture(
        &d,
        r#"macro_rules! cfg_x { ($($i:item)*) => { $($i)* } }
cfg_x! {
    pub fn hidden() {}
    pub fn twin() {}
    pub struct H;
}
impl H {
    cfg_x! { pub fn in_impl(&self) {} }
}
pub fn caller() { hidden(); twin(); }
"#,
    );
    std::fs::write(d.join("src/twin.rs"), "pub fn twin() {}\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    assert!(query(&["def", "hidden"], &db).contains("other.rs:3"), "マクロ内の fn が見えない");
    assert!(query(&["def", "in_impl"], &db).contains("H::in_impl"), "impl 内マクロの method が見えない");
    let twin = query(&["callers", "twin"], &db);
    assert!(twin.contains("2 型が定義 (同名)"), "マクロ内の twin が見えない:\n{twin}");
    assert!(query(&["callers", "twin", "path:other.rs"], &db).contains("in caller"), "同じ file (マクロ内) の twin に確定しない:\n{twin}");
    assert!(!query(&["callers", "twin", "path:twin.rs"], &db).contains("in caller"), "別 file の twin に誤確定:\n{twin}");
}

/// trait 実装の method への確定先は rust-analyzer (= bake 済み) の流儀に揃える: 具体的な型への impl を
/// method 呼びで呼べば impl の method。path 呼び `T::f()` は repo の trait なら impl の method、std の trait
/// (Default) なら repo の外。
#[test]
fn trait_impl_call_targets_follow_rust_analyzer() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub trait Tr { fn go(&self); }
pub struct A;
impl Tr for A { fn go(&self) {} }
pub struct W<T>(T);
impl<T> Tr for W<T> { fn go(&self) {} }
impl Default for A { fn default() -> Self { A } }
pub fn concrete(a: &A) { a.go(); }
pub fn generic_arg(w: &W<u8>) { w.go(); }
pub fn path_call() { let _ = A::default(); }
pub fn local_path_call(a: &A) { A::go(a); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| {
        let out = query(args, &db);
        out.split("候補").next().unwrap_or("").to_string()
    };
    assert!(confirmed(&["callers", "go", "A"]).contains("in concrete\t"), "具体的な impl は impl の method に確定するはず");
    assert!(confirmed(&["callers", "go", "W"]).contains("in generic_arg\t"), "W<T> への impl は具体的な型の impl (impl の method に確定するはず)");
    assert!(!confirmed(&["callers", "default", "A"]).contains("in path_call\t"), "std の Default::default を自前に確定");
    assert!(confirmed(&["callers", "go", "A"]).contains("in local_path_call\t"), "repo の trait の path 呼びは impl の method に確定するはず");
}

/// repo に `std` という module があっても、`use std::time::Instant` の std は標準ライブラリ。
#[test]
fn a_local_module_named_std_does_not_make_std_local() {
    let d = tmp();
    write_fixture(&d, "use std::time::Instant;\npub fn f() { let _ = Instant::now(); }\n");
    std::fs::create_dir_all(d.join("src/std")).unwrap();
    std::fs::write(d.join("src/std/mod.rs"), "pub struct Instant;\nimpl Instant { pub fn now() -> Self { Instant } }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "now", "Instant"], &db);
    assert!(!out.split("候補").next().unwrap_or("").contains("in f\t"), "std の Instant::now を自前に確定:\n{out}");
}

/// 別 crate の fn は use しなければ修飾なしでは呼べない / tests・benches 直下の file は互いに別 crate /
/// use していない prelude の型 (Vec) は std / 外の型への impl の中の self.f() は std の inherent が先 /
/// 同名 method を宣言する trait が複数あれば trait 境界からは決めない。どれも確定しない。
#[test]
fn syn_resolution_respects_crates_prelude_and_trait_ambiguity() {
    let d = tmp();
    write_fixture(
        &d,
        r#"use std::process::Child as StdChild;
pub trait Kill { fn kill(&mut self); }
impl Kill for StdChild { fn kill(&mut self) { self.kill(); } }
pub trait Buf { fn consume(&self); }
pub trait BufExt { fn consume(&self) {} }
impl<R: Buf> BufExt for R {}
pub trait Sealed { fn extend(&mut self); }
impl<T> Sealed for Vec<T> { fn extend(&mut self) {} }
pub fn by_bound<R: Buf>(r: R) { r.consume(); }
pub fn by_vec() { let mut v: Vec<u8> = Vec::new(); v.extend(); }
"#,
    );
    std::fs::create_dir_all(d.join("benches")).unwrap();
    std::fs::write(d.join("benches/a.rs"), "fn helper() {}\nfn main() { helper(); }\n").unwrap();
    std::fs::write(d.join("benches/b.rs"), "fn main() { helper(); }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| query(args, &db).split("候補").next().unwrap_or("").to_string();
    assert!(!confirmed(&["callers", "kill", "StdChild"]).contains("in StdChild::kill"), "外の型への impl の self.kill() を自分に確定");
    assert!(!confirmed(&["callers", "consume", "Buf"]).contains("in by_bound"), "同名 method の trait が 2 つあるのに確定");
    assert!(!confirmed(&["callers", "extend"]).contains("in by_vec"), "use していない Vec (std) の extend を自前に確定");
    let helper = confirmed(&["callers", "helper"]);
    assert!(helper.contains("benches/a.rs:2"), "同じ file の helper() は確定するはず:\n{helper}");
    assert!(!helper.contains("benches/b.rs"), "別ターゲット (benches/b.rs) の helper() を確定:\n{helper}");
}

/// 自作マクロの中の受け手は信じない (トークンを組み替え得る) / std の式マクロは信じる /
/// repo で定義していない型 (re-export された std の Arc) への trait 実装には確定しない /
/// `cfg_not_*!` の中の定義 (代用品) は確定先にしない。属性の cfg(not(..)) は普段の build の本体でもあり得るので
/// 除外しない (双子なら同名 2 つ = 曖昧で確定しない)。
#[test]
fn macros_reexported_types_and_negative_cfg_are_handled_conservatively() {
    let d = tmp();
    write_fixture(
        &d,
        r#"macro_rules! delegate { ($e:expr) => { $e } }
macro_rules! cfg_not_x { ($($i:item)*) => { $($i)* } }
cfg_not_x! { mod shim; }
macro_rules! cfg_x { ($($i:item)*) => { $($i)* } }
cfg_x! { mod both; }
cfg_not_x! { mod both; }
mod loom { pub use std::sync::Arc; }
use crate::loom::Arc;
use self::shim::Atomic;
pub trait Link { fn from_raw(p: u8) -> Self; }
impl Link for Arc<u8> { fn from_raw(p: u8) -> Self { Arc::new(p) } }
pub struct Cmd;
impl Cmd { pub fn run(&self) -> u8 { 1 } pub fn go(&self) { delegate!(self.run()); } }
pub fn std_macro(c: &Cmd) { println!("{}", c.run()); }
pub fn reexported() { let _ = Arc::from_raw(1); }
pub fn shimmed(a: &Atomic) { a.load(); }
#[cfg(not(unix))]
pub fn plat() {}
#[cfg(unix)]
pub fn plat() {}
pub fn calls_plat() { plat(); }
"#,
    );
    // `mod shim;` は src/other.rs の中の宣言なので、子 file は src/other/shim.rs
    std::fs::create_dir_all(d.join("src/other")).unwrap();
    std::fs::write(d.join("src/other/shim.rs"), "pub struct Atomic;\nimpl Atomic { pub fn load(&self) {} }\n").unwrap();
    // 両方の枝で宣言された module (出し分け) の中身は除外しない
    std::fs::write(d.join("src/other/both.rs"), "pub struct Real;\nimpl Real { pub fn work(&self) {} }\npub fn use_real(r: &Real) { r.work(); }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| query(args, &db).split("候補").next().unwrap_or("").to_string();
    let run = confirmed(&["callers", "run", "Cmd"]);
    assert!(run.contains("in std_macro\t"), "std の println! の中の c.run() は確定するはず:\n{run}");
    assert!(!run.contains("in Cmd::go\t"), "自作マクロの中の self.run() を確定:\n{run}");
    assert!(!confirmed(&["callers", "from_raw"]).contains("in reexported\t"), "re-export された std の Arc::from_raw を自前の trait 実装に確定");
    assert!(!confirmed(&["callers", "load", "Atomic"]).contains("in shimmed\t"), "cfg_not の代用品の Atomic::load に確定");
    assert!(confirmed(&["callers", "work", "Real"]).contains("in use_real\t"), "両方の枝で宣言された module の中身まで除外した");
    // 属性の cfg の双子は同名 2 つ = どちらにも確定しない (どちらが活性かは build 次第)
    let plat = query(&["callers", "plat"], &db);
    assert!(plat.contains("名前一致 1 件中 0 件を確定"), "cfg の双子の片方に確定:\n{plat}");
}

/// `#[path]` で別 file を指す同名の `mod imp;` は、cfg_not 側 (代用品) だけ除外する。`#[cfg(..)] mod x;` の中の
/// 定義は、その module の外からは確定しない (cfg 付きの re-export 越しに別の実体に切り替わり得る)。
#[test]
fn path_attr_modules_and_cfg_conditional_regions() {
    let d = tmp();
    write_fixture(
        &d,
        r#"macro_rules! cfg_x { ($($i:item)*) => { $($i)* } }
macro_rules! cfg_not_x { ($($i:item)*) => { $($i)* } }
cfg_x! { #[path = "native.rs"] mod imp; }
cfg_not_x! { #[path = "shim.rs"] mod imp; }
#[cfg(feature = "fast")]
mod fast;
#[cfg(not(feature = "fast"))]
use slowcrate::Fast;
pub fn use_native(n: &imp::Native) { n.get(); }
pub fn use_shim(s: &imp::Shim) { s.get(); }
pub fn use_fast(f: &fast::Fast) { f.run(); }
"#,
    );
    std::fs::write(d.join("src/native.rs"), "pub struct Native;\nimpl Native { pub fn get(&self) {} }\n").unwrap();
    std::fs::write(d.join("src/shim.rs"), "pub struct Shim;\nimpl Shim { pub fn get(&self) {} }\n").unwrap();
    std::fs::create_dir_all(d.join("src/other")).unwrap();
    std::fs::write(d.join("src/other/fast.rs"), "pub struct Fast;\nimpl Fast { pub fn run(&self) {} pub fn inner(&self) { self.run(); } }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| query(args, &db).split("候補").next().unwrap_or("").to_string();
    assert!(confirmed(&["callers", "get", "Native"]).contains("in use_native\t"), "#[path] の native 側は確定するはず");
    assert!(!confirmed(&["callers", "get", "Shim"]).contains("in use_shim\t"), "cfg_not 側 (#[path] = shim.rs) に確定");
    let run = confirmed(&["callers", "run", "Fast"]);
    assert!(!run.contains("in use_fast\t"), "cfg で外の実体 (slowcrate::Fast) と入れ替わる定義に外から確定:\n{run}");
    assert!(run.contains("in Fast::inner\t"), "同じ module の中からは確定するはず:\n{run}");
}

/// 確定先から外した候補 (cfg 付き module の中) は、残った同名の定義を「一意」にする根拠にしない
/// (外した方が本当の呼び先かもしれない — tokio で unique の誤確定が 3 → 41 件に増えた形)。
#[test]
fn excluded_candidates_do_not_make_the_rest_unique() {
    let d = tmp();
    write_fixture(&d, "#[cfg(feature = \"x\")]\nmod gated;\npub fn caller() { helper(); }\n");
    std::fs::create_dir_all(d.join("src/other")).unwrap();
    std::fs::write(d.join("src/other/gated.rs"), "pub fn helper() {}\n").unwrap();
    std::fs::write(d.join("src/elsewhere.rs"), "pub fn helper() {}\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "helper"], &db);
    assert!(out.contains("名前一致 1 件中 0 件を確定"), "外した候補があるのに残りの helper に確定:\n{out}");
}

/// 関数の中で定義した fn (index していない) を、同名の外の fn に確定しない。tests/ 直下の file は自分の
/// package の lib にとっても別 crate — 名前で use していなければ確定しない。
#[test]
fn nested_fns_and_integration_tests_do_not_borrow_outer_names() {
    let d = tmp();
    write_fixture(&d, "pub fn iter() {}\npub fn outer() { iter(); fn iter() {} }\n");
    std::fs::create_dir_all(d.join("tests")).unwrap();
    std::fs::write(d.join("tests/t.rs"), "use fix::iter;\n#[test]\nfn a() { iter(); }\n").unwrap();
    std::fs::write(d.join("tests/u.rs"), "use fix::*;\n#[test]\nfn b() { iter(); }\n").unwrap();
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["callers", "iter"], &db);
    let confirmed = out.split("候補").next().unwrap_or("");
    assert!(!confirmed.contains("in outer\t"), "関数の中の fn iter を外の iter に確定:\n{out}");
    assert!(confirmed.contains("tests/t.rs"), "名前で use した tests からは確定するはず:\n{out}");
    assert!(!confirmed.contains("tests/u.rs"), "グロブ use の tests から確定:\n{out}");
}

/// Rust の method 解決で先に当たり得る別の候補があれば確定しない: `self: Pin<&mut Self>` の `self.f()` (Pin の
/// method) / `&T` への impl / 拡張 trait が同名を宣言する trait 実装 / `Trait::f(x)` の trait 修飾 /
/// マクロの中の use (`cfg_x! { mod m { use std::cell::Cell; } }`)。field 越し (`self.inner.f()`) は届く。
#[test]
fn method_resolution_precedence_blocks_unsafe_confirmations() {
    let d = tmp();
    write_fixture(
        &d,
        r#"use std::pin::Pin;
pub struct Inner;
impl Inner { pub fn go(&self) {} }
pub struct S { inner: Inner }
impl S {
    pub fn get_mut(&mut self) -> &mut Inner { &mut self.inner }
    pub fn poll(self: Pin<&mut Self>) { let _ = self.get_mut(); self.inner.go(); }
}
pub trait Read { fn read(&mut self); }
pub struct Fd;
impl Read for Fd { fn read(&mut self) {} }
impl Read for &Fd { fn read(&mut self) {} }
pub fn by_ref(mut fd: &Fd) { fd.read(); }
pub fn ufcs(fd: &mut Fd) { Read::read(fd); }
pub trait Buf { fn consume(self: Pin<&mut Self>); }
pub trait BufExt { fn consume(&mut self) {} }
impl<R: Buf> BufExt for R {}
pub struct Rdr;
impl Buf for Rdr { fn consume(self: Pin<&mut Self>) {} }
pub fn ext(mut r: Rdr) { r.consume(); }
macro_rules! cfg_x { ($($i:item)*) => { $($i)* } }
cfg_x! { mod m { use std::cell::Cell; pub fn mk() { let _ = Cell::new(1); } } }
pub struct Cell;
impl Cell { pub fn new(x: u8) -> Self { let _ = x; Cell } }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| query(args, &db).split("候補").next().unwrap_or("").to_string();
    assert!(!confirmed(&["callers", "get_mut", "S"]).contains("in S::poll\t"), "Pin<&mut Self> の self.get_mut() を S::get_mut に確定");
    assert!(confirmed(&["callers", "go", "Inner"]).contains("in S::poll\t"), "self.inner.go() は field 越しに確定するはず");
    assert!(!confirmed(&["callers", "read", "Fd"]).contains("in by_ref\t"), "&Fd への impl があるのに Fd::read に確定");
    assert!(!confirmed(&["callers", "read"]).contains("in ufcs\t"), "Read::read(fd) を宣言に確定");
    assert!(!confirmed(&["callers", "consume", "Rdr"]).contains("in ext\t"), "拡張 trait の consume があるのに実装に確定");
    assert!(!confirmed(&["callers", "new", "Cell"]).contains("in m::mk\t") && !confirmed(&["callers", "new", "Cell"]).contains("in mk\t"), "マクロの中で use した std の Cell を自前に確定");
}

/// `use .. as 別名` で呼んだ関数・型・module は元の名前の call として数える。別名のままだと `callers target`
/// の名前一致から丸ごと漏れ、「呼び出し箇所は全部この 3 つのどれか」が嘘になる (agent が grep に戻る理由の 2 番目)。
#[test]
fn renamed_imports_are_counted_under_the_original_name() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub mod a {
    pub fn helper() {}
    pub struct Store;
    impl Store { pub fn open() -> Self { Store } }
}
use self::a::helper as hlp;
use self::a::Store as S2;
use self::a::{self as aa};
pub fn by_fn_alias() { hlp(); }
pub fn by_type_alias() { let _ = S2::open(); }
pub fn by_mod_alias() { aa::helper(); }
pub fn local_shadow() { let hlp = || (); hlp(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let h = query(&["callers", "helper"], &db);
    assert!(h.contains("in by_fn_alias") && h.contains("in by_mod_alias"), "別名で呼んだ helper が漏れた:\n{h}");
    assert!(!h.contains("in local_shadow"), "同名の束縛 (クロージャ) を別名の関数と取り違えた:\n{h}");
    let o = query(&["callers", "open", "Store"], &db);
    assert!(o.split("候補").next().unwrap_or("").contains("in by_type_alias"), "別名の型の関連関数は確定するはず:\n{o}");
}

/// `tests` / `impact` は確定 edge で届かない分を「候補経由」[c1] として同じ出力に並べる。trait 経由
/// (外部 trait の method を generic に呼ぶ包み) で呼ばれる実装に届くテストを、agent が grep で補わなくていいように。
/// `Drop::drop` は名前で呼べない (E0040) ので、`drop(x)` を候補にしない。
#[test]
fn tests_lists_tests_reached_only_through_candidate_edges() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub fn inner() {}
pub struct Timer;
impl Iterator for Timer {
    type Item = u8;
    fn next(&mut self) -> Option<u8> { inner(); None }
}
pub struct Wrap<I>(pub I);
impl<I: Iterator> Wrap<I> {
    pub fn step(&mut self) { let _ = self.0.next(); }
}
pub struct Guard;
impl Drop for Guard { fn drop(&mut self) { inner(); } }
pub struct Other;
impl Other { pub fn next(&self) {} } // 同名があると名前一意の値参照では辿れない = 候補 edge の出番
#[test]
fn through_trait() { let mut w = Wrap(Timer); w.step(); }
#[test]
fn only_drops() { let v = vec![1u8]; drop(v); }
#[test]
fn direct() { inner(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["tests", "inner"], &db);
    let row = |n: &str| out.lines().find(|l| l.contains(&format!("fn {n} "))).unwrap_or("").to_string();
    assert!(row("direct").contains("[d1]"), "確定で届くテストは [d]:\n{out}");
    assert!(row("through_trait").contains("[c1]"), "trait 経由で届くテストは [c1] に出るはず:\n{out}");
    assert!(row("only_drops").is_empty(), "drop(x) を Drop::drop の呼び出し候補にした:\n{out}");
    assert!(out.contains("候補経由"), "候補経由の件数行が無い:\n{out}");
    let imp = query(&["impact", "inner"], &db);
    assert!(imp.contains("候補経由") && imp.contains("through_trait"), "impact にも候補経由の層が出るはず:\n{imp}");
    // callers と同じ絞り込み (path: / crate:) が効く。同名の自由関数を tests / impact で選べないと grep に戻る。
    assert!(query(&["tests", "inner", "path:src/other.rs"], &db).contains("[d1]"), "tests が path: で絞れない");
    assert!(query(&["impact", "inner", "path:nope.rs"], &db).contains("path~nope.rs"), "impact の絞り込み注記が無い");
    // 切れた一覧は「そのまま打てる件数」を出す。`--limit 0` = 上限なし (0 件ではない)。
    let cut = query(&["tests", "inner", "--limit", "1"], &db);
    assert!(cut.contains("`--limit 0`") && cut.contains("切れている"), "切れた時の案内が無い:\n{cut}");
    assert_eq!(query(&["tests", "inner", "--limit", "0"], &db).lines().filter(|l| l.starts_with("  [")).count(), 2, "--limit 0 で全部出るはず");
}

/// `match` / `if let` のパターンで束縛した変数の型は、enum の variant / tuple struct の定義に書いてある
/// (`match self { E::V(x) => x.f() }`)。enum で振り分ける呼び出しを確定させる。`use E::*` の後の `V(x)` は
/// 同名の tuple struct と取り違え得るので確定しない。
#[test]
fn pattern_bindings_take_the_variant_field_type() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct Io;
impl Io { pub fn park(&self) {} }
pub struct Th;
impl Th { pub fn park(&self) {} }
pub enum Stack { On(Io), Off { th: Th } }
impl Stack {
    pub fn park(&self) {
        match self {
            Stack::On(v) => v.park(),
            Self::Off { th } => th.park(),
        }
    }
}
pub struct Wrap(pub Io);
impl Wrap { pub fn go(&self) { self.0.park(); } }
pub struct Pair(pub Io);
pub fn by_tuple(w: Pair) { let Pair(inner) = w; inner.park(); }
pub enum Other { Wrap(Th) }
pub fn glob(o: Other) { use Other::*; match o { Wrap(t) => t.park() } }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let confirmed = |args: &[&str]| query(args, &db).split("候補").next().unwrap_or("").to_string();
    let io = confirmed(&["callers", "park", "Io"]);
    assert!(io.contains("in Stack::park\t"), "E::V(x) の x を variant の field 型で確定するはず:\n{io}");
    assert!(io.contains("in Wrap::go\t"), "self.0 を tuple struct の field 型で確定するはず:\n{io}");
    assert!(io.contains("in by_tuple\t"), "let T(x) = .. の x を確定するはず:\n{io}");
    assert!(!io.contains("in glob\t"), "variant の Wrap(t) を tuple struct Wrap の field に誤確定:\n{io}");
    assert!(confirmed(&["callers", "park", "Th"]).contains("in Stack::park\t"), "Self::V {{ f }} の f を確定するはず");
}

/// `path <from> <name>` の name が同名の定義を複数持つと、最短経路は最初の 1 つで止まる。そこから確定 edge で
/// 同名の定義へ委譲が続く分 (`Context::park` → `Driver::park` → enum の振り分け先) を木で続けて出す。
#[test]
fn path_continues_through_same_named_delegation() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct Leaf;
impl Leaf { pub fn park(&self) {} }
pub struct Other;
impl Other { pub fn park(&self) {} }
pub enum Mid { A(Leaf), B(Other) }
impl Mid { pub fn park(&self) { match self { Mid::A(l) => l.park(), Mid::B(o) => o.park() } } }
pub struct Top { m: Mid }
impl Top { pub fn park(&self) { self.m.park(); } }
pub fn entry(t: &Top) { t.park(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let out = query(&["path", "entry", "park"], &db);
    let (head, tail) = out.split_once("hops").unwrap_or((&out, ""));
    assert!(head.contains("Top::park"), "最短経路は最初の park まで:\n{out}");
    assert!(tail.contains("Mid::park") && tail.contains("Leaf::park") && tail.contains("Other::park"), "同名の委譲の続きが無い:\n{out}");
    assert!(tail.contains("続き"), "続きの件数行が無い:\n{out}");
}

/// #15: vocab の hash 索引は予約全域に slot が散るので、予約 ≒ disk 消費。entity 数 × 16 で予約すると小さな
/// repo でも 26 MB (充填 0.2%) になっていた。語数の見積もりで予約し、数 MB に収まることを固定する。
#[test]
fn vocab_index_is_sized_from_the_string_estimate_not_entity_count() {
    let (_dir, db) = indexed();
    let out = query(&["stats"], &db);
    let line = out.lines().find(|l| l.starts_with("disk:")).unwrap_or_else(|| panic!("stats に disk 行が無い:\n{out}"));
    let mb: f64 = line.split("vocab 索引 ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse().ok()).unwrap_or(f64::MAX);
    assert!(mb < 4.0, "小さな fixture の vocab 索引が {mb} MB (entity 数 × 16 の予約に戻っていないか): {line}");
}

/// `static` / `const` の宣言に書いてある型は受け手の手掛かり (`static SEM: Semaphore` の `SEM.add()`)。
/// file 直下も関数の中も。束縛 (`let SEM = ..`) は外の static を隠し、同名の static が 2 つあれば決めない。
#[test]
fn statics_and_consts_give_their_declared_type() {
    let d = tmp();
    write_fixture(
        &d,
        r#"pub struct Sem;
impl Sem { pub const fn new() -> Self { Sem } pub fn add(&self) {} }
pub struct Other;
impl Other { pub fn add(&self) {} }
static TOP: Sem = Sem::new();
pub fn top() { TOP.add(); }
pub fn inner() {
    static IN: crate::other::Sem = crate::other::Sem::new();
    let f = async move { IN.add(); };
    drop(f);
}
pub fn shadow(TOP: Other) { TOP.add(); }
mod a { pub static DUP: super::Sem = super::Sem::new(); }
mod b { pub static DUP: super::Other = super::Other; }
pub fn dup() { DUP.add(); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let sem = query(&["callers", "add", "Sem"], &db);
    let confirmed = sem.split("候補").next().unwrap_or("");
    assert!(confirmed.contains("in top\t") && confirmed.contains("in inner\t"), "static の型で確定するはず:\n{sem}");
    assert!(!confirmed.contains("in shadow\t"), "引数が static を隠しているのに static の型で確定:\n{sem}");
    assert!(!confirmed.contains("in dup\t"), "同名の static が 2 つあるのに確定:\n{sem}");
}

/// crate の境目: (1) module 名が内側なのは同じ crate の中からだけ — crate `a` に `mod parking_lot` があっても、
/// crate `b` の `use parking_lot::Mutex` は外の crate (tokio の loom/std/parking_lot.rs で tokio-util の
/// `static M: Mutex` を tokio の Mutex に誤確定した形)。(2) 別の crate からは pub でない型を書けないので、同名の
/// 型が複数でも pub の 1 つに絞れる (tokio の `Semaphore` 5 つ)。
#[test]
fn crate_boundaries_limit_module_roots_and_type_visibility() {
    let d = tmp();
    std::fs::write(d.join("Cargo.toml"), "[workspace]\nmembers = [\"a\", \"b\"]\n").unwrap();
    for (k, files) in [
        (
            "a",
            vec![
                ("lib.rs", "pub mod parking_lot;\npub mod s;\nmod t;\n"),
                ("parking_lot.rs", "pub struct Mutex<T>(pub T);\nimpl<T> Mutex<T> { pub fn lock(&self) {} }\n"),
                ("s.rs", "pub struct Sem;\nimpl Sem { pub const fn new() -> Self { Sem } pub fn add(&self) {} }\n"),
                ("t.rs", "pub(crate) struct Sem;\nimpl Sem { pub(crate) fn add(&self) {} }\n"),
            ],
        ),
        (
            "b",
            vec![(
                "lib.rs",
                "use parking_lot::Mutex;\nstatic M: Mutex<()> = parking_lot::const_mutex(());\npub fn ext() { M.lock(); }\nstatic S: a::s::Sem = a::s::Sem::new();\npub fn vis() { S.add(); }\n",
            )],
        ),
    ] {
        std::fs::create_dir_all(d.join(k).join("src")).unwrap();
        std::fs::write(d.join(k).join("Cargo.toml"), format!("[package]\nname = \"{k}\"\nversion = \"0.0.0\"\nedition = \"2021\"\n")).unwrap();
        for (f, src) in files {
            std::fs::write(d.join(k).join("src").join(f), src).unwrap();
        }
    }
    let db = d.join("k.db");
    index(&d, &db);
    let lock = query(&["callers", "lock", "Mutex"], &db);
    assert!(!lock.split("候補").next().unwrap_or("").contains("in ext\t"), "別 crate の module 名 parking_lot を内側と数えて誤確定:\n{lock}");
    let add = query(&["callers", "add", "Sem", "path:a/src/s.rs"], &db);
    assert!(add.split("候補").next().unwrap_or("").contains("in vis\t"), "別 crate から書けるのは pub の Sem だけ — 確定するはず:\n{add}");
}

/// #5 / #6 / #7 / #8: `unsafe:` / `self:` facet、到達性 facet (`reaches:` / `reachable-from:`)、`uncovered`。
#[test]
fn unsafe_self_reachability_facets_and_uncovered() {
    let d = tmp();
    write_fixture(
        &d,
        r#"use std::pin::Pin;
pub struct R;
impl R {
    pub fn by_ref(&self) { unsafe { core::hint::unreachable_unchecked() } }
    pub fn by_mut(&mut self) {}
    pub fn by_val(self) {}
    pub fn by_pin(self: Pin<&mut Self>) {}
    pub fn assoc() -> Self { R }
}
pub unsafe fn raw() {}
pub fn safe_wrapper() { unsafe { raw() } }
pub fn tested_root() { safe_wrapper(); }
pub fn untested_unsafe() { unsafe { raw() } }
pub trait Run { fn run(&self); }
pub struct S;
impl Run for S { fn run(&self) { lonely(); } }
pub fn lonely() {}
pub fn via_dyn(r: &dyn Run) { r.run(); }
#[test]
fn t() { tested_root(); via_dyn(&S); }
"#,
    );
    let db = d.join("k.db");
    index(&d, &db);
    let names = |args: &[&str]| -> Vec<String> {
        let out = query(args, &db);
        out.lines().filter(|l| l.starts_with('/')).filter_map(|l| l.split('\t').nth(1)).map(|s| s.split("  ").next().unwrap_or("").to_string()).collect()
    };
    let has = |v: &[String], n: &str| v.iter().any(|s| s.ends_with(&format!(" {n}")));
    // #6 unsafe:
    let fns = names(&["search", "unsafe:fn"]);
    assert!(has(&fns, "raw") && !has(&fns, "safe_wrapper"), "unsafe:fn: {fns:?}");
    let blocks = names(&["search", "unsafe:block"]);
    assert!(has(&blocks, "safe_wrapper") && has(&blocks, "R::by_ref") && !has(&blocks, "raw"), "unsafe:block: {blocks:?}");
    assert_eq!(names(&["search", "unsafe:1"]).len(), fns.len() + blocks.len(), "unsafe:1 = fn + block");
    // #5 self:
    assert!(has(&names(&["search", "self:ref"]), "R::by_ref"));
    let m = names(&["search", "self:mut"]);
    assert!(has(&m, "R::by_mut") && has(&m, "R::by_pin"), "Pin<&mut Self> は mut: {m:?}");
    assert!(has(&names(&["search", "self:owned"]), "R::by_val"));
    assert!(has(&names(&["search", "kind:method", "self:none"]), "R::assoc"));
    // #7 到達性 facet
    let from = names(&["search", "reachable-from:tested_root"]);
    assert!(has(&from, "safe_wrapper") && has(&from, "raw") && !has(&from, "tested_root"), "reachable-from: {from:?}");
    let to = names(&["search", "kind:fn", "reaches:raw"]);
    assert!(has(&to, "safe_wrapper") && has(&to, "tested_root") && has(&to, "untested_unsafe"), "reaches: {to:?}");
    // #8 uncovered: テストから届かない unsafe = untested_unsafe だけ (raw / safe_wrapper はテストから届く)
    let out = query(&["uncovered", "unsafe:1"], &db);
    let strong = out.split("# 候補 edge").next().unwrap_or("");
    assert!(strong.contains("fn untested_unsafe") && !strong.contains("fn safe_wrapper") && !strong.contains("fn raw "), "uncovered unsafe:1:\n{out}");
    // dyn 経由 (宣言に確定 → 実装) でしか届かない lonely は「候補経由でのみ」側
    let all = query(&["uncovered"], &db);
    let (strong, rest) = all.split_once("# 候補 edge").unwrap_or((&all, ""));
    assert!(!strong.contains("fn lonely") && rest.contains("fn lonely"), "trait 経由でのみ届く物は候補側:\n{all}");
}
