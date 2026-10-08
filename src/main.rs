//! kenning — enchudb-backed code intelligence engine (PoC)。
//!
//! code index は派生物 (ビルドキャッシュと同類) なので local で持ち gitignore、VCS には
//! 混ぜない。**完全 standalone** — 外部ツールとは統合しない。freshness は自前の増分 index
//! (`update`) と将来の file-watcher で持つ。

mod kenning;

/// index/update の引数を「flag (`--db P` / `--scip F`) と位置引数」に分ける。
/// 探索系 (parse_opts) と同じ `--db` をここでも受ける — 以前は位置引数扱いで、
/// `kenning index . --db x` が **`--db` という名の db を cwd に作っていた** (256MB の残骸が実在)。
/// 知らない `--flag` は db 名に化ける前に拒否する。
fn split_index_args(args: &[String]) -> (Vec<String>, Option<String>, Option<String>) {
    let mut pos = Vec::new();
    let mut db = None;
    let mut scip = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--db" => db = it.next().cloned(),
            "--scip" => scip = it.next().cloned(),
            f if f.starts_with("--") => {
                eprintln!("# unknown flag {f} (index/update take only --db <path> / --scip <file>)");
                std::process::exit(2);
            }
            _ => pos.push(a.clone()),
        }
    }
    (pos, db, scip)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("index") => {
            // index [dir] [db_path] [--db P] [--scip <file.scip>]
            let (pos, db_flag, scip) = split_index_args(&args[2..]);
            // dir 省略は "." (repo 内で `kenning index` 一発)。db 省略は repo root から自動導出。
            let dir = pos.first().cloned().unwrap_or_else(|| ".".to_string());
            let db = db_flag.or_else(|| pos.get(1).cloned()).or_else(|| kenning::default_db_for(&dir)).unwrap_or_else(|| {
                eprintln!("# cannot derive a db path ({dir} is outside a repo). Pass it explicitly: index <dir> <db>");
                std::process::exit(2);
            });
            kenning::run_index(&dir, &db, scip.as_deref());
        }
        Some("update") => {
            // update [dir] [db] [--db P] | update <db> | update (引数なし = cwd の repo を自動導出)。
            // 位置引数を「ディレクトリ = dir」「それ以外 = db」に振り分ける。enchudb v10 の db は
            // directory なので、db 自身を dir と誤認しないよう is_db_path で除外する。
            let (pos, db_flag, _) = split_index_args(&args[2..]);
            let mut dir: Option<String> = None;
            let mut db: Option<String> = db_flag;
            for a in &pos {
                if std::path::Path::new(a).is_dir() && !kenning::is_db_path(a) {
                    dir = Some(a.clone());
                } else {
                    db = Some(a.clone());
                }
            }
            if dir.is_none() && db.is_none() {
                dir = kenning::repo_root_str("."); // 引数なし: cwd の repo root
                if dir.is_none() {
                    eprintln!("usage: kenning update [dir] [db] (dir is required outside a repo)");
                    std::process::exit(1);
                }
            }
            match (dir, db) {
                (Some(d), Some(b)) => kenning::run_update(&d, &b),
                (Some(d), None) => {
                    let Some(b) = kenning::default_db_for(&d) else {
                        eprintln!("# cannot derive a db path. Pass it explicitly: update <dir> <db>");
                        std::process::exit(2);
                    };
                    kenning::run_update(&d, &b);
                }
                (None, Some(b)) => kenning::run_update_from_db(&b),
                (None, None) => unreachable!(),
            }
        }
        Some("bake") => {
            // bake [dir] — rust-analyzer scip → --scip 再 index を一発 (空きゲート + 直列 lock 付き)
            let dir = args.get(2).cloned().unwrap_or_else(|| ".".to_string());
            kenning::run_bake(&dir);
        }
        Some("def") => kenning::cmd_def(&args[2..]),
        Some("read") => kenning::cmd_read(&args[2..]),
        Some("find") => kenning::cmd_find(&args[2..]),
        Some("text") => kenning::cmd_text(&args[2..]),
        Some("search") => kenning::cmd_search(&args[2..]),
        Some("callers") => kenning::cmd_callers(&args[2..]),
        Some("callees") => kenning::cmd_callees(&args[2..]),
        Some("edges") => kenning::cmd_edges(&args[2..]),
        Some("refs") => kenning::cmd_refs(&args[2..]),
        Some("impact") => kenning::cmd_impact(&args[2..]),
        Some("tests") => kenning::cmd_tests(&args[2..]),
        Some("uncovered") => kenning::cmd_uncovered(&args[2..]),
        Some("impls") => kenning::cmd_impls(&args[2..]),
        Some("across") => kenning::cmd_across(&args[2..]),
        Some("path") => kenning::cmd_path(&args[2..]),
        Some("outline") => kenning::cmd_outline(&args[2..]),
        Some("stats") => kenning::cmd_stats(&args[2..]),
        Some("cache") => kenning::cmd_cache(&args[2..]),
        Some("changes") => kenning::cmd_changes(&args[2..]),
        Some("bench") => kenning::cmd_bench(&args[2..]),
        Some("--version" | "-V" | "version") => println!("kenning {}", env!("CARGO_PKG_VERSION")),
        Some("help" | "--help" | "-h") => println!("{USAGE}"),
        other => {
            if let Some(c) = other {
                eprintln!("# unknown command: {c}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(1);
        }
    }
}

