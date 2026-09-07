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
                eprintln!("# 不明な flag {f} (index/update は --db <path> / --scip <file> のみ)");
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
                eprintln!("# db パスを導出できない ({dir} は repo 外)。index <dir> <db> で明示を。");
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
        Some("cache") => kenning::cmd_cache(&args[2..]),
        Some("bench") => kenning::cmd_bench(&args[2..]),
        Some("--version" | "-V" | "version") => println!("kenning {}", env!("CARGO_PKG_VERSION")),
        Some("help" | "--help" | "-h") => println!("{USAGE}"),
        other => {
            if let Some(c) = other {
                eprintln!("# 不明なコマンド: {c}\n");
            }
            eprintln!("{USAGE}");
            std::process::exit(1);
        }
    }
}

const USAGE: &str = concat!(
    "kenning ", env!("CARGO_PKG_VERSION"), " — enchudb-backed code intelligence\n\
     探索コマンドの出力は `path:line<TAB>詳細` = そのまま Read に渡せる。stdout はデータのみ、進捗は stderr。\n\
     db は repo root から自動導出 (~/.cache/kenning/、自動作成・自動更新)。手動なら `--db P` / env KENNING_DB。\n\n\
     index (普段は不要 — query が自動で面倒を見る):\n  \
     index  [dir] [db] [--db P] [--scip F]  Rust ソースを full index (--scip で正確名前解決)\n  \
     update <dir> [db] | update <db>  変更分だけ増分 re-index (dir 省略で index の root)\n  \
     bake   [dir]                     rust-analyzer scip を焚いて精密 facts を焼き込む\n  \
     \u{0020}                              (空きメモリゲート + 直列 lock、常駐なし)\n\n\
     探索 (共通 flag: --db <path> / --limit <n>。<name> は Type::method 形でも可):\n  \
     def <name>                       名前の定義位置 (exact, path:line)\n  \
     read <name> [container] [crate:X] [path:S] [--all]  定義本体 (def + Read の 1 手化)。同名は絞るか --all\n  \
     read <path>:<line>               その行を囲む item の本体 (非 Rust は見出し配下)\n  \
     read <path>:<from>-<to>          行範囲 (sed -n 'A,Bp' の代わり)。跨ぐ定義 / 見出しを列挙\n  \
     read <file>#<見出し>              md の見出し / toml の [table] / yaml のキー配下\n  \
     find <substr>                    名前の部分一致 (発見用、大小無視)\n  \
     text <term>... [-e] [--and] [--files] [path:S]  全文検索 + どの関数内かの注釈 (grep superset)\n  \
     \u{0020}                              複数語 OR / --and で全語 AND、-e 正規表現、--files で file 別件数\n  \
     callers <name> [container]       精密 who-calls (確実 ∪ 未確定候補を位置付きで)\n  \
     callees <name> [container]       X が呼ぶ先 (outgoing、callers の鏡)\n  \
     edges                            全 cross-file call edge の集計 TSV (from TAB to TAB count)\n  \
     refs <name> [container]          正確 find-all-refs (要 --scip index、読み書き型も)\n  \
     impact <name> [container] [--confirmed-only]  推移的 callers = 変えると壊れる範囲 (逆 BFS)\n  \
     \u{0020}                              既定は値渡し参照 (map(f)) も辿る — 見落としの方が危険なので\n  \
     tests <name> [container]         これに届くテスト = impact ∩ is_test (回す物の特定)\n  \
     impls <trait|type>               go-to-implementation (trait↔型)\n  \
     across <name>                    全 repo 横断: 全 repo db で定義/利用 + repo 跨ぎ精密参照\n  \
     path <from> <to>                 from→to の呼び出し経路 1 本 (前方 BFS)\n  \
     search <facet...>                faceted AND。例: kind:method vis:pub container:Engine\n  \
     \u{0020}                              facet= name: kind: vis: async: test: crate: container: module: path:\n  \
     \u{0020}                                     attr: calls: callers: namecalls: traitimpl: reachable:\n  \
     \u{0020}                              reachable:0 = live root から届かない定義 = 消せる候補 (dead な塊ごと)\n  \
     outline <path|dir>               ファイルの symbol / 見出し一覧、dir なら配下 file の地図 (末尾一致可)\n  \
     stats [path:<substr>]            規模と解決の内訳 (repo 内確定率。path: で dir 別)\n  \
     cache  [ls|prune] [--older-than D] [--dry-run]  自動 db の棚卸し / 掃除 (root 消失・旧版)\n  \
     bench  [quality|agent|micro|all] 再現可能ベンチ (--n/--nq/--seed、markdown 出力)\n\n\
     `--version` / `help`"
);
