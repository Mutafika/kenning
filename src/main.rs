//! kenning — enchudb-backed code intelligence engine (PoC)。
//!
//! code index は派生物 (ビルドキャッシュと同類) なので local で持ち gitignore、VCS には
//! 混ぜない。**完全 standalone** — 外部ツールとは統合しない。freshness は自前の増分 index
//! (`update`) と将来の file-watcher で持つ。

mod kenning;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("index") => {
            // index <dir> [db_path] [--scip <file.scip>]
            let mut pos: Vec<String> = Vec::new();
            let mut scip: Option<String> = None;
            let mut it = args[2..].iter();
            while let Some(a) = it.next() {
                if a == "--scip" {
                    scip = it.next().cloned();
                } else {
                    pos.push(a.clone());
                }
            }
            // dir 省略は "." (repo 内で `kenning index` 一発)。db 省略は repo root から自動導出。
            let dir = pos.first().cloned().unwrap_or_else(|| ".".to_string());
            let db = pos.get(1).cloned().or_else(|| kenning::default_db_for(&dir)).unwrap_or_else(|| {
                eprintln!("# db パスを導出できない ({dir} は repo 外)。index <dir> <db> で明示を。");
                std::process::exit(2);
            });
            kenning::run_index(&dir, &db, scip.as_deref());
        }
        Some("update") => {
            // update [dir] [db] | update <db> | update (引数なし = cwd の repo を自動導出)。
            // 位置引数を「ディレクトリ = dir」「それ以外 = db」に振り分ける。
            let mut dir: Option<String> = None;
            let mut db: Option<String> = None;
            for a in &args[2..] {
                if std::path::Path::new(a).is_dir() {
                    dir = Some(a.clone());
                } else {
                    db = Some(a.clone());
                }
            }
            if dir.is_none() && db.is_none() {
                dir = kenning::repo_root_str("."); // 引数なし: cwd の repo root
                if dir.is_none() {
                    eprintln!("usage: kenning update [dir] [db] (repo 外では dir 指定必須)");
                    std::process::exit(1);
                }
            }
            match (dir, db) {
                (Some(d), Some(b)) => kenning::run_update(&d, &b),
                (Some(d), None) => {
                    let Some(b) = kenning::default_db_for(&d) else {
                        eprintln!("# db パスを導出できない。update <dir> <db> で明示を。");
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
        Some("impls") => kenning::cmd_impls(&args[2..]),
        Some("across") => kenning::cmd_across(&args[2..]),
        Some("path") => kenning::cmd_path(&args[2..]),
        Some("outline") => kenning::cmd_outline(&args[2..]),
        Some("stats") => kenning::cmd_stats(&args[2..]),
        Some("bench") => kenning::cmd_bench(&args[2..]),
        _ => {
            eprintln!(
                "kenning — enchudb-backed code intelligence (PoC)\n\
                 探索コマンドの出力は `path:line<TAB>詳細` = そのまま Read に渡せる。\n\
                 db は末尾 [db_path] / `--db P` / env KENNING_DB (default /tmp/kenning.db)。\n\n\
                 index:\n  \
                 index  <dir> [db] [--scip F]     Rust ソースを full index (--scip で正確名前解決)\n  \
                 update <dir> [db] | update <db>  変更分だけ増分 re-index (dir 省略で index の root)\n  \
                 bake   [dir]                     rust-analyzer scip を焚いて精密 facts を焼き込む\n  \
                 \u{0020}                              (空きメモリゲート + 直列 lock、常駐なし)\n\n\
                 探索 (共通 flag: --db <path> / --limit <n>):\n  \
                 def <name>                       名前の定義位置 (exact, path:line)\n  \
                 read <name> [container]          定義本体をそのまま出す (def + Read の 1 手化)\n  \
                 find <substr>                    名前の部分一致 (発見用、大小無視)\n  \
                 text <term>                      全文検索 + どの関数内かの注釈 (grep superset)\n  \
                 callers <name> [container]       精密 who-calls (確実 ∪ 未確定候補を位置付きで)\n  \
                 callees <name> [container]       X が呼ぶ先 (outgoing、callers の鏡)\n  \
                 edges                            全 cross-file call edge の集計 TSV (from TAB to TAB count)\n  \
                 refs <name> [container]          正確 find-all-refs (要 --scip index、読み書き型も)\n  \
                 impact <name> [container]        推移的 callers = 変えると壊れる範囲 (逆 BFS)\n  \
                 tests <name> [container]         これに届くテスト = impact ∩ is_test (回す物の特定)\n  \
                 impls <trait|type>               go-to-implementation (trait↔型)\n  \
                 across <name>                    全 repo 横断: 全 repo db で定義/利用 + repo 跨ぎ精密参照\n  \
                 path <from> <to>                 from→to の呼び出し経路 1 本 (前方 BFS)\n  \
                 search <facet...>                faceted AND。例: kind:method vis:pub container:Engine\n  \
                 \u{0020}                              facet= name: kind: vis: async: test: crate: container: module:\n  \
                 outline <path>                   ファイルの symbol 一覧 (Read せず構造把握、末尾一致可)\n  \
                 stats                            規模と名前解決率\n  \
                 bench  [quality|agent|micro|all] 再現可能ベンチ (--n/--nq/--seed、markdown 出力)"
            );
            std::process::exit(1);
        }
    }
}
