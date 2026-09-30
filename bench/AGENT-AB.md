# 実 agent の A/B — kenning あり / なしで同じ課題を解かせる

生成: `./bench/agent-ab.sh` (headless `claude -p`、Claude Opus 5.5、2026-09-28)。採点: `bench/agent-ab-score.py`。

`bench/RESULTS.md` の agent / beyond スイートは「grep 経路をモデル化した下限」との**バイト比**で、実際の agent が
どれだけ得をするかは測っていない。こちらは本物の agent に同じ課題を解かせ、**費用・turn・時間・正答**を比べる。

## 条件

| 条件 | 道具 | 案内 (system prompt に追記) |
|---|---|---|
| **A** | Read / Grep / Glob / 読むだけの Bash (rg grep sed cat head tail find ls wc awk) + **kenning** | 全文の案内 (~15KB。当時の CLAUDE.md、今は `docs/GUIDE.md`) |
| **B** | Read / Grep / Glob / 読むだけの Bash | なし |
| **C** | A と同じ | 短い案内 `agent-ab/guide-slim.md` (~1.5KB) |

同じ課題の A / B / C は同時に走らせる (負荷と prompt cache の条件を揃える)。各 3 回、中央値。
許可の外の呼び出し (for 文の複合コマンド等) で拒否されたのは 3 条件とも数回ずつ (A 2 / B 4 / C 4、全 run 計)。

## 課題と正解 (kenning と独立に作る)

| 課題 | 問い | 正解の作り方 |
|---|---|---|
| tokio-add-permits | `sync::Semaphore::add_permits` の全呼び出し箇所 (12) | rename → `cargo check --all-targets` の error 位置 |
| tokio-joinset-spawn | `JoinSet::spawn` に引数を足す時に直す箇所 (25。同名 `spawn` 1,140 件に埋もれる) | 同上 + cfg(loom) の 5 件 |
| enchudb-oplog-open | `OpLog::open` の変更で直す箇所 (24。12 型に `open` 209 件) | 同上 (`truth-rename.sh`) |
| enchudb-leafstore-insert | `LeafStore::insert` の変更で直す箇所 (44。`insert` 562 件) | 同上 |
| tokio-sleep-reset-tests | `Sleep::reset` を変えた後に回すテスト (35) | 本体に目印を入れて全テストを 1 本ずつ実行 = 実際に通ったテスト |
| tokio-block-on-park | `block_on` から thread が park するまでの呼び出しの連鎖 | 手で追った 8 段 (全 run が 8 段とも到達) |

## 結果

### 1 回目 — v0.5.0 (commit 21493ae)

| 課題 | A 費用 / turn / 秒 | B 費用 / turn / 秒 | C 費用 / turn / 秒 | 正答 (recall) |
|---|---|---|---|---|
| tokio-add-permits | $0.06 / 3 / 13 | $0.08 / 4 / 19 | $0.05 / 4 / 16 | 全条件 100% |
| tokio-joinset-spawn | $0.22 / 6 / 27 | $0.22 / 11 / 40 | $0.21 / 13 / 40 | 全条件 100% |
| enchudb-oplog-open | $0.13 / 5 / 19 | $0.13 / 7 / 24 | $0.07 / 4 / 20 | 全条件 100% |
| enchudb-leafstore-insert | $0.11 / 4 / 22 | $0.23 / 11 / 38 | $0.13 / 7 / 25 | 全条件 100% |
| tokio-sleep-reset-tests | $0.24 / 9 / 49 | $0.56 / 14 / 93 | $0.19 / 8 / 48 | 全条件 100% |
| tokio-block-on-park | $0.23 / 9 / 36 | $0.27 / 14 / 40 | $0.19 / 7 / 33 | 全条件 8/8 段 |

合計 (課題ごとの中央値の和): **B は A の 1.50 倍、C の 1.77 倍の費用**。turn B/A 1.69x、時間 B/A 1.53x。
input token は B/A 1.01x (A は CLAUDE.md 全文の固定費を毎 turn 払う)、B/C 1.24x。

