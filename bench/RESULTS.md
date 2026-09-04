# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 770 files / 28834 call-sites / 解決率 35.5% / **baked (SCIP)**

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
| callers sleep | 115856 | 70 | 15063 | 1 | 7.7x |
| callers registration | 27112 | 10 | 6594 | 1 | 4.1x |
| callers shared | 15239 | 3 | 9282 | 1 | 1.6x |
| callers enable_all | 43935 | 38 | 5934 | 1 | 7.4x |
| callers insert_at | 11934 | 4 | 8262 | 1 | 1.4x |
| callers worker_threads | 41996 | 37 | 6380 | 1 | 6.6x |
| callers unbounded_channel | 23621 | 17 | 9855 | 1 | 2.4x |
| callers run_one | 6011 | 3 | 5651 | 1 | 1.1x |
| callers changed | 23502 | 13 | 5615 | 1 | 4.2x |
| callers measure | 7252 | 3 | 5821 | 1 | 1.2x |
| callers asyncify | 19574 | 25 | 4654 | 1 | 4.2x |
| callers push_front | 18865 | 12 | 3551 | 1 | 5.3x |
| callers filled | 26987 | 19 | 3312 | 1 | 8.1x |
| callers put_slice | 34037 | 24 | 11032 | 1 | 3.1x |
| callers sleep_until | 24365 | 16 | 6599 | 1 | 3.7x |
| callers remaining | 25863 | 18 | 3845 | 1 | 6.7x |
| callers run_until | 11254 | 6 | 3099 | 1 | 3.6x |
| callers assume_init | 23564 | 15 | 4361 | 1 | 5.4x |
| callers child_token | 6534 | 4 | 3866 | 1 | 1.7x |
| callers socketpair | 4362 | 3 | 3207 | 1 | 1.4x |

**中央値**: 圧縮比 **4.1x**、grep 経路の tool 呼び出し 15 回 → 1 回

**単発 wall-clock 中央値**: rg 18.2ms vs kenning 13.9ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers sleep | 152 | 19443 | 218 | 105+47 | 15063 | 11.5 |
| callers registration | 89 | 15839 | 240 | 89+0 | 6594 | 11.9 |
| callers shared | 66 | 10400 | 365 | 66+0 | 9282 | 14.7 |
| callers enable_all | 64 | 14951 | 263 | 63+0 | 5934 | 15.0 |
| callers insert_at | 63 | 8442 | 247 | 63+0 | 8262 | 13.3 |
| callers worker_threads | 56 | 13681 | 332 | 56+0 | 6380 | 15.0 |
| callers unbounded_channel | 30 | 3663 | 250 | 52+7 | 9855 | 14.6 |
| callers run_one | 45 | 4116 | 249 | 45+0 | 5651 | 13.8 |
| callers changed | 42 | 4783 | 335 | 42+0 | 5615 | 14.3 |
| callers measure | 42 | 4569 | 234 | 42+0 | 5821 | 13.9 |
| callers asyncify | 32 | 4586 | 255 | 32+0 | 4654 | 13.6 |
| callers push_front | 24 | 3171 | 243 | 24+0 | 3551 | 31.8 |
| callers filled | 23 | 2719 | 217 | 23+0 | 3312 | 13.8 |
| callers put_slice | 74 | 7756 | 220 | 23+51 | 11032 | 13.1 |
| callers sleep_until | 39 | 5150 | 251 | 23+16 | 6599 | 14.4 |
| callers remaining | 24 | 2930 | 247 | 22+2 | 3845 | 13.9 |
| callers run_until | 22 | 24679 | 255 | 22+0 | 3099 | 13.8 |
| callers assume_init | 27 | 3572 | 250 | 21+6 | 4361 | 15.1 |
| callers child_token | 21 | 2881 | 258 | 21+0 | 3866 | 13.6 |
| callers socketpair | 22 | 2739 | 269 | 21+1 | 3207 | 13.5 |