const USAGE: &str = concat!(
    "kenning ", env!("CARGO_PKG_VERSION"), " — enchudb-backed code intelligence\n\
     Query output is `path:line<TAB>detail` = pass it straight to Read. stdout is data only; progress goes to stderr.\n\
     The db is derived from the repo root (~/.cache/kenning/, created and refreshed automatically). Manual: `--db P` / env KENNING_DB.\n\n\
     index (usually not needed — queries take care of it):\n  \
     index  [dir] [db] [--db P] [--scip F]  full index of Rust sources (--scip for exact name resolution)\n  \
     update <dir> [db] | update <db>  incremental re-index of changed files (dir defaults to the index root)\n  \
     bake   [dir]                     run rust-analyzer scip and bake in precise facts\n  \
     \u{0020}                              (free-memory gate + serialized lock, nothing resident)\n\n\
     queries (common flags: --db <path> / --limit <n>. <name> may be Type::method):\n  \
     def <name> [container|path:S]    where a name is defined (exact, path:line)\n  \
     read <name> [container] [crate:X] [path:S] [--all]  definition body (def + Read in one step). Narrow same-named ones or --all\n  \
     read <path>:<line>               body of the item enclosing that line (non-Rust: the section under the heading)\n  \
     read <path>:<from>-<to>          a line range (instead of sed -n 'A,Bp'); lists definitions / headings it spans\n  \
     read <file>#<heading>            under a md heading / toml [table] / yaml key\n  \
     find <substr>                    substring match on names (discovery, case-insensitive)\n  \
     text <term>... [-e] [--and] [--files] [path:S]  full-text search annotated with the enclosing fn (grep superset)\n  \
     \u{0020}                              several terms = OR / --and = all terms, -e regex, --files per-file counts\n  \
     callers <name> [container] [crate:X] [path:S]  precise who-calls (confirmed ∪ unresolved candidates, with positions)\n  \
     callees <name> [container|path:S] what X calls (outgoing, mirror of callers)\n  \
     edges                            all cross-file call edges, aggregated TSV (from TAB to TAB count)\n  \
     refs <name> [container]          exact find-all-refs (needs a --scip index; includes read/write/type refs)\n  \
     impact <name> [container] [--confirmed-only]  transitive callers = what breaks if it changes (reverse BFS)\n  \
     \u{0020}                              follows pass-by-value refs (map(f)) by default — missing one is worse\n  \
     tests <name> [container]         tests that reach it = impact ∩ is_test (what to run)\n  \
     uncovered [facet...]             fn/methods no test reaches (e.g. uncovered unsafe:1)\n  \
     impls <trait|type>               go-to-implementation (trait↔type)\n  \
     across <name>                    across all repos: definitions/uses in every repo db + precise cross-repo refs\n  \
     path <from> <to>                 one call chain from→to (forward BFS)\n  \
     search <facet...>                faceted AND. e.g. kind:method vis:pub container:Engine\n  \
     \u{0020}                              facet= name: kind: vis: async: test: crate: container: module: path:\n  \
     \u{0020}                                     attr: calls: callers: namecalls: traitimpl: reachable:\n  \
     \u{0020}                              reachable:0 = definitions no live root reaches = removable (whole dead clusters)\n  \
     outline <path|dir>               symbols / headings of a file; for a dir, a map of its files (suffix match ok)\n  \
     changes [--since <git ref|token> | --cursor <name>] [--json] [--all]  semantic diff (broken refs /\n  \
     \u{0020}                              signature changes / newly dead / revived). `--since HEAD` = pre-commit check\n  \
     stats [path:<substr>]            size and resolution breakdown (in-repo confirmed rate; path: per dir)\n  \
     cache  [ls|prune] [--older-than D] [--dry-run]  inventory / clean up auto dbs (root gone, old versions)\n  \
     bench  [quality|agent|micro|all] reproducible benchmarks (--n/--nq/--seed, markdown output)\n\n\
     `--version` / `help`"
);