### 2 回目 — 取りこぼしを直した版 (commit b59e5dd)

1 回目の結果を調べて見つけた kenning の穴 (下記) を直してから、同じ課題を回し直した。

| 課題 | A 費用 / turn / 秒 | B 費用 / turn / 秒 | C 費用 / turn / 秒 | 正答 (recall) |
|---|---|---|---|---|
| tokio-add-permits | $0.06 / 3 / 12 | $0.08 / 4 / 18 | $0.04 / 4 / 14 | 全条件 100% |
| tokio-joinset-spawn | $0.20 / 7 / 29 | $0.24 / 13 / 45 | $0.21 / 8 / 32 | 全条件 100% |
| enchudb-oplog-open | $0.10 / 4 / 17 | $0.16 / 8 / 25 | $0.09 / 5 / 21 | 全条件 100% |
| enchudb-leafstore-insert | $0.10 / 6 / 24 | $0.24 / 11 / 33 | $0.11 / 5 / 22 | 全条件 100% |
| tokio-sleep-reset-tests | $0.27 / 7 / 51 | $0.65 / 20 / 122 | $0.23 / 8 / 47 | 全条件 100% |
| tokio-block-on-park | $0.22 / 7 / 34 | $0.24 / 12 / 41 | $0.15 / 6 / 30 | 全条件 8/8 段 |

合計: **B は A の 1.69 倍、C の 1.91 倍の費用**。turn B/A 2.00x・B/C 1.89x、時間 B/A 1.71x・B/C 1.70x。
input token B/A 1.15x、B/C 1.72x。

### 3 回目 — 案内の長さ (B / C / D、commit b59e5dd の build)

D = CLAUDE.md を短くした版 (日本語 ~2.1KB、要点 + 詳細は `docs/GUIDE.md` を必要な節だけ読む)。

| 課題 | B 費用 / turn / 秒 | C 費用 / turn / 秒 | D 費用 / turn / 秒 |
|---|---|---|---|
| tokio-add-permits | $0.18 / 4 / 82 | $0.05 / 5 / 13 | $0.06 / 5 / 19 |
| tokio-joinset-spawn | $0.16 / 6 / 30 | $0.20 / 11 / 39 | $0.19 / 13 / 39 |
| enchudb-oplog-open | $0.10 / 6 / 23 | $0.08 / 4 / 21 | $0.09 / 5 / 21 |
| enchudb-leafstore-insert | $0.22 / 10 / 33 | $0.12 / 5 / 23 | $0.13 / 6 / 25 |
| tokio-sleep-reset-tests | $0.62 / 18 / 118 | $0.16 / 7 / 44 | $0.28 / 9 / 48 |
| tokio-block-on-park | $0.25 / 14 / 39 | $0.15 / 5 / 28 | $0.15 / 6 / 32 |

合計: **B は C の 2.03 倍、D の 1.70 倍の費用**。時間 B/C 1.94x・B/D 1.77x。正答は全条件 100% (テスト特定の precision は
C / D とも 65% = 静的に届く上位集合)。案内の長さ別の B との費用比 (3 回の実測): 全文 15KB (A) 1.50x / 1.69x、
日本語 2.1KB (D) 1.70x、英語 1.5KB (C) 1.77x / 1.91x / 2.03x。

### 4 回目 — Haiku (探索の subagent が使う model、v0.5.1)

`AB_MODEL=claude-haiku-4-5-20251001`、B (grep のみ) と D (kenning + CLAUDE.md)。

