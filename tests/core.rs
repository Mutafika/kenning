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

/// Fresh, empty temp dir with a `src/` subdir.
fn tmp() -> PathBuf {
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
    let out = kenning()
        .args(cmd)
        .args(["--db", db.to_str().unwrap()])
        .env("KENNING_NO_STALE", "1")
        .output()
        .expect("spawn kenning query");
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
