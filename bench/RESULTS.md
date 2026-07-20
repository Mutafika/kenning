# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 722 files / 28834 call-sites / 解決率 35.5% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| get | 192 | 12 | 76 | 104 |
| open | 115 | 14 | 23 | 78 |
| store | 110 | 2 | 97 | 11 |
| acquire | 72 | 31 | 14 | 27 |
| insert_at | 65 | 63 | 0 | 2 |
| borrow_mut | 62 | 0 | 57 | 5 |
| block_in_place | 55 | 1 | 38 | 16 |
| elapsed | 42 | 3 | 8 | 31 |
| async_io | 28 | 20 | 3 | 5 |
| ptr_eq | 27 | 2 | 11 | 14 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 2 + 候補 0 = 検討対象 3 行、ノイズ率 43%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers sleep | 115126 | 69 | 11544 | 1 | 10.0x |
| callers registration | 27451 | 10 | 5495 | 1 | 5.0x |
| callers shared | 15458 | 3 | 6998 | 1 | 2.2x |
| callers enable_all | 44220 | 38 | 5352 | 1 | 8.3x |
| callers insert_at | 12129 | 4 | 6150 | 1 | 2.0x |
| callers worker_threads | 42209 | 37 | 5473 | 1 | 7.7x |
| callers unbounded_channel | 23759 | 17 | 7245 | 1 | 3.3x |
| callers run_one | 6149 | 3 | 5024 | 1 | 1.2x |
| callers changed | 23694 | 13 | 4268 | 1 | 5.6x |
| callers measure | 7381 | 3 | 5161 | 1 | 1.4x |
| callers asyncify | 19670 | 25 | 3152 | 1 | 6.2x |
| callers push_front | 18940 | 12 | 2803 | 1 | 6.8x |
| callers filled | 27113 | 19 | 2549 | 1 | 10.6x |
| callers put_slice | 32400 | 23 | 9111 | 1 | 3.6x |
| callers sleep_until | 24500 | 16 | 4634 | 1 | 5.3x |
| callers remaining | 25974 | 18 | 2809 | 1 | 9.2x |
| callers run_until | 11344 | 6 | 2616 | 1 | 4.3x |
| callers assume_init | 23654 | 15 | 3196 | 1 | 7.4x |
| callers child_token | 6603 | 4 | 3037 | 1 | 2.2x |
| callers socketpair | 4434 | 3 | 2625 | 1 | 1.7x |

**中央値**: 圧縮比 **5.3x**、grep 経路の tool 呼び出し 15 回 → 1 回

**単発 wall-clock 中央値**: rg 19.9ms vs kenning 13.0ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers sleep | 152 | 19908 | 392 | 105+47 | 11544 | 11.7 |
| callers registration | 89 | 16295 | 526 | 89+0 | 5495 | 14.8 |
| callers shared | 66 | 10598 | 369 | 66+0 | 6998 | 10.8 |
| callers enable_all | 64 | 15356 | 448 | 63+0 | 5352 | 12.1 |
| callers insert_at | 63 | 8640 | 372 | 63+0 | 6150 | 13.4 |
| callers worker_threads | 56 | 14041 | 448 | 56+0 | 5473 | 12.6 |
| callers unbounded_channel | 30 | 3753 | 465 | 52+7 | 7245 | 13.3 |
| callers run_one | 45 | 4251 | 338 | 45+0 | 5024 | 11.7 |
| callers changed | 42 | 4909 | 467 | 42+0 | 4268 | 15.4 |
| callers measure | 42 | 4695 | 569 | 42+0 | 5161 | 11.1 |
| callers asyncify | 32 | 4700 | 561 | 32+0 | 3152 | 20.3 |
| callers push_front | 24 | 3249 | 623 | 24+0 | 2803 | 14.3 |
| callers filled | 23 | 2788 | 415 | 23+0 | 2549 | 10.4 |
| callers put_slice | 74 | 7978 | 353 | 23+51 | 9111 | 11.3 |
| callers sleep_until | 39 | 5267 | 522 | 23+16 | 4634 | 11.7 |
| callers remaining | 24 | 2999 | 394 | 22+2 | 2809 | 13.0 |
| callers run_until | 22 | 25381 | 404 | 22+0 | 2616 | 14.7 |
| callers assume_init | 27 | 3653 | 349 | 21+6 | 3196 | 12.5 |
| callers child_token | 21 | 2944 | 423 | 21+0 | 3037 | 14.8 |
| callers socketpair | 22 | 2820 | 628 | 21+1 | 2625 | 13.9 |