| 課題 | B 費用 / turn / 秒 / input | D 費用 / turn / 秒 / input | 正答 B → D |
|---|---|---|---|
| tokio-add-permits | $0.15 / 29 / 82 / 512k | $0.05 / 5 / 32 / 78k | recall 100 → 100%、precision 80 → 100% |
| tokio-joinset-spawn | $0.19 / 24 / 100 / 815k | $0.09 / 12 / 57 / 267k | 100 → 100% |
| enchudb-oplog-open | $0.05 / 4 / 16 / 89k | $0.03 / 2 / 17 / 32k | recall 100 → 96% |
| enchudb-leafstore-insert | $0.19 / 38 / 89 / 957k | $0.04 / 3 / 39 / 46k | recall 93 → 100% |
| tokio-sleep-reset-tests | $0.33 / 46 / 156 / 1.78M | $0.02 / 2 / 22 / 29k | recall 89 → 74% |
| tokio-block-on-park | $0.36 / 57 / 198 / 2.02M | $0.24 / 39 / 145 / 1.11M | 8 段中 6〜8 → 全 run 8 |

合計: **B は D の 2.74 倍の費用、input token 3.95 倍、turn 3.14 倍、時間 2.05 倍**。Opus より差が大きい —
Haiku は grep だけだと Read / Grep を 24〜57 回往復し、1 課題で input 2M token に達する。正答も B の方が
崩れる (取りこぼし・余計な行・経路の途中で止まる)。

逆に **D がテストの特定で 74% に落ちた**のは、Haiku が `kenning tests` の出力をそのまま答えにしたから
(kenning 自体が 35 本中 24 本しか静的に辿れない — generic / `tokio::pin!` / trait 経由)。Opus は grep で
補って 100% にしていた。弱い model ほど kenning の答えが最終回答になる = kenning の取りこぼしがそのまま出る。

### 5 回目 — grep に戻った理由を潰した後 (D、Haiku / Opus、commit 8441d22 + 条件付き定義の修正)

kenning ありの run で agent が grep に戻った ~310 回を分類した。多い順に (1) `callers` の後に取りこぼしを
grep で数え直す (別の同名に確定した分の行き先が見えなかった)、(2) 別名 (`use X as Y`) の確認、(3) 同名の定義を
`grep -n "fn X" -A5` で読む、(4) kenning の本当の穴 (`tests` の取りこぼし) の補完。(1) は行き先を定義ごとに出し、
(3) は案内に `read <name> path:` を足し、(4) の一部 (cfg_rt! の中の定義に外から確定しなかった) を直した。

| 課題 | Haiku D (前 → 後) | 正答 (前 → 後) |
|---|---|---|
| tokio-joinset-spawn | $0.09 / 12 turn → **$0.03 / 2 turn** (kenning 1 回) | recall 100% → 100% (途中の版では 76%) |
| tokio-add-permits | $0.05 / 5 → $0.01 / 2 | 100% → 100% |
| enchudb-leafstore-insert | $0.04 / 3 → $0.03 / 2 | 100% → 100% |
| tokio-sleep-reset-tests | $0.02 / 2 → $0.02 / 2 | recall 74% → 74% (kenning の `tests` が 35 本中 26 本 — Haiku はそのまま答える) |

Haiku は kenning の出力を 1 回見てそのまま答えるようになった (grep だけだと同じ課題に 24〜46 turn)。その分、
**kenning の答えの穴がそのまま正答率になる**: 残る穴は trait 経由 (`Stream::poll_next`) と、generic /
`tokio::pin!` 越しの受け手。

### 6 回目 — `tests` / `impact` に候補経由の層、`use .. as` の別名 (D、Haiku / Opus、INDEX_VER 28)

5 回目の残り 2 つを潰した。(a) `tests` / `impact` は確定 edge だけを辿っていた — `callers` が ⚠候補に出している
呼び出し (受け手の型不明 / trait 経由) を 1 本まで通って届く物を `[c1]` として同じ出力に並べる (同名定義が多く候補が
64 を超える名前は辿らず `#` 行に名前を出す、`Drop::drop` は名前で呼べないので除外)。(b) `use a::f as g` の `g()` が
`callers f` の名前一致から丸ごと漏れていた (別名 fn / `{self as aa}` / 別名の型の関連関数) — 元の名前で持つ。
rust-analyzer との突き合わせは 4 repo とも誤確定 0 のまま。比較の相手は 5 回目 (同じ課題・同じ 3 run、前日)。