**中央値**: ast-grep 250ms / 4783B vs kenning 13.9ms / 5821B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact sleep | 74 | 365365 | 325 | 7643 | 48x |
| impact registration | 200 | 1698553 | 1038 | 14718 | 115x |
| impact shared | 24 | 110445 | 80 | 3742 | 30x |
| impact enable_all | 138 | 1815184 | 1180 | 14387 | 126x |
| impact insert_at | 40 | 136135 | 124 | 5895 | 23x |

中央値: **48x**、tool 呼び出し 325 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Debug | 138 | 117626 | 101 | 26.0x |
| impls Drop | 91 | 110618 | 89 | 23.6x |
| impls Future | 73 | 119429 | 95 | 26.9x |
| impls Sync | 59 | 59277 | 46 | 13.5x |
| impls Stream | 55 | 85591 | 72 | 18.4x |

中央値: **23.6x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| CHANGELOG.md | 143415 | 4859 | 30x |
| named_pipe.rs | 99154 | 11908 | 8x |
| udp.rs | 84068 | 11898 | 7x |
| bounded.rs | 64865 | 10469 | 6x |
| builder.rs | 59923 | 6553 | 9x |

中央値: **8x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1637 B / 2 回 → def 238 B / 1 回 = **6.4x**

**faceted** (`kind:method vis:pub test:0`): 1300 件 290.541µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 15 | 319769 | 2701 | 24 | 374607 | = |
| unwrap | 2686 | 16 | 318940 | 2686 | 24 | 367798 | = |
| Result | 2839 | 17 | 380851 | 2839 | 39 | 435412 | = |
| runtime | 2482 | 25 | 327851 | 2482 | 23 | 370248 | = |
| stream | 3200 | 16 | 415027 | 3200 | 22 | 476035 | = |
| github | 1571 | 17 | 189393 | 1571 | 16 | 248234 | = |
| assert | 6693 | 17 | 805288 | 6693 | 30 | 932314 | = |
| future | 2545 | 15 | 328368 | 2545 | 23 | 368578 | = |
| return | 3257 | 14 | 443672 | 3257 | 26 | 507918 | = |
| handle | 2879 | 18 | 378121 | 2879 | 24 | 427659 | = |
| thread | 2868 | 14 | 380972 | 2868 | 23 | 428817 | = |
| Context | 1584 | 18 | 213427 | 1584 | 24 | 240731 | = |
| feature | 1227 | 16 | 148931 | 1227 | 20 | 161248 | = |
| struct | 1414 | 19 | 166367 | 1414 | 24 | 190203 | = |
| channel | 1280 | 17 | 165719 | 1280 | 21 | 194170 | = |
| method | 1186 | 15 | 173192 | 1186 | 20 | 197761 | = |
| socket | 1847 | 16 | 237089 | 1847 | 17 | 274055 | = |
| buffer | 1225 | 15 | 166630 | 1225 | 18 | 191065 | = |
| unsafe | 1067 | 16 | 136843 | 1067 | 18 | 151971 | = |
| function | 1070 | 16 | 153227 | 1070 | 19 | 178654 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 16.2ms vs text 22.9ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 855.041µs
index: 770 files / 7156 symbols / 28834 call-sites
  kind=fn                                              =    2300 件  [541ns]
  pub fn                                               =      89 件  [3.5µs]
  pub async fn 非test                                   =      38 件  [3.791µs]
  def new                                              =     271 件  [166ns]
  callers new (名前一致)                                   =    2242 件  [583ns]
  callers new (確実 逆引き)                                 =       1 件  [83ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 207 files / 12268 call-sites / 解決率 48.7% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| parse_low_raw | 546 | 545 | 0 | 1 |
| create | 447 | 4 | 5 | 438 |
| path | 333 | 268 | 22 | 43 |
| line_number | 165 | 158 | 0 | 7 |
| map | 139 | 14 | 120 | 5 |
| doc_short | 108 | 2 | 0 | 106 |
| create_dir | 100 | 1 | 0 | 99 |
| end | 83 | 55 | 25 | 3 |
| get | 64 | 17 | 36 | 11 |
| create_bytes | 47 | 1 | 0 | 46 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers parse_low_raw | 75670 | 3 | 7995 | 1 | 9.5x |
| callers expected_no_line_number | 9655 | 3 | 7083 | 1 | 1.4x |
| callers test | 8679 | 3 | 6007 | 1 | 1.4x |
| callers unwrap_switch | 12063 | 3 | 5594 | 1 | 2.2x |
| callers add_child | 8302 | 3 | 6312 | 1 | 1.3x |
| callers assert_paths | 5105 | 2 | 4399 | 1 | 1.2x |
| callers unwrap_value | 6281 | 3 | 4873 | 1 | 1.3x |
| callers as_byte | 16289 | 10 | 4570 | 1 | 3.6x |
| callers push_token | 4345 | 2 | 3151 | 1 | 1.4x |
| callers with_path | 8331 | 5 | 3319 | 1 | 2.5x |
| callers expected_with_line_number | 5303 | 3 | 3009 | 1 | 1.8x |
| callers pos | 5755 | 4 | 3374 | 1 | 1.7x |
| callers analysis | 2711 | 2 | 1934 | 1 | 1.4x |
| callers wtr | 3490 | 2 | 2341 | 1 | 1.5x |
| callers with_depth | 4492 | 3 | 2078 | 1 | 2.2x |
| callers set_pos | 4024 | 3 | 1833 | 1 | 2.2x |
| callers str | 4592 | 3 | 1789 | 1 | 2.6x |
| callers build_glob_set | 2525 | 2 | 1771 | 1 | 1.4x |
| callers heap_limit | 7839 | 5 | 1825 | 1 | 4.3x |
| callers nice_err | 2537 | 2 | 1505 | 1 | 1.7x |

**中央値**: 圧縮比 **1.7x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 10.7ms vs kenning 8.3ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers parse_low_raw | 545 | 77997 | 119 | 545+0 | 7995 | 7.1 |
| callers expected_no_line_number | 55 | 26623 | 116 | 55+0 | 7083 | 6.8 |
| callers test | 55 | 35090 | 169 | 55+0 | 6007 | 7.1 |
| callers unwrap_switch | 39 | 4612 | 162 | 39+0 | 5594 | 8.9 |
| callers add_child | 38 | 6645 | 126 | 38+0 | 6312 | 8.5 |
| callers assert_paths | 33 | 12988 | 149 | 33+0 | 4399 | 14.5 |
| callers unwrap_value | 32 | 4087 | 179 | 32+0 | 4873 | 9.0 |
| callers as_byte | 27 | 3717 | 178 | 27+0 | 4570 | 10.5 |
| callers push_token | 21 | 2652 | 162 | 21+0 | 3151 | 8.3 |
| callers with_path | 20 | 3226 | 189 | 20+0 | 3319 | 8.3 |
| callers expected_with_line_number | 19 | 11422 | 137 | 19+0 | 3009 | 8.6 |
| callers pos | 19 | 2641 | 155 | 18+1 | 3374 | 8.7 |
| callers analysis | 15 | 1578 | 171 | 15+0 | 1934 | 10.4 |
| callers wtr | 13 | 1716 | 135 | 13+0 | 2341 | 8.3 |
| callers with_depth | 12 | 2681 | 132 | 12+0 | 2078 | 7.8 |
| callers set_pos | 11 | 1366 | 125 | 11+0 | 1833 | 7.3 |
| callers str | 11 | 1397 | 148 | 11+0 | 1789 | 7.6 |
| callers build_glob_set | 10 | 1296 | 115 | 10+0 | 1771 | 7.5 |
| callers heap_limit | 10 | 1994 | 128 | 10+0 | 1825 | 8.0 |
| callers nice_err | 10 | 1147 | 142 | 10+0 | 1505 | 8.1 |

**中央値**: ast-grep 148ms / 3226B vs kenning 8.3ms / 3319B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact parse_low_raw | 105 | 207703 | 211 | 6259 | 33x |
| impact expected_no_line_number | 34 | 62041 | 75 | 4676 | 13x |
| impact test | 34 | 61065 | 75 | 4657 | 13x |
| impact unwrap_switch | 41 | 35634 | 8 | 5037 | 7x |
| impact add_child | 57 | 191914 | 163 | 6964 | 28x |

中央値: **13x**、tool 呼び出し 75 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Flag | 104 | 15560 | 4 | 3.6x |
| impls Default | 24 | 23015 | 17 | 10.6x |
| impls Display | 17 | 20114 | 15 | 13.4x |
| impls Error | 11 | 19253 | 13 | 10.6x |
| impls Serialize | 10 | 6062 | 5 | 7.0x |

中央値: **10.6x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| defs.rs | 235436 | 7794 | 30x |
| raw.csv | 226710 | 106 | 2139x |
| standard.rs | 136288 | 12348 | 11x |
| raw.csv | 114939 | 109 | 1054x |
| raw.csv | 91922 | 107 | 859x |

中央値: **859x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1479 B / 2 回 → def 244 B / 1 回 = **6.2x**

**faceted** (`kind:method vis:pub test:0`): 477 件 174µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 9 | 856322 | 3298 | 11 | 746164 | = |
| ignore | 3475 | 9 | 566374 | 3473 | 12 | 585001 | ≠ |
| OpenSubtitles2016 | 2349 | 9 | 637772 | 2349 | 9 | 547110 | = |
| ripgrep | 1886 | 9 | 244360 | 1885 | 11 | 262354 | ≠ |
| benchsuite | 1896 | 9 | 507042 | 1896 | 9 | 440490 | = |
| Sherlock | 2133 | 9 | 421572 | 2139 | 10 | 383183 | ≠ |
| Holmes | 1679 | 9 | 396369 | 1688 | 10 | 349402 | ≠ |
| sample | 1580 | 8 | 406148 | 1580 | 9 | 343953 | = |
| Холмс | 1406 | 9 | 379069 | 1406 | 9 | 339776 | = |
| Шерлок | 1206 | 10 | 330816 | 1206 | 8 | 295258 | = |
| assert_eq | 1296 | 9 | 158656 | 1296 | 10 | 177179 | = |
| unwrap | 1306 | 10 | 165821 | 1306 | 11 | 185555 | = |
| LC_ALL | 1103 | 10 | 281795 | 1103 | 7 | 244529 | = |
| PM_RESUME | 969 | 9 | 175046 | 969 | 8 | 173294 | = |
| matcher | 1200 | 9 | 151245 | 1194 | 10 | 171829 | ≠ |
| github | 954 | 8 | 121749 | 886 | 10 | 116086 | ≠ |
| BurntSushi | 806 | 9 | 102017 | 806 | 8 | 103321 | = |
| matches | 972 | 9 | 123456 | 972 | 11 | 139052 | = |
| return | 1307 | 9 | 164960 | 1308 | 12 | 188021 | ≠ |
| pattern | 1002 | 10 | 143447 | 1002 | 11 | 155061 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.1ms vs text 10.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 605.083µs
index: 207 files / 3192 symbols / 12268 call-sites
  kind=fn                                              =     705 件  [208ns]
  pub fn                                               =      30 件  [1.458µs]
  pub async fn 非test                                   =       0 件  [125ns]
  def new                                              =      84 件  [166ns]
  callers new (名前一致)                                   =    1073 件  [333ns]
  callers new (確実 逆引き)                                 =       3 件  [125ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 295 files / 37022 call-sites / 解決率 43.2% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| cleanup | 861 | 752 | 0 | 109 |
| oplog_sync | 145 | 128 | 0 | 17 |
| with_capacity | 130 | 36 | 84 | 10 |
| filter | 119 | 8 | 99 | 12 |
| open_standalone | 108 | 105 | 0 | 3 |
| publish_since | 104 | 95 | 0 | 9 |
| as_bytes | 100 | 0 | 73 | 27 |
| value | 48 | 17 | 6 | 25 |
| vocab_id | 46 | 37 | 0 | 9 |
| remote_tie_apply | 35 | 15 | 0 | 20 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 140495 | 67 | 6118 | 1 | 23.0x |
| callers define_himo | 160026 | 79 | 7569 | 1 | 21.1x |
| callers entity_in | 156951 | 79 | 8589 | 1 | 18.3x |
| callers create_standalone | 106037 | 52 | 7843 | 1 | 13.5x |
| callers define_himo_in | 161746 | 84 | 9490 | 1 | 17.0x |
| callers define_table | 154109 | 83 | 8570 | 1 | 18.0x |
| callers flush_writes | 110881 | 64 | 7126 | 1 | 15.6x |
| callers eid_local | 70423 | 30 | 7636 | 1 | 9.2x |
| callers number | 63553 | 29 | 7752 | 1 | 8.2x |
| callers oplog_sync | 108003 | 62 | 7873 | 1 | 13.7x |
| callers tie_to | 82714 | 46 | 8298 | 1 | 10.0x |
| callers oplog_commit | 93794 | 56 | 7483 | 1 | 12.5x |
| callers open_concurrent_with_oplog | 79958 | 41 | 10030 | 1 | 8.0x |
| callers pull_once | 57695 | 28 | 8422 | 1 | 6.9x |
| callers open_standalone | 73729 | 41 | 8542 | 1 | 8.6x |
| callers tie_text | 66964 | 36 | 7655 | 1 | 8.7x |
| callers publish_since | 51797 | 25 | 8441 | 1 | 6.1x |
| callers transfer_oplog_to_sync_ops | 70834 | 38 | 7976 | 1 | 8.9x |
| callers remove_db | 73537 | 44 | 7669 | 1 | 9.6x |
| callers enable_sync_tables | 100171 | 53 | 8180 | 1 | 12.2x |

**中央値**: 圧縮比 **12.2x**、grep 経路の tool 呼び出し 52 回 → 1 回

**単発 wall-clock 中央値**: rg 11.5ms vs kenning 12.0ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 382 | 38098 | 308 | 382+0 | 6118 | 11.0 |
| callers define_himo | 350 | 40571 | 313 | 350+0 | 7569 | 11.6 |
| callers entity_in | 230 | 28501 | 313 | 230+0 | 8589 | 13.0 |
| callers create_standalone | 218 | 27457 | 307 | 218+0 | 7843 | 12.0 |
| callers define_himo_in | 181 | 27272 | 294 | 181+0 | 9490 | 13.0 |
| callers define_table | 177 | 21829 | 319 | 177+0 | 8570 | 12.3 |
| callers flush_writes | 166 | 16131 | 303 | 166+0 | 7126 | 11.9 |
| callers eid_local | 142 | 20070 | 303 | 142+0 | 7636 | 12.2 |
| callers number | 128 | 22251 | 283 | 138+0 | 7752 | 9.7 |
| callers oplog_sync | 128 | 13343 | 288 | 128+0 | 7873 | 12.4 |
| callers tie_to | 127 | 14971 | 272 | 127+0 | 8298 | 11.4 |
| callers oplog_commit | 117 | 11398 | 315 | 117+0 | 7483 | 10.4 |
| callers open_concurrent_with_oplog | 114 | 16909 | 294 | 114+0 | 10030 | 13.9 |
| callers pull_once | 108 | 11305 | 319 | 108+0 | 8422 | 10.9 |
| callers open_standalone | 105 | 13714 | 321 | 105+0 | 8542 | 14.9 |
| callers tie_text | 100 | 12269 | 340 | 100+0 | 7655 | 12.6 |
| callers publish_since | 95 | 11458 | 304 | 95+0 | 8441 | 10.9 |
| callers transfer_oplog_to_sync_ops | 84 | 9717 | 331 | 84+0 | 7976 | 13.6 |
| callers remove_db | 83 | 9772 | 275 | 83+0 | 7669 | 10.5 |
| callers enable_sync_tables | 82 | 9531 | 287 | 82+0 | 8180 | 11.4 |

**中央値**: ast-grep 307ms / 16131B vs kenning 12.0ms / 7976B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 362 | 1240989 | 1012 | 21699 | 57x |
| impact define_himo | 437 | 1444225 | 1179 | 20829 | 69x |
| impact entity_in | 603 | 3407034 | 2234 | 40353 | 84x |
| impact create_standalone | 451 | 2008456 | 1438 | 19369 | 104x |
| impact define_himo_in | 439 | 2036877 | 1384 | 23250 | 88x |

中央値: **84x**、tool 呼び出し 1384 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Drop | 17 | 19498 | 13 | 13.9x |
| impls Sync | 17 | 29036 | 17 | 20.4x |
| impls Send | 16 | 29130 | 17 | 21.6x |
| impls Default | 12 | 16571 | 13 | 17.0x |
| impls From | 10 | 8057 | 6 | 10.4x |

中央値: **17.0x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 829572 | 11610 | 71x |
| CHANGELOG.md | 368129 | 6496 | 57x |
| lib.rs | 133158 | 10031 | 13x |
| oplog.rs | 106684 | 7519 | 14x |
| sync.rs | 105249 | 10332 | 10x |

中央値: **14x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.7x**

**faceted** (`kind:method vis:pub test:0`): 1049 件 221.041µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 4641 | 11 | 588311 | 4641 | 20 | 708537 | = |
| assert_eq | 2122 | 11 | 264621 | 2122 | 15 | 329341 | = |
| Engine | 2793 | 11 | 387712 | 2787 | 17 | 472371 | ≠ |
| format | 1355 | 11 | 179180 | 1355 | 15 | 204033 | = |
| assert | 3574 | 11 | 448725 | 3568 | 17 | 564122 | ≠ |
| entity | 2234 | 10 | 303790 | 2234 | 15 | 372214 | = |
| return | 1042 | 11 | 124846 | 1042 | 12 | 141061 | = |
| cleanup | 876 | 10 | 82901 | 876 | 12 | 111948 | = |
| ValueType | 846 | 10 | 106174 | 846 | 12 | 123558 | = |
| enchudb | 2239 | 10 | 288666 | 2199 | 15 | 329404 | ≠ |
| author | 1160 | 10 | 170149 | 1160 | 12 | 215453 | = |
| engine | 2793 | 10 | 387712 | 2787 | 17 | 472371 | ≠ |
| enchudb_oplog | 793 | 11 | 105879 | 793 | 12 | 121694 | = |
| himo_id | 751 | 11 | 104080 | 751 | 12 | 120278 | = |
| record | 1411 | 12 | 205089 | 1411 | 13 | 253253 | = |
| Ordering | 676 | 10 | 89131 | 676 | 11 | 100478 | = |
| schema | 936 | 10 | 133715 | 923 | 12 | 159313 | ≠ |
| remove_file | 620 | 11 | 76292 | 620 | 13 | 87526 | = |
| String | 948 | 11 | 119279 | 936 | 14 | 134739 | ≠ |
| transport | 743 | 9 | 101927 | 740 | 11 | 127678 | ≠ |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 7 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 10.7ms vs text 12.9ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 776.5µs
index: 295 files / 4099 symbols / 37022 call-sites
  kind=fn                                              =    2080 件  [500ns]
  pub fn                                               =      74 件  [2.917µs]
  pub async fn 非test                                   =       0 件  [208ns]
  def new                                              =      50 件  [125ns]
  callers new (名前一致)                                   =    1965 件  [500ns]
  callers new (確実 逆引き)                                 =      10 件  [125ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 20 files / 4436 call-sites / 解決率 16.0% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| txt | 44 | 40 | 0 | 4 |
| query | 43 | 35 | 0 | 8 |
| index | 28 | 11 | 0 | 17 |
| ref_of | 22 | 21 | 0 | 1 |
| file_paths | 17 | 16 | 0 | 1 |
| tmp | 17 | 16 | 0 | 1 |
| write_fixture | 15 | 14 | 0 | 1 |
| tmp_tree | 12 | 11 | 0 | 1 |
| fmt_sym | 11 | 1 | 0 | 10 |
| acquire | 10 | 6 | 0 | 4 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers txt | 6843 | 2 | 4936 | 1 | 1.4x |
| callers num | 7395 | 2 | 5024 | 1 | 1.5x |
| callers query | 8231 | 3 | 5308 | 1 | 1.6x |
| callers next | 7222 | 3 | 3884 | 1 | 1.9x |
| callers drop | 3353 | 2 | 2570 | 1 | 1.3x |
| callers ref_of | 4822 | 2 | 3215 | 1 | 1.5x |
| callers open_ro | 3538 | 2 | 2123 | 1 | 1.7x |
| callers file_paths | 3043 | 2 | 1771 | 1 | 1.7x |
| callers parse_opts | 3149 | 2 | 1718 | 1 | 1.8x |
| callers tmp | 2272 | 2 | 2024 | 1 | 1.1x |
| callers write_fixture | 2447 | 2 | 2032 | 1 | 1.2x |
| callers index | 17301 | 7 | 1435 | 1 | 12.1x |
| callers kenning | 16876 | 8 | 1680 | 1 | 10.0x |
| callers tmp_tree | 2810 | 2 | 1556 | 1 | 1.8x |
| callers line_of | 3181 | 2 | 1682 | 1 | 1.9x |
| callers col_of | 3014 | 2 | 1578 | 1 | 1.9x |
| callers indexed_fixture | 3059 | 2 | 1605 | 1 | 1.9x |
| callers quiet | 3531 | 2 | 948 | 1 | 3.7x |
| callers read_meta | 2840 | 2 | 1290 | 1 | 2.2x |
| callers suggest_similar | 2699 | 2 | 1035 | 1 | 2.6x |

**中央値**: 圧縮比 **1.8x**、grep 経路の tool 呼び出し 2 回 → 1 回

**単発 wall-clock 中央値**: rg 6.5ms vs kenning 5.1ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers txt | 40 | 4361 | 109 | 40+0 | 4936 | 5.0 |
| callers num | 38 | 4048 | 110 | 38+0 | 5024 | 5.0 |
| callers query | 35 | 3478 | 109 | 35+0 | 5308 | 5.4 |
| callers next | 29 | 3603 | 105 | 29+0 | 3884 | 5.2 |
| callers drop | 24 | 1587 | 109 | 24+0 | 2570 | 5.0 |
| callers ref_of | 21 | 2834 | 107 | 21+0 | 3215 | 4.8 |
| callers open_ro | 17 | 1673 | 105 | 17+0 | 2123 | 5.7 |
| callers file_paths | 16 | 1340 | 106 | 16+0 | 1771 | 5.0 |
| callers parse_opts | 16 | 1289 | 107 | 16+0 | 1718 | 5.4 |
| callers tmp | 16 | 1045 | 107 | 16+0 | 2024 | 5.0 |
| callers write_fixture | 14 | 1150 | 89 | 14+0 | 2032 | 4.3 |
| callers index | 11 | 735 | 108 | 11+0 | 1435 | 4.8 |
| callers kenning | 11 | 960 | 107 | 11+0 | 1680 | 4.4 |
| callers tmp_tree | 11 | 908 | 107 | 11+0 | 1556 | 5.5 |
| callers line_of | 10 | 1439 | 106 | 10+0 | 1682 | 5.2 |
| callers col_of | 9 | 1332 | 109 | 9+0 | 1578 | 5.1 |
| callers indexed_fixture | 9 | 885 | 117 | 9+0 | 1605 | 6.8 |
| callers quiet | 9 | 590 | 108 | 9+0 | 948 | 11.8 |
| callers read_meta | 9 | 919 | 108 | 9+0 | 1290 | 5.4 |
| callers suggest_similar | 8 | 742 | 106 | 8+0 | 1035 | 4.9 |

**中央値**: ast-grep 107ms / 1332B vs kenning 5.1ms / 1771B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact txt | 76 | 227704 | 174 | 6818 | 33x |
| impact num | 72 | 217414 | 166 | 6508 | 33x |
| impact query | 17 | 41257 | 35 | 2074 | 20x |
| impact next | 82 | 241780 | 188 | 7355 | 33x |
| impact drop | 41 | 138201 | 102 | 4048 | 34x |

中央値: **33x**、tool 呼び出し 166 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| kenning.rs | 290626 | 7325 | 40x |
| RESULTS.md | 28152 | 3511 | 8x |
| core.rs | 26910 | 5813 | 5x |
| README.ja.md | 18786 | 657 | 29x |
| README.md | 17047 | 593 | 29x |

中央値: **29x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1671 B / 2 回 → def 139 B / 1 回 = **11.7x**

**faceted** (`kind:method vis:pub test:0`): 0 件 133.5µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 415 | 6 | 47188 | 415 | 5 | 56813 | = |
| kenning | 375 | 7 | 50233 | 374 | 6 | 62841 | ≠ |
| String | 371 | 12 | 43061 | 371 | 28 | 48882 | = |
| callers | 259 | 14 | 30168 | 259 | 6 | 50481 | = |
| println | 296 | 7 | 38372 | 296 | 5 | 40661 | = |
| assert | 256 | 6 | 33256 | 256 | 6 | 44720 | = |
| contains | 110 | 6 | 14392 | 110 | 5 | 18302 | = |
| return | 135 | 6 | 11117 | 135 | 5 | 12574 | = |
| container | 155 | 6 | 19575 | 155 | 5 | 22402 | = |
| eprintln | 117 | 6 | 16123 | 117 | 4 | 16839 | = |
| enchudb | 115 | 6 | 14439 | 99 | 5 | 16987 | ≠ |
| collect | 112 | 7 | 12645 | 112 | 5 | 13953 | = |
| format | 97 | 6 | 13436 | 97 | 5 | 14899 | = |
| update | 133 | 5 | 18994 | 133 | 5 | 22772 | = |
| assert_eq | 93 | 8 | 11739 | 93 | 5 | 15643 | = |
| is_empty | 87 | 5 | 8695 | 87 | 5 | 9686 | = |
| to_string | 148 | 8 | 17973 | 148 | 6 | 20798 | = |
| symbol | 134 | 6 | 17612 | 134 | 5 | 21314 | = |
| call_t | 86 | 6 | 9379 | 86 | 5 | 10366 | = |
| file_t | 86 | 6 | 8683 | 86 | 5 | 9729 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 6.3ms vs text 5.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 540.834µs
index: 20 files / 321 symbols / 4436 call-sites
  kind=fn                                              =     244 件  [166ns]
  pub fn                                               =      25 件  [250ns]
  pub async fn 非test                                   =       0 件  [125ns]
  def new                                              =       2 件  [83ns]
  callers new (名前一致)                                   =     128 件  [166ns]
  callers new (確実 逆引き)                                 =       1 件  [84ns]
```