**中央値**: ast-grep 448ms / 4909B vs kenning 13.0ms / 5024B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact sleep | 74 | 360312 | 321 | 7808 | 46x |
| impact registration | 200 | 1685896 | 1022 | 15078 | 112x |
| impact shared | 24 | 111162 | 80 | 3814 | 29x |
| impact enable_all | 138 | 1819666 | 1171 | 14732 | 124x |
| impact insert_at | 40 | 135426 | 123 | 6015 | 23x |

中央値: **46x**、tool 呼び出し 321 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Debug | 138 | 116380 | 100 | 24.9x |
| impls Drop | 91 | 111050 | 89 | 22.9x |
| impls Future | 73 | 119257 | 94 | 26.0x |
| impls Sync | 59 | 58477 | 45 | 12.8x |
| impls Stream | 55 | 83026 | 70 | 17.3x |

中央値: **22.9x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| named_pipe.rs | 99154 | 12061 | 8x |
| udp.rs | 84068 | 12051 | 7x |
| bounded.rs | 64865 | 10622 | 6x |
| builder.rs | 59923 | 6640 | 9x |
| async_fd.rs | 58959 | 11710 | 5x |

中央値: **7x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1640 B / 2 回 → def 241 B / 1 回 = **6.3x**

**faceted** (`kind:method vis:pub test:0`): 1300 件 315.458µs — grep では表現不能 (比較なし、能力差)