| 課題 | Haiku 前 → 後 (費用 / turn / recall) | Opus 前 → 後 (費用 / turn / grep 系の回数) |
|---|---|---|
| tokio-sleep-reset-tests | $0.02 / 2 / **74%** → $0.05 / 4 / **100%** (precision 58 → 42%) | $0.20 / 8 / 2.3 → $0.22 / 8 / 3.0 |
| tokio-joinset-spawn | $0.03 / 2 / 100% → $0.03 / 2 / 100% | $0.19 / 8 / 4.7 → $0.19 / 5 / 3.3 |
| tokio-add-permits | $0.01 / 2 / 100% → 同じ | $0.04 / 4 / 1.7 → $0.05 / 4 / 2.0 |
| enchudb-oplog-open | $0.03 / 2 / 96% → 同じ | $0.07 / 4 / 1.0 → $0.09 / 5 / 0.7 |
| enchudb-leafstore-insert | $0.03 / 2 / 100% → $0.04 / 2 / 100% | $0.11 / 4 / 1.7 → $0.11 / 6 / 3.3 |
| tokio-block-on-park | $0.29 / 44 → $0.32 / 51 (grep 系 6 → 16) | $0.18 / 6 / 3.3 → $0.17 / 7 / 4.0 |

- **Haiku のテスト特定が 74% → 100%。** kenning 1 回の出力 ([d] 45 + [c1] 39) をそのまま答えて届く。precision は
  静的に届き得る上位集合を全部挙げる分だけ下がる (回すテストとしては安全側)。Haiku は block-on-park 以外で grep 0 回。
- **Opus の grep 系は減らなかった** (課題あたり合計 14.7 → 16.3 回、3 run の揺れの内)。中身を読むと kenning の穴では
  なく癖: `sed -n A,Bp` で行を切り出す (block-on-park の大半。`read <path>:A-B` がある)、`.md` を含む語の検索
  (`text` がある)、`callers` の答えの `grep add_permits(` での数え直し。機能を足しても 0 にはならない種類。
- **block-on-park は別の穴。** `path Runtime::block_on park` は最初の `park` (`Context::park`、4 段) で止まる。
  正解の 8 段は driver の enum を `match` で振り分ける先 (`Driver::park` → time / io driver) まで続き、そこは
  静的な呼び出しグラフの外。Haiku はここで Read / grep を 50 turn 往復する。

### 7 回目 — パターン束縛の型 / `path` の同名委譲 / 切れた一覧の案内 (D、Haiku / Opus、INDEX_VER 29)

6 回目で「静的な呼び出しグラフの外」と書いた block-on-park の残りは誤りだった — `match self { E::V(x) => x.park() }`
の x の型は variant の定義に書いてある。(a) enum variant / tuple struct の field 型を field 表に載せ、パターン
`E::V(x)` / `E::V { f }` / `Self::V(x)` / `T(x)` と `self.0` の受け手をたどる (`use E::*` の後の `V(x)` は同名の
tuple struct と取り違え得るので確定しない)。syn 層の typed 確定: tokio +39 / ripgrep +32 / enchudb +116、RA との
突き合わせは 4 repo とも誤確定 0。(b) `path X park` が最初の `park` で止まる時、終点から同名の定義へ続く委譲
(enum の振り分け先を含む) を木で出す。(c) `--limit` で切れた一覧は「全 N 件は `--limit N` か `--limit 0`」と打てる
形で出し、`--limit 0` = 上限なし。

| 課題 | Haiku 6 回目 → 7 回目 | Opus 6 回目 → 7 回目 |
|---|---|---|
| tokio-block-on-park | $0.32 / 51 turn → $0.30 / 46 turn。答えは 3 run とも io driver の `turn` まで | $0.17 / 7 → $0.15 / 5。3 run とも `io::Driver::turn` まで |
| tokio-sleep-reset-tests | $0.05 / 4 / 100% → (b 前) 74% → (c 後) **$0.05 / 3 / 100%** | $0.22 / 8 / 100% → $0.26 / 8 / 100% |
| 他の 4 課題 | 同じ ($0.01〜0.03 / 2 turn / grep 0) | 同じ (正答 100%) |

