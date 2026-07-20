# kenning

**Rust のためのセマンティックコード検索 — AI コーディングエージェント向けに設計。**
[English README](README.md)

*[kenning](https://en.wikipedia.org/wiki/Kenning) は、概念を凝縮した短い名前に畳む
古ノルド語の技法 — 海を「鯨の道 (whale-road)」と呼ぶような。このツールはそれをコード
ベースに対してやる: コールグラフ全体を、エージェントが実際に必要とする数行に圧縮する。
"ken"（＝知りうる範囲）の意味も含んでいる。*

`kenning` は、エージェント（や人間）がコードベースを探索するときに実際に問う質問 —
*誰がこれを呼ぶ? 変えたら何が壊れる? どの型がこの trait を実装している?* — に、
ミリ秒で答える。事前に焼いた fact データベースから。常駐する language server も、
ギガ単位の RAM も、index の儀式も要らない。

```bash
cd your-rust-repo
kenning callers finish_with_oplog     # これだけ — index は初回クエリで自分で建つ
```

## なぜ

AI コーディングエージェントは `grep` ＋ファイル通読でコードを探索する。動くが、token を
焼く: 「X を変えたら何が壊れる?」は grep とファイル読みの再帰的な連鎖になる — 1 問で
数百回の tool 呼び出し（enchudb で 808 回、下記で実測）。kenning の `impact` は事前に
焼いたグラフから 1 レスポンスで答える: ベンチ corpus 全体で **13–52× 少ないバイト**、
そして問いが深いほど差が広がる。

古典的な精密解は language server だが、rust-analyzer は 1 問ずつ答えるために複数 GB の
RSS で常駐する。一方でエージェントはバースト的に、多数の repo から、しばしばそのメモリが
存在しない SSH 先のマシンで問う。

`kenning` は第三の道を採る — Meta の Glean や Google の Kythe と同じ形を、ローカルの
単一バイナリまで縮小したもの:

1. **Parse**: 全 `.rs` を `syn` でパース（速い・cfg 非依存・ビルド不要）→ symbol /
   call-site / impl を、埋め込み faceted データベース
   ([enchudb](https://github.com/Mutafika/enchudb)) の行にする。
2. **Bake**（任意・コマンド一発）: rust-analyzer を*一度だけ*バッチコンパイラとして走らせ
   （`kenning bake`）、その SCIP 出力を取り込んで syn の fact と位置 join する。call 解決が
   rust-analyzer とちょうど同じ正確さになる — そして rust-analyzer は終了する。
3. **Serve**: fact DB から µs でクエリに答える。全列が自動 index されるので、faceted な
   連言（`kind:method vis:pub container:Engine calls:unwrap`）は scan でなくバケット交差。

index は自己維持する: 古いファイルは毎クエリで検出され（~10 ms の stat-walk）増分再 index
される（小編集で ~18 ms）ので、答えが黙って古くなることはない。

## インストール

```bash
cargo install --git https://github.com/Mutafika/kenning
```

これでインストールは全部 — ストレージエンジン
[enchudb](https://github.com/Mutafika/enchudb) は pin された git 依存として一緒に入る。
精密モード（`bake`）を使うなら rust-analyzer も用意する
（`rustup component add rust-analyzer`）。

kenning と enchudb を並べて開発する場合は、両方を横に checkout して
`.cargo/config.toml` で依存をローカルの checkout に向ける:

```toml
[patch."https://github.com/Mutafika/enchudb"]
enchudb = { path = "../enchudb" }
enchudb-oplog = { path = "../enchudb/crates/enchudb-oplog" }
```

## コマンド

```
kenning def     <name>              定義 + シグネチャ + doc 1 行目 (hover 相当)
kenning read    <name> [container]  定義本体をそのまま出力 (def + ファイル読みを 1 手に)
kenning find    <substr>            名前の部分一致 (発見用)
kenning text    <term>              全文検索 + enclosing symbol 注釈 (grep superset)
kenning callers <name> [container]  who-calls: 確実 ∪ 未確定候補を位置付きで
kenning callees <name> [container]  呼ぶ先 (outgoing)
kenning edges                       全 cross-file call edge を集計 (from\tto\tcount TSV)
kenning refs    <name> [container]  find-all-references (要 bake; 型参照・読み書きも)
kenning impls   <trait|type>        go-to-implementation (双方向)
kenning impact  <name> [container]  推移的 callers = 変更の影響範囲 (逆 BFS)
kenning tests   <name> [container]  この symbol に届くテスト = impact ∩ is_test
kenning path    <from> <to>         A から B への呼び出し経路 1 本 (前方 BFS)
kenning across  <name>              index 済み全 repo 横断の精密参照
kenning search  kind:method vis:pub container:Engine   faceted 等値 AND
kenning outline <path>              ファイルを読まずに構造把握
kenning bake                        rust-analyzer を 1 回走らせ SCIP 取込 → RA 級精度
kenning stats                       index 規模 + 解決率
```

出力は stdout の決定的な `path:line<TAB>詳細` 行（進捗は stderr）— 各行はそのまま
ファイルリーダに渡せる。repo には `CLAUDE.md` が同梱され、Claude 系エージェントが
プロンプトなしで正しいサブコマンドを選べる。

## 設計の要点

- **誠実な完全性。** `callers` は 3 つのラベル付き集合を返す: *確実*（解決済みエッジの
  逆引き — 誤検出なし）、*候補*（未解決の同名 call-site — 要確認）、*別定義に確定*。
  和集合は grep-complete で、ラベルがどの行を無条件に信じてよいか教える。推測は一切しない。
- **cfg-blind 回収。** rust-analyzer は活性な cfg 構成しか解析しないので、SCIP は off の
  `#[cfg(...)]` ブランチ内で沈黙する。`syn` は全ブランチを見る。SCIP が沈黙する所は
  保守的な syn resolver にフォールバック — kenning は rust-analyzer 自身が取りこぼす
  impl や caller を見つける。
- **GIGO は明示的。** 精度は食わせた SCIP に等しい。`bake` は `--config-path` で
  `features = "all"` を注入する（tokio ではこれが解決 call edge 177 と 6,760 の差になる）。
  解決率は隠さず表示する。
- **index は派生物。** `~/.cache/kenning/` に住み、repo の中には入らず、repo root で
  keying。いつ消してもよい — 次の質問で建て直す。
- **repo 横断。** SCIP symbol は大域的に一意（crate + version）なので、`across` はある
  repo の定義 symbol を、index 済み全 repo の外部参照テーブルと join する — 単一
  workspace の language server にはできない repo 跨ぎの find-references。

## 実測（再現可能スイート、逸話ではない）

自分で回せる: `./bench/corpus.sh && ./bench/run.sh` — tag 固定の corpus
（tokio @ tokio-1.43.0）、固定乱数 seed、手法は各表の直上に自己記述。全文:
[bench/RESULTS.md](bench/RESULTS.md)。

| スイート | tokio (722 files) | ripgrep (100 files) | enchudb (175 files) | 測るもの |
|---|---|---|---|---|
| **agent** — 「誰が X を呼ぶ?」の答えまでのバイト数 | **5.3×** 少・15 呼→1 | **2.3×** 少・3 呼→1 | **10.2×** 少・30 呼→1 | 固定 20 問、grep 経路は*楽観*モデル（下限）vs 実際の `callers` 出力 |
| **beyond** — 「X を変えたら何が壊れる?」(`impact`) | **46×**・321 呼→1 | **13×**・75 呼→1 | **52×**・808 呼→1 | 推移的 caller BFS: grep 経路 = エージェントが実際にやる手動 grep+read 再帰 |
| **quality** — ランダム 100 symbol の grep ノイズ | 中央値 43% | 中央値 33% | 中央値 33% | `\bname\(` ヒットのうち def/コメント/文字列/別 symbol の割合 = 無駄に読む行 |
| **micro** — warm クエリ遅延 | 125 ns – 4 µs | 同等 | 125 ns – 2.3 µs | faceted カウント・def 検索・精密逆引き callers |

同じスイートは他の非サーチクエリも測る: `impls`（go-to-implementation）10–23×、
`outline`（読まずに構造把握）7–12×（442 KB のファイルが 42× に圧縮）、`def`（hover:
位置 + シグネチャ + doc 行）6–10×。faceted クエリは grep 等価物が存在しない — µs で走り、
比ではなく能力として報告する。パターンに注目: **問いが深いほど差が開く** — ripgrep では
素の who-calls は 2.3× だが推移的 impact は 13×、grep 経路は BFS の hop ごとに掛け算になるから。

このばらつきが正直な物語: 優位は symbol がどれだけ広く呼ばれるかに比例する。ripgrep —
小さく、よく分割されていることで有名 — が床（2.3×、中央値の symbol は 3 箇所から呼ばれる）；
enchudb のホットな symbol（30 箇所）は 10.2×。最悪例は grep が最も溺れる所:
enchudb の `len` = 988 grep ヒットのうち、ローカルな `len` の確実な caller は 46。

**vs rust-analyzer** ([bench/VS-RA.md](bench/VS-RA.md), `./bench/vs-ra.sh`):
cold から「who-calls に答えられる」までの時間とメモリ — RA（`analysis-stats`、RA 自身の
ベンチツール）: enchudb で 39 s / 6.1 GB、対して kenning syn index: 0.45 s / 175 MB、
以後の常駐ゼロ。精度のトレードと feature スコープの注記は表の隣に明記。

**vs CodeQL** ([bench/VS-CODEQL.md](bench/VS-CODEQL.md), `./bench/vs-codeql.sh`):
GitHub の「code as data」エンジン。その Rust extractor も rust-analyzer ベース — 同じ
アーキテクチャを、ナビゲーションでなくセキュリティ解析向けに作ったもの。enchudb で:
DB 構築 **68 分 / 9.9 GB / 301 MB** vs 2.4 s / 174 MB / 67 MB；who-calls 1 問が
**cache 済でも 47.5 s** vs 0.1 s。lib コードでは回答が完全一致（44 = 44 — 3 本目の独立
相互検証）；CodeQL は現状 `tests/` の caller をほぼ落とす（57 行 vs 117）。QL は kenning が
決してやらない問い（taint tracking）を聞ける — 別の仕事、同じ facts の発想。

**vs Glean (Meta)** ([bench/VS-GLEAN.md](bench/VS-GLEAN.md), `./bench/vs-glean.sh`):
最も純粋な対決 — Glean の Rust 経路も rust-analyzer SCIP なので、**まったく同じ .scip
ファイル**を両エンジンに食わせ、serving 層だけを比べた。取込 8.0 s / 702 MB vs
0.52 s / 268 MB；find-refs 1 問 ~1.0 s vs 0.011 s（Rosetta で説明できるのはせいぜい
2–3×）；回答は一致（57 vs 58、def-role のカウント差 — 4 本目の独立相互検証）。Glean は
facts の disk（14 MB vs 87 MB — こちらは syn コールグラフと facet も載せている）で勝ち、
live source に join しないので古い SCIP も優雅に serve できる。

**vs ast-grep**（構造検索; 同じ質問、agent スイート内）: その構造一致は kenning の
確実 ∪ 候補 集合とほぼ完全に一致（`tie` 321 = 321、`clone` 621 = 224+397）— call-site
検出が完全であることの独立相互検証。差は: 1 問あたり中央値 564–878 ms（repo walk、
ユーザーが列挙する 3 つの call 形パターン）vs 13–19 ms（index 済み）、そして名前解決が
ない — call が*どの*定義に属すか言えず、impact/path/faceted/cross-repo も無い。

- index 構築: ~1k LOC/ms（enchudb: 175 files / 2,905 symbols / 26,131 call-sites を約 360 ms）。
- 小編集後の増分 update: ~18 ms。クエリごとの鮮度チェック: ~10 ms。
- `bake`: rust-analyzer のバッチ 1 回（peak ≈ 5 GB で ~30–50 s）、その後 **常駐ゼロ**。
  enchudb の解決率: 26.6%（syn のみ）→ 39.8%（baked, features=all）。残る未解決は
  std / 外部 crate の call が主で、これらもラベル付き候補として列挙される。

## 設計の取引 — やらないこと、とその代償

上の数字は全部「何かをやらない」ことで買っている。台帳:

| やらないこと | 買ったもの | 代償（実測・実感） |
|---|---|---|
| 型推論（`x.f()` の受け手） | 0.5 s 構築、18 ms 増分、cfg 全ブランチ被覆 | syn のみの解決は 13–26% 止まり；精密は `bake`（40 s / 5 GB の RA 1 回）が要る |
| hover / 補完 / 診断 | 常駐ゼロ、LSP プロトコル不要 | 人間のエディタにはならない；型はエージェントが `cargo check` で得る |
| マクロ展開 | per-file の parse 速度 | マクロ内で生まれる call / impl はどの層からも見えない |
| 常駐サーバ / file watcher | RAM 0、運用ゼロ、SSH 先で動く | 毎クエリ ~10–20 ms の stat-walk が床；warm-µs の数字は in-process のみ |
| SCIP の as-is serve（代わりに live source へ位置 join） | 答えが常に今のコードを指す | 古い bake は精密 facts を落とす（Glean 戦で `refs → 0` を実地で踏んだ；`upd_since_bake` が警告） |
| 推測（解決の偽装をしない） | 確実集合に誤検出ゼロ | エージェントは*候補*バケットの目視が残る |
| 汎用クエリ言語（Angle/QL） | 学習コストゼロ、µs の答え | 任意の関係クエリ（taint tracking）は CodeQL の領分のまま |
| Rust 以外の言語（今は） | 深さ（cfg 回収、trait コンテナ） | TS/Python repo では無力；fact schema 自体は言語中立 |
| disk 節約 | 全列自動 index + syn グラフを SCIP と並走 | 同じ corpus で 87 MB vs Glean の 14 MB |

**~15 コマンドで足りるのか？** 閉じた集合ではない — エージェントが実際に聞く質問の語彙
（定義 / 利用者 / 呼ぶ先 / 影響範囲 / 実装 / 経路 / 構造）を dogfooding で育てたもの。実運用で
逃げ道（grep + ファイル読み）が要ったのは長い尾に対してで、ナビゲーションではない。欠けが
出たら新コマンドは午後の仕事であってプロジェクトではない: facts は既に store にある —
`tests <name>`（どのテストがこの symbol を通るか）は文字通り `impact ∩ is_test` で、既存の
facts の合成として一度の作業で足せた。資産はコマンド一覧ではなく schema。

## ライセンス

MIT。ストレージエンジン ([enchudb](https://github.com/Mutafika/enchudb)) は別ライセンス
（FSL-1.1-Apache-2.0）。
