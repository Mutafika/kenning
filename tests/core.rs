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
/// これが「確実 = 誤りなし」の境界。ここでの `d` は free fn の戻り値で、型はどこにも書いていない。
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
         fn make_d() -> D { D }\n\
         pub fn outside() { let d = make_d(); d.ping(); }\n",
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
    write_fixture(&dir, "pub fn entry() { enc(\"x\"); }\nfn enc(s: &str) -> String { s.into() }\n");
    let db = dir.join("k.db");
    index(&dir, &db);
    query(&["changes", "--cursor", "c"], &db);
    std::fs::write(dir.join("src/dup.rs"), "pub fn other() { enc(\"y\"); }\nfn enc(s: &str) -> String { s.to_string() }\n").unwrap();
    update(&dir, &db);
    let out = query(&["changes", "--cursor", "c"], &db);
    assert!(out.contains("other.rs:2\tcallers\tenc: callers 1 → 0"), "重複で確定を失ったのが出ない:\n{out}");
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
pub fn shadowed(db: &Db) { let db = make(); db.get(); }
pub fn generic<T>(db: T) { let _ = db; }
pub fn generic_call<Db2: Fn()>(db: Db2) { db.get(); }
pub fn inner_scope(db: &Db) { { let db = make(); let _ = db; } db.get(); }
fn make() -> Db { Db }
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
