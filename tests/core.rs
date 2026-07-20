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