### micro — warm latency
```
open(readonly): 215.459µs
index: 722 files / 7156 symbols / 28834 call-sites
  kind=fn                                              =    2300 件  [708ns]
  pub fn                                               =      89 件  [4.541µs]
  pub async fn 非test                                   =      38 件  [4.958µs]
  def new                                              =     271 件  [208ns]
  callers new (名前一致)                                   =    2242 件  [708ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 100 files / 12268 call-sites / 解決率 48.7% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| parse_low_raw | 546 | 545 | 0 | 1 |
| create | 447 | 4 | 5 | 438 |
| path | 330 | 268 | 22 | 40 |
| line_number | 165 | 158 | 0 | 7 |
| map | 138 | 14 | 120 | 4 |
| doc_short | 108 | 2 | 0 | 106 |
| create_dir | 100 | 1 | 0 | 99 |
| end | 82 | 55 | 25 | 2 |
| get | 63 | 17 | 36 | 10 |
| create_bytes | 47 | 1 | 0 | 46 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers parse_low_raw | 77308 | 3 | 5276 | 1 | 14.7x |
| callers expected_no_line_number | 9823 | 3 | 5729 | 1 | 1.7x |
| callers test | 8847 | 3 | 5710 | 1 | 1.5x |
| callers unwrap_switch | 12273 | 3 | 4217 | 1 | 2.9x |
| callers add_child | 8416 | 3 | 4278 | 1 | 2.0x |
| callers assert_paths | 5207 | 2 | 3368 | 1 | 1.5x |
| callers unwrap_value | 6380 | 3 | 3457 | 1 | 1.8x |
| callers as_byte | 16373 | 10 | 3403 | 1 | 4.8x |
| callers push_token | 4411 | 2 | 2395 | 1 | 1.8x |
| callers with_path | 8391 | 5 | 2316 | 1 | 3.6x |
| callers expected_with_line_number | 5363 | 3 | 2357 | 1 | 2.3x |
| callers pos | 5818 | 4 | 2569 | 1 | 2.3x |
| callers analysis | 2759 | 2 | 1559 | 1 | 1.8x |
| callers wtr | 3538 | 2 | 1792 | 1 | 2.0x |
| callers with_depth | 4531 | 3 | 1486 | 1 | 3.0x |
| callers set_pos | 4060 | 3 | 1549 | 1 | 2.6x |
| callers str | 2820 | 2 | 1306 | 1 | 2.2x |
| callers build_glob_set | 2558 | 2 | 1308 | 1 | 2.0x |
| callers heap_limit | 7872 | 5 | 1389 | 1 | 5.7x |
| callers nice_err | 2567 | 2 | 1063 | 1 | 2.4x |

**中央値**: 圧縮比 **2.3x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 11.3ms vs kenning 6.5ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers parse_low_raw | 545 | 79818 | 177 | 545+0 | 5276 | 7.3 |
| callers expected_no_line_number | 55 | 27283 | 163 | 55+0 | 5729 | 6.4 |
| callers test | 55 | 35978 | 194 | 55+0 | 5710 | 6.6 |
| callers unwrap_switch | 39 | 4729 | 173 | 39+0 | 4217 | 6.6 |
| callers add_child | 38 | 6798 | 157 | 38+0 | 4278 | 6.4 |
| callers assert_paths | 33 | 13354 | 141 | 33+0 | 3368 | 6.9 |
| callers unwrap_value | 32 | 4183 | 167 | 32+0 | 3457 | 6.5 |
| callers as_byte | 27 | 3798 | 150 | 27+0 | 3403 | 6.5 |
| callers push_token | 21 | 2715 | 148 | 21+0 | 2395 | 6.4 |
| callers with_path | 20 | 3298 | 177 | 20+0 | 2316 | 6.6 |
| callers expected_with_line_number | 19 | 11701 | 157 | 19+0 | 2357 | 8.1 |
| callers pos | 19 | 2698 | 195 | 18+1 | 2569 | 6.6 |
| callers analysis | 15 | 1623 | 155 | 15+0 | 1559 | 6.5 |
| callers wtr | 13 | 1755 | 163 | 13+0 | 1792 | 6.1 |
| callers with_depth | 12 | 2744 | 143 | 12+0 | 1486 | 6.4 |
| callers set_pos | 11 | 1399 | 132 | 11+0 | 1549 | 6.4 |
| callers str | 11 | 1430 | 167 | 11+0 | 1306 | 6.0 |
| callers build_glob_set | 10 | 1326 | 142 | 10+0 | 1308 | 6.0 |
| callers heap_limit | 10 | 2039 | 138 | 10+0 | 1389 | 5.8 |
| callers nice_err | 10 | 1177 | 144 | 10+0 | 1063 | 6.3 |

**中央値**: ast-grep 157ms / 3298B vs kenning 6.5ms / 2395B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact parse_low_raw | 105 | 209653 | 211 | 6412 | 33x |
| impact expected_no_line_number | 34 | 62326 | 75 | 4778 | 13x |
| impact test | 34 | 61350 | 75 | 4759 | 13x |
| impact unwrap_switch | 41 | 36201 | 8 | 5160 | 7x |
| impact add_child | 57 | 185686 | 159 | 7135 | 26x |

中央値: **13x**、tool 呼び出し 75 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Flag | 104 | 15890 | 4 | 3.5x |
| impls Default | 24 | 23090 | 17 | 10.3x |
| impls Display | 17 | 20174 | 15 | 13.0x |
| impls Error | 11 | 19352 | 13 | 10.3x |
| impls Serialize | 10 | 6095 | 5 | 6.8x |

中央値: **10.3x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| defs.rs | 235436 | 7947 | 30x |
| standard.rs | 136288 | 12501 | 11x |
| walk.rs | 88407 | 8941 | 10x |
| glob.rs | 60771 | 9829 | 6x |
| dir.rs | 58199 | 10818 | 5x |

中央値: **10x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1482 B / 2 回 → def 247 B / 1 回 = **6.2x**

**faceted** (`kind:method vis:pub test:0`): 477 件 140.583µs — grep では表現不能 (比較なし、能力差)

### micro — warm latency
```
open(readonly): 235.167µs
index: 100 files / 3192 symbols / 12268 call-sites
  kind=fn                                              =     705 件  [250ns]
  pub fn                                               =      30 件  [1.458µs]
  pub async fn 非test                                   =       0 件  [166ns]
  def new                                              =      84 件  [208ns]
  callers new (名前一致)                                   =    1073 件  [416ns]
  callers new (確実 逆引き)                                 =       3 件  [125ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 175 files / 26131 call-sites / 解決率 39.8% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| tmp | 350 | 172 | 145 | 33 |
| contains | 280 | 0 | 54 | 226 |
| set | 268 | 237 | 9 | 22 |
| create | 118 | 89 | 19 | 10 |
| oplog_sync | 91 | 84 | 0 | 7 |
| make_eid | 83 | 68 | 0 | 15 |
| max | 78 | 3 | 45 | 30 |
| tie_async | 75 | 71 | 0 | 4 |
| merge | 52 | 0 | 46 | 6 |
| publish_since | 42 | 36 | 0 | 6 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 3 + 候補 0 = 検討対象 4 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 102038 | 47 | 4784 | 1 | 21.3x |
| callers define_himo | 135464 | 68 | 4895 | 1 | 27.7x |
| callers clone | 191952 | 76 | 11198 | 1 | 17.1x |
| callers create_standalone | 93788 | 47 | 4955 | 1 | 18.9x |
| callers number | 39495 | 19 | 5770 | 1 | 6.8x |
| callers eid_local | 51232 | 23 | 5035 | 1 | 10.2x |
| callers flush_writes | 64260 | 37 | 5939 | 1 | 10.8x |
| callers entity_in | 59139 | 29 | 6409 | 1 | 9.2x |
| callers tie_text | 54113 | 29 | 5254 | 1 | 10.3x |
| callers open_concurrent_with_oplog | 60610 | 32 | 5518 | 1 | 11.0x |
| callers define_table | 56963 | 30 | 6209 | 1 | 9.2x |
| callers open_standalone | 60133 | 34 | 5863 | 1 | 10.3x |
| callers define_himo_in | 61281 | 31 | 6013 | 1 | 10.2x |
| callers oplog_sync | 58437 | 34 | 5664 | 1 | 10.3x |
| callers oplog_commit | 50484 | 30 | 6257 | 1 | 8.1x |
| callers create_with_capacity | 47044 | 25 | 5958 | 1 | 7.9x |
| callers tie_async | 33669 | 19 | 4995 | 1 | 6.7x |
| callers make_eid | 42627 | 21 | 5405 | 1 | 7.9x |
| callers tie_to | 31923 | 17 | 6692 | 1 | 4.8x |
| callers append | 19174 | 8 | 5582 | 1 | 3.4x |

**中央値**: 圧縮比 **10.2x**、grep 経路の tool 呼び出し 30 回 → 1 回

**単発 wall-clock 中央値**: rg 11.0ms vs kenning 6.6ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 338 | 33620 | 209 | 338+0 | 4784 | 6.4 |
| callers define_himo | 324 | 36927 | 268 | 324+0 | 4895 | 6.5 |
| callers clone | 622 | 60718 | 235 | 224+398 | 11198 | 7.2 |
| callers create_standalone | 217 | 27302 | 227 | 217+0 | 4955 | 6.5 |
| callers number | 114 | 19241 | 217 | 124+0 | 5770 | 6.8 |
| callers eid_local | 119 | 16745 | 247 | 119+0 | 5035 | 7.0 |
| callers flush_writes | 117 | 10663 | 236 | 117+0 | 5939 | 6.6 |
| callers entity_in | 114 | 14342 | 234 | 114+0 | 6409 | 6.4 |
| callers tie_text | 95 | 11448 | 246 | 95+0 | 5254 | 6.7 |
| callers open_concurrent_with_oplog | 91 | 13281 | 228 | 91+0 | 5518 | 8.3 |
| callers define_table | 87 | 10839 | 213 | 87+0 | 6209 | 6.6 |
| callers open_standalone | 85 | 10799 | 211 | 85+0 | 5863 | 6.6 |
| callers define_himo_in | 84 | 12734 | 241 | 84+0 | 6013 | 6.6 |
| callers oplog_sync | 84 | 8319 | 245 | 84+0 | 5664 | 6.8 |
| callers oplog_commit | 82 | 7754 | 244 | 82+0 | 6257 | 6.6 |
| callers create_with_capacity | 81 | 11231 | 229 | 81+0 | 5958 | 6.2 |
| callers tie_async | 71 | 7125 | 209 | 71+0 | 4995 | 6.5 |
| callers make_eid | 68 | 9135 | 229 | 68+0 | 5405 | 6.4 |
| callers tie_to | 68 | 7927 | 203 | 68+0 | 6692 | 6.1 |
| callers append | 66 | 11383 | 217 | 64+2 | 5582 | 6.2 |

**中央値**: ast-grep 229ms / 11448B vs kenning 6.6ms / 5770B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 307 | 912983 | 808 | 18606 | 49x |
| impact define_himo | 343 | 985599 | 866 | 18706 | 53x |
| impact clone | 77 | 323987 | 228 | 6259 | 52x |
| impact create_standalone | 417 | 1130816 | 1070 | 18149 | 62x |
| impact number | 76 | 195890 | 192 | 9158 | 21x |

中央値: **52x**、tool 呼び出し 808 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Sync | 16 | 26640 | 16 | 20.0x |
| impls Send | 15 | 27045 | 16 | 21.5x |
| impls Drop | 14 | 17682 | 12 | 15.0x |
| impls Default | 11 | 14061 | 12 | 15.8x |
| impls From | 9 | 6154 | 5 | 8.9x |

中央値: **15.8x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 442565 | 10446 | 42x |
| lib.rs | 118459 | 9708 | 12x |
| oplog.rs | 82256 | 7148 | 12x |
| lib.rs | 69756 | 8688 | 8x |
| dist_dashboard.rs | 47651 | 5994 | 8x |

中央値: **12x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2373 B / 2 回 → def 221 B / 1 回 = **9.7x**

**faceted** (`kind:method vis:pub test:0`): 823 件 171.875µs — grep では表現不能 (比較なし、能力差)

### micro — warm latency
```
open(readonly): 198.542µs
index: 175 files / 2905 symbols / 26131 call-sites
  kind=fn                                              =    1395 件  [416ns]
  pub fn                                               =      42 件  [2.125µs]
  pub async fn 非test                                   =       0 件  [250ns]
  def new                                              =      48 件  [166ns]
  callers new (名前一致)                                   =    1496 件  [458ns]
  callers new (確実 逆引き)                                 =      10 件  [83ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 2 files / 2418 call-sites / 解決率 13.5% / syn-only (未 bake)

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| txt | 41 | 37 | 0 | 4 |
| num | 29 | 23 | 0 | 6 |
| next | 28 | 23 | 0 | 5 |
| drop | 16 | 15 | 0 | 1 |
| ref_of | 16 | 15 | 0 | 1 |
| parse_opts | 14 | 13 | 0 | 1 |
| open_ro | 13 | 12 | 0 | 1 |
| file_paths | 12 | 11 | 0 | 1 |
| line_of | 11 | 10 | 0 | 1 |
| col_of | 10 | 9 | 0 | 1 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 1 + 候補 0 = 検討対象 1 行、ノイズ率 50%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers txt | 6681 | 2 | 2848 | 1 | 2.3x |
| callers next | 6847 | 3 | 1822 | 1 | 3.8x |
| callers num | 5702 | 2 | 1839 | 1 | 3.1x |
| callers drop | 2988 | 2 | 1340 | 1 | 2.2x |
| callers ref_of | 4100 | 2 | 1283 | 1 | 3.2x |
| callers parse_opts | 3009 | 2 | 1128 | 1 | 2.7x |
| callers open_ro | 2887 | 2 | 1058 | 1 | 2.7x |
| callers file_paths | 2693 | 2 | 995 | 1 | 2.7x |
| callers line_of | 2972 | 2 | 953 | 1 | 3.1x |
| callers col_of | 2867 | 2 | 882 | 1 | 3.3x |
| callers first_doc_line | 2124 | 2 | 705 | 1 | 3.0x |
| callers record_symbol | 1891 | 2 | 704 | 1 | 2.7x |
| callers classify_vis | 2903 | 2 | 634 | 1 | 4.6x |
| callers read_meta | 2600 | 2 | 662 | 1 | 3.9x |
| callers rust_files | 2584 | 2 | 684 | 1 | 3.8x |
| callers timed | 2560 | 2 | 647 | 1 | 4.0x |
| callers defs_of | 2479 | 2 | 572 | 1 | 4.3x |
| callers cs_run_bytes | 2296 | 2 | 516 | 1 | 4.4x |
| callers now_secs | 2248 | 2 | 511 | 1 | 4.4x |
| callers repo_root_of | 2279 | 2 | 510 | 1 | 4.5x |

**中央値**: 圧縮比 **3.3x**、grep 経路の tool 呼び出し 2 回 → 1 回

**単発 wall-clock 中央値**: rg 7.6ms vs kenning 4.0ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers txt | 37 | 4181 | 75 | 37+0 | 2848 | 3.1 |
| callers next | 23 | 3062 | 108 | 23+0 | 1822 | 10.4 |
| callers num | 23 | 2619 | 75 | 23+0 | 1839 | 3.6 |
| callers drop | 15 | 1100 | 70 | 15+0 | 1340 | 4.6 |
| callers ref_of | 15 | 2176 | 72 | 15+0 | 1283 | 3.3 |
| callers parse_opts | 13 | 1137 | 71 | 13+0 | 1128 | 3.2 |
| callers open_ro | 12 | 1248 | 70 | 12+0 | 1058 | 4.1 |
| callers file_paths | 11 | 991 | 72 | 11+0 | 995 | 3.9 |
| callers line_of | 10 | 1403 | 72 | 10+0 | 953 | 3.1 |
| callers col_of | 9 | 1290 | 75 | 9+0 | 882 | 3.3 |
| callers first_doc_line | 7 | 662 | 143 | 7+0 | 705 | 4.4 |
| callers record_symbol | 7 | 4696 | 181 | 7+0 | 704 | 8.6 |
| callers classify_vis | 6 | 890 | 144 | 6+0 | 634 | 7.2 |
| callers read_meta | 6 | 613 | 102 | 6+0 | 662 | 4.0 |
| callers rust_files | 6 | 567 | 91 | 6+0 | 684 | 3.9 |
| callers timed | 6 | 1558 | 88 | 6+0 | 647 | 5.2 |
| callers defs_of | 5 | 532 | 83 | 5+0 | 572 | 3.9 |
| callers cs_run_bytes | 4 | 466 | 81 | 4+0 | 516 | 4.0 |
| callers now_secs | 4 | 584 | 81 | 4+0 | 511 | 4.1 |
| callers repo_root_of | 4 | 422 | 82 | 4+0 | 510 | 3.9 |

**中央値**: ast-grep 81ms / 1137B vs kenning 4.0ms / 882B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact txt | 46 | 145469 | 109 | 4307 | 34x |
| impact next | 43 | 136812 | 105 | 4034 | 34x |
| impact num | 41 | 131899 | 99 | 3886 | 34x |
| impact drop | 24 | 92028 | 65 | 2358 | 39x |
| impact ref_of | 26 | 87398 | 65 | 2499 | 35x |

中央値: **34x**、tool 呼び出し 99 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| kenning.rs | 152494 | 7817 | 20x |
| main.rs | 6814 | 136 | 50x |

中央値: **50x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1615 B / 2 回 → def 149 B / 1 回 = **10.2x**

**faceted** (`kind:method vis:pub test:0`): 0 件 138.209µs — grep では表現不能 (比較なし、能力差)

### micro — warm latency
```
open(readonly): 486.125µs
index: 2 files / 144 symbols / 2418 call-sites
  kind=fn                                              =     104 件  [166ns]
  pub fn                                               =      20 件  [291ns]
  pub async fn 非test                                   =       0 件  [166ns]
  def txt                                              =       1 件  [125ns]
  callers txt (名前一致)                                   =      37 件  [166ns]
  callers txt (確実 逆引き)                                 =      37 件  [125ns]
```