- **テスト特定が 74% に戻ったのは kenning の出力の切れ方。** 既定の `--limit 50` で 84 件中 50 件が表示され、
  Haiku はそれをそのまま答えにした (「--limit で全部」の案内だけでは件数が分からない)。(c) の後は 3 run とも 100%。
- **block-on-park で Haiku がまだ Read を 20 回するのは、問いが実行時の枝を聞いているから。** `path Runtime::block_on
  Driver::turn` は 11 段を 1 回で出すが、最短は `park_yield` 経由 — 「future が ready でない時」の `park` の枝かは
  本文を読んで決めるしかない。kenning の穴ではない。
- Opus の grep 系は 6 回目と同じ (16〜17 回、癖)。

## 読み方 (正直な所)

- **正答は差が無い。** grep だけの Opus も 1,140 件の同名 `spawn` から 25 件を全部当てる。kenning の価値は
  「grep だと間違える」ではなく、**同じ答えに半分の turn・6 割の費用で着く**こと。
- **差が一番開くのはテストの特定** (B は $0.56〜0.65・93〜122 秒、A / C は $0.19〜0.27・47〜51 秒)。推移的な
  呼び出しを追う問いほど grep の往復が増える — `RESULTS.md` の「問いが深いほど差が開く」と同じ向き。
- テストの特定で A / C の precision が 65% に見えるのは、実行時には通らなかったが**静的には届く**テストも挙げた
  から (35 本に対して 54 本)。変更後に回すテストとしては安全側。B は 35 本ちょうどを当てたが費用は 2.4 倍。
- **案内は短い方が得。** CLAUDE.md 全文 (A) は毎 turn ~15KB を読ませるので、簡単な課題では input token で
  B に負ける。~1.5KB の案内 (C) は input でも B より 1.2〜1.7 倍少ない。
- 1 回目と 2 回目で B (kenning 無し) の費用も $1.49 → $1.60 と揺れている。比較は同じ回の中の比で見ること。
  1 条件 3 run・1 model・prompt cache の効き方で費用は ±30% 程度動く (同じ課題の単発で $0.04〜0.13)。

## この実験で見つけて直した kenning の穴

1 回目の答えを正解と突き合わせて見つけた。`kenning tests Sleep::reset` は実行時の正解 35 本のうち 24 本しか
挙げていなかった (agent は grep で補って 100% に届いていた)。

- `callers Type::name` (修飾名) が名前一致を修飾名のまま引いて、候補を黙って 0 件にしていた。
- 同名の自由関数が別 file にあると、同じ file の helper (`tmp_dir()` / `walk()`) の呼び出しまで「同名複数」で
  確定しなかった — 自分の module の item が優先される Rust の規則どおりに直した。enchudb の bake 済み確定率が
  80.5% → 89.5% (同名複数 2,572 → 25)。
- `time::sleep` の module を file の置き場所 (`src/time/sleep.rs`) から見ていなかった。
- 自由関数の戻り値と `Box::pin(x)` / `.as_mut()` 越しの受け手を推定していなかった。
- `callers` に `path:` / `crate:` の絞り込みが無く、同名の自由関数を絞る手段が無かった。

どの修正も rust-analyzer との突き合わせ (`kenning bench infer`) で誤確定 0 を 4 repo で保ったまま入れた。
残る 11 本は、generic な `tokio_test::task::spawn(..).into_inner()` / `tokio::pin!` 越しの受け手と、trait 経由
(`Stream::poll_next`) の呼び出し — 静的な呼び出しグラフの外。

## 再現

```sh
./bench/corpus.sh                                   # tokio / ripgrep / enchudb (固定 commit)
(cd ~/.cache/kenning-bench/tokio && kenning bake)
(cd ~/.cache/kenning-bench/enchudb && KENNING_BAKE_DEFAULT_FEATURES=1 kenning bake)
AB_REPS=3 AB_CONDS="A B C" ./bench/agent-ab.sh      # 54 run、1 回 ~$10 (2026-09-28)
```
