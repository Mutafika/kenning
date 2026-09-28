# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 770 files / 38430 call-sites (うち repo 内 20735) / repo 内確定率 72.6% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| join | 257 | 1 | 198 | 58 |
| fmt | 253 | 8 | 51 | 194 |
| ready | 245 | 40 | 48 | 157 |
| as_ref | 228 | 2 | 201 | 25 |
| reserve | 90 | 25 | 44 | 21 |
| pop | 86 | 32 | 47 | 7 |
| insert_at | 65 | 63 | 0 | 2 |
| split | 64 | 25 | 9 | 30 |
| try_acquire | 57 | 33 | 13 | 11 |
| parse | 39 | 1 | 28 | 10 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers registration | 27112 | 10 | 6962 | 1 | 3.9x |
| callers shared | 15239 | 3 | 9282 | 1 | 1.6x |
| callers data | 12701 | 4 | 7708 | 1 | 1.6x |
| callers enable_all | 43935 | 38 | 6153 | 1 | 7.1x |
| callers insert_at | 11934 | 4 | 8262 | 1 | 1.4x |
| callers worker_threads | 41996 | 37 | 6655 | 1 | 6.3x |
| callers unbounded_channel | 23621 | 17 | 10960 | 1 | 2.2x |
| callers run_one | 6011 | 3 | 5651 | 1 | 1.1x |
| callers measure | 7252 | 3 | 5821 | 1 | 1.2x |
| callers changed | 23502 | 13 | 7389 | 1 | 3.2x |
| callers asyncify | 19574 | 25 | 4654 | 1 | 4.2x |
| callers filled | 26987 | 19 | 5162 | 1 | 5.2x |
| callers made_progress | 18864 | 13 | 4706 | 1 | 4.0x |
| callers remaining | 25863 | 18 | 4981 | 1 | 5.2x |
| callers poll_proceed | 18885 | 13 | 4443 | 1 | 4.3x |
| callers push_front | 18865 | 12 | 3668 | 1 | 5.1x |
| callers sleep_until | 24365 | 16 | 6752 | 1 | 3.6x |
| callers put_slice | 34037 | 24 | 10820 | 1 | 3.1x |
| callers run_until | 11254 | 6 | 3588 | 1 | 3.1x |
| callers child_token | 6534 | 4 | 3866 | 1 | 1.7x |

**中央値**: 圧縮比 **3.6x**、grep 経路の tool 呼び出し 13 回 → 1 回

**単発 wall-clock 中央値**: rg 14.9ms vs kenning 11.6ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers registration | 89 | 15839 | 197 | 112+0 | 6962 | 12.0 |
| callers shared | 66 | 10400 | 198 | 66+0 | 9282 | 10.1 |
| callers data | 0 | 0 | 189 | 63+1 | 7708 | 10.9 |
| callers enable_all | 64 | 14951 | 193 | 63+1 | 6153 | 11.4 |
| callers insert_at | 63 | 8442 | 186 | 63+0 | 8262 | 10.8 |
| callers worker_threads | 56 | 13681 | 210 | 56+1 | 6655 | 12.7 |
| callers unbounded_channel | 30 | 3663 | 199 | 52+14 | 10960 | 12.0 |
| callers run_one | 45 | 4116 | 195 | 45+0 | 5651 | 11.1 |
| callers measure | 42 | 4569 | 189 | 42+0 | 5821 | 10.9 |
| callers changed | 42 | 4783 | 183 | 34+17 | 7389 | 11.6 |
| callers asyncify | 32 | 4586 | 204 | 32+0 | 4654 | 11.8 |
| callers filled | 23 | 2719 | 200 | 29+7 | 5162 | 12.4 |
| callers made_progress | 27 | 2945 | 190 | 28+8 | 4706 | 11.1 |
| callers remaining | 24 | 2930 | 194 | 28+4 | 4981 | 11.8 |
| callers poll_proceed | 1 | 147 | 204 | 23+3 | 4443 | 11.9 |
| callers push_front | 24 | 3171 | 184 | 23+1 | 3668 | 12.2 |
| callers sleep_until | 39 | 5150 | 210 | 23+17 | 6752 | 11.8 |
| callers put_slice | 74 | 7756 | 177 | 22+52 | 10820 | 11.5 |
| callers run_until | 22 | 24679 | 210 | 22+2 | 3588 | 10.8 |
| callers child_token | 21 | 2881 | 259 | 21+0 | 3866 | 11.5 |

**中央値**: ast-grep 197ms / 4586B vs kenning 11.6ms / 6153B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact registration | 291 | 1953167 | 1262 | 19202 | 102x |
| impact shared | 233 | 2051279 | 1393 | 22046 | 93x |
| impact data | 29 | 43839 | 60 | 8705 | 5x |
| impact enable_all | 108 | 1751685 | 1101 | 12372 | 142x |
| impact insert_at | 40 | 136135 | 124 | 5900 | 23x |

中央値: **93x**、tool 呼び出し 1101 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Debug | 140 | 117626 | 101 | 26.1x |
| impls Drop | 92 | 110618 | 89 | 23.5x |
| impls Future | 77 | 119429 | 95 | 27.1x |
| impls Sync | 60 | 59277 | 46 | 13.5x |
| impls Stream | 55 | 85591 | 72 | 18.4x |

中央値: **23.5x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| CHANGELOG.md | 143415 | 4859 | 30x |
| named_pipe.rs | 99154 | 11938 | 8x |
| udp.rs | 84068 | 11996 | 7x |
| bounded.rs | 64865 | 10469 | 6x |
| builder.rs | 59923 | 10801 | 6x |

中央値: **7x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1637 B / 2 回 → def 238 B / 1 回 = **6.4x**

**faceted** (`kind:method vis:pub test:0`): 1405 件 424.333µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 38 | 319769 | 2701 | 25 | 377187 | = |
| unwrap | 2686 | 15 | 318940 | 2686 | 25 | 369004 | = |
| Result | 2839 | 16 | 380851 | 2839 | 26 | 439632 | = |
| runtime | 2482 | 14 | 327851 | 2482 | 23 | 375878 | = |
| stream | 3200 | 14 | 415027 | 3200 | 22 | 477364 | = |
| github | 1571 | 15 | 189393 | 1571 | 18 | 248238 | = |
| assert | 6693 | 15 | 805288 | 6693 | 30 | 935625 | = |
| future | 2545 | 17 | 328368 | 2545 | 24 | 369457 | = |
| return | 3257 | 16 | 443672 | 3257 | 26 | 513451 | = |
| handle | 2879 | 16 | 378121 | 2879 | 23 | 431289 | = |
| thread | 2868 | 15 | 380972 | 2868 | 23 | 432415 | = |
| Context | 1584 | 16 | 213427 | 1584 | 22 | 241280 | = |
| feature | 1227 | 15 | 148931 | 1227 | 20 | 161735 | = |
| struct | 1414 | 16 | 166367 | 1414 | 23 | 191452 | = |
| channel | 1280 | 16 | 165719 | 1280 | 18 | 194601 | = |
| method | 1186 | 14 | 173192 | 1186 | 21 | 201872 | = |
| socket | 1847 | 16 | 237089 | 1847 | 18 | 274705 | = |
| buffer | 1225 | 15 | 166630 | 1225 | 18 | 195053 | = |
| unsafe | 1067 | 14 | 136843 | 1067 | 18 | 152451 | = |
| function | 1070 | 16 | 153227 | 1070 | 20 | 179865 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 15.5ms vs text 22.7ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 986.708µs
index: 770 files / 7723 symbols / 38430 call-sites
  kind=fn                                              =    2446 件  [625ns]
  pub fn                                               =     107 件  [4.041µs]
  pub async fn 非test                                   =      42 件  [3.958µs]
  def new                                              =     280 件  [208ns]
  callers new (名前一致)                                   =    2424 件  [625ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 207 files / 17953 call-sites (うち repo 内 10395) / repo 内確定率 92.6% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| parse_low_raw | 546 | 545 | 0 | 1 |
| build | 403 | 355 | 8 | 40 |
| search_reader | 131 | 130 | 0 | 1 |
| name_long | 126 | 21 | 0 | 105 |
| wfile | 85 | 85 | 0 | 0 |
| is_match | 82 | 44 | 9 | 29 |
| write | 59 | 32 | 4 | 23 |
| name_negated | 55 | 12 | 0 | 43 |
| matches_all | 46 | 35 | 0 | 11 |
| line_terminator | 43 | 32 | 0 | 11 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 3 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers arg | 97701 | 11 | 8364 | 1 | 11.7x |
| callers parse_low_raw | 75670 | 3 | 7995 | 1 | 9.5x |
| callers create | 57957 | 10 | 6958 | 1 | 8.3x |
| callers is_ignore | 33306 | 10 | 7735 | 1 | 4.3x |
| callers create_dir | 12611 | 4 | 5644 | 1 | 2.2x |
| callers prev | 11173 | 2 | 8078 | 1 | 1.4x |
| callers unwrap_switch | 12063 | 3 | 7410 | 1 | 1.6x |
| callers exact | 11725 | 2 | 8279 | 1 | 1.4x |
| callers assert_err | 14363 | 7 | 6163 | 1 | 2.3x |
| callers expected_no_line_number | 9655 | 3 | 7083 | 1 | 1.4x |
| callers test | 8679 | 3 | 6007 | 1 | 1.4x |
| callers create_bytes | 11719 | 6 | 5977 | 1 | 2.0x |
| callers add_child | 8302 | 3 | 6312 | 1 | 1.3x |
| callers assert_paths | 5105 | 2 | 4399 | 1 | 1.2x |
| callers unwrap_value | 6281 | 3 | 4873 | 1 | 1.3x |
| callers inexact | 6270 | 2 | 4572 | 1 | 1.4x |
| callers as_byte | 16289 | 10 | 4570 | 1 | 3.6x |
| callers text | 4050 | 2 | 3521 | 1 | 1.2x |
| callers seq | 4753 | 2 | 3770 | 1 | 1.3x |
| callers bstr | 4612 | 2 | 3641 | 1 | 1.3x |

**中央値**: 圧縮比 **1.4x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 10.1ms vs kenning 7.6ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers arg | 8 | 710 | 148 | 674+8 | 8364 | 7.5 |
| callers parse_low_raw | 545 | 77997 | 137 | 545+0 | 7995 | 7.9 |
| callers create | 9 | 979 | 134 | 442+5 | 6958 | 8.6 |
| callers is_ignore | 4 | 441 | 126 | 134+0 | 7735 | 8.4 |
| callers create_dir | 1 | 90 | 127 | 100+0 | 5644 | 7.6 |
| callers prev | 0 | 0 | 140 | 72+0 | 8078 | 7.6 |
| callers unwrap_switch | 39 | 4612 | 122 | 69+0 | 7410 | 7.8 |
| callers exact | 9 | 1277 | 130 | 66+8 | 8279 | 6.5 |
| callers assert_err | 0 | 0 | 125 | 60+0 | 6163 | 7.7 |
| callers expected_no_line_number | 55 | 26623 | 127 | 55+0 | 7083 | 7.2 |
| callers test | 55 | 35090 | 128 | 55+0 | 6007 | 8.9 |
| callers create_bytes | 1 | 113 | 113 | 47+0 | 5977 | 7.3 |
| callers add_child | 38 | 6645 | 117 | 38+0 | 6312 | 7.5 |
| callers assert_paths | 33 | 12988 | 112 | 33+0 | 4399 | 7.9 |
| callers unwrap_value | 32 | 4087 | 131 | 32+0 | 4873 | 8.0 |
| callers inexact | 2 | 1063 | 129 | 29+1 | 4572 | 7.5 |
| callers as_byte | 27 | 3717 | 126 | 27+0 | 4570 | 8.5 |
| callers text | 0 | 0 | 143 | 27+0 | 3521 | 7.1 |
| callers seq | 0 | 0 | 156 | 25+0 | 3770 | 7.1 |
| callers bstr | 0 | 0 | 126 | 23+0 | 3641 | 7.3 |

**中央値**: ast-grep 128ms / 1063B vs kenning 7.6ms / 6163B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact arg | 1 | 97701 | 11 | 372 | 263x |
| impact parse_low_raw | 105 | 207703 | 211 | 6264 | 33x |
| impact create | 2 | 59611 | 12 | 461 | 129x |
| impact is_ignore | 74 | 302364 | 227 | 9176 | 33x |
| impact create_dir | 2 | 14265 | 6 | 465 | 31x |

中央値: **33x**、tool 呼び出し 12 回 → 1 回

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

中央値: grep 1445 B / 2 回 → def 239 B / 1 回 = **6.1x**

**faceted** (`kind:method vis:pub test:0`): 477 件 208µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 10 | 856322 | 3298 | 12 | 746164 | = |
| ignore | 3475 | 9 | 566374 | 3473 | 14 | 585001 | ≠ |
| OpenSubtitles2016 | 2349 | 11 | 637772 | 2349 | 10 | 547110 | = |
| ripgrep | 1886 | 10 | 244360 | 1885 | 11 | 262354 | ≠ |
| benchsuite | 1896 | 10 | 507042 | 1896 | 9 | 440490 | = |
| Sherlock | 2133 | 10 | 421572 | 2139 | 12 | 383183 | ≠ |
| Holmes | 1679 | 10 | 396369 | 1688 | 11 | 349402 | ≠ |
| sample | 1580 | 10 | 406148 | 1580 | 9 | 343953 | = |
| Холмс | 1406 | 11 | 379069 | 1406 | 10 | 339776 | = |
| Шерлок | 1206 | 10 | 330816 | 1206 | 9 | 295258 | = |
| assert_eq | 1296 | 11 | 158656 | 1296 | 10 | 177179 | = |
| unwrap | 1306 | 8 | 165821 | 1306 | 11 | 185555 | = |
| LC_ALL | 1103 | 10 | 281795 | 1103 | 8 | 244529 | = |
| PM_RESUME | 969 | 9 | 175046 | 969 | 8 | 173294 | = |
| matcher | 1200 | 10 | 151245 | 1194 | 11 | 171829 | ≠ |
| github | 954 | 10 | 121749 | 886 | 10 | 116086 | ≠ |
| BurntSushi | 806 | 9 | 102017 | 806 | 9 | 103321 | = |
| matches | 972 | 9 | 123456 | 972 | 12 | 139052 | = |
| return | 1307 | 10 | 164960 | 1308 | 12 | 188021 | ≠ |
| pattern | 1002 | 9 | 143447 | 1002 | 12 | 155061 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.9ms vs text 10.7ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 613.125µs
index: 207 files / 3192 symbols / 17953 call-sites
  kind=fn                                              =     705 件  [291ns]
  pub fn                                               =      30 件  [1.583µs]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =      84 件  [208ns]
  callers new (名前一致)                                   =    1093 件  [416ns]
  callers new (確実 逆引き)                                 =       3 件  [166ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 352 files / 62869 call-sites (うち repo 内 28516) / repo 内確定率 80.5% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| push | 739 | 50 | 672 | 17 |
| get_table | 279 | 256 | 0 | 23 |
| define_himo_in | 227 | 214 | 0 | 13 |
| with_capacity | 149 | 39 | 101 | 9 |
| make_engine | 138 | 0 | 119 | 19 |
| candidates | 54 | 44 | 0 | 10 |
| db_path | 48 | 7 | 35 | 6 |
| sync | 36 | 22 | 0 | 14 |
| fmt | 31 | 4 | 0 | 27 |
| make_db | 29 | 0 | 24 | 5 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 3 + 候補 0 = 検討対象 4 行、ノイズ率 32%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 157631 | 73 | 6656 | 1 | 23.7x |
| callers define_himo | 175527 | 85 | 7579 | 1 | 23.2x |
| callers number | 136407 | 57 | 7983 | 1 | 17.1x |
| callers entity_in | 172590 | 85 | 8652 | 1 | 19.9x |
| callers eid_local | 93639 | 38 | 7976 | 1 | 11.7x |
| callers create_standalone | 109333 | 53 | 7908 | 1 | 13.8x |
| callers define_himo_in | 176141 | 90 | 9424 | 1 | 18.7x |
| callers define_table | 165963 | 89 | 8531 | 1 | 19.5x |
| callers flush_writes | 120804 | 68 | 7272 | 1 | 16.6x |
| callers tie_to | 101597 | 52 | 8324 | 1 | 12.2x |
| callers primary_key | 116532 | 56 | 8747 | 1 | 13.3x |
| callers oplog_sync | 120773 | 69 | 7977 | 1 | 15.1x |
| callers pull_once | 71817 | 34 | 8508 | 1 | 8.4x |
| callers oplog_commit | 101896 | 61 | 7497 | 1 | 13.6x |
| callers open_concurrent_with_oplog | 83836 | 43 | 10027 | 1 | 8.4x |
| callers make_eid | 79845 | 38 | 8485 | 1 | 9.4x |
| callers open_standalone | 79323 | 44 | 8738 | 1 | 9.1x |
| callers remove_db | 82782 | 48 | 8098 | 1 | 10.2x |
| callers publish_since | 58132 | 29 | 8558 | 1 | 6.8x |
| callers tie_text | 69749 | 37 | 7969 | 1 | 8.8x |

**中央値**: 圧縮比 **13.6x**、grep 経路の tool 呼び出し 56 回 → 1 回

**単発 wall-clock 中央値**: rg 11.4ms vs kenning 13.5ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 435 | 44151 | 327 | 435+3 | 6656 | 14.2 |
| callers define_himo | 382 | 44538 | 327 | 382+0 | 7579 | 14.2 |
| callers number | 276 | 39939 | 312 | 286+0 | 7983 | 12.2 |
| callers entity_in | 260 | 32500 | 312 | 262+0 | 8652 | 13.1 |
| callers eid_local | 187 | 26880 | 303 | 230+1 | 7976 | 13.2 |
| callers create_standalone | 221 | 27875 | 304 | 221+0 | 7908 | 13.8 |
| callers define_himo_in | 213 | 31738 | 311 | 214+0 | 9424 | 14.0 |
| callers define_table | 197 | 24157 | 311 | 197+0 | 8531 | 13.5 |
| callers flush_writes | 176 | 17122 | 332 | 177+0 | 7272 | 13.4 |
| callers tie_to | 174 | 21462 | 318 | 174+0 | 8324 | 12.8 |
| callers primary_key | 159 | 47003 | 320 | 160+0 | 8747 | 12.7 |
| callers oplog_sync | 137 | 14298 | 318 | 138+0 | 7977 | 14.8 |
| callers pull_once | 116 | 12033 | 300 | 130+0 | 8508 | 12.3 |
| callers oplog_commit | 123 | 11961 | 313 | 123+0 | 7497 | 11.3 |
| callers open_concurrent_with_oplog | 117 | 17367 | 330 | 117+0 | 10027 | 14.6 |
| callers make_eid | 101 | 13927 | 325 | 113+0 | 8485 | 15.9 |
| callers open_standalone | 112 | 14612 | 323 | 113+1 | 8738 | 13.6 |
| callers remove_db | 108 | 12681 | 329 | 108+0 | 8098 | 11.1 |
| callers publish_since | 99 | 11886 | 312 | 105+0 | 8558 | 11.4 |
| callers tie_text | 103 | 12660 | 329 | 103+1 | 7969 | 14.0 |

**中央値**: ast-grep 318ms / 21462B vs kenning 13.5ms / 8324B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 383 | 1649053 | 1204 | 22701 | 73x |
| impact define_himo | 398 | 1630756 | 1221 | 21172 | 77x |
| impact number | 144 | 681496 | 481 | 10770 | 63x |
| impact entity_in | 698 | 4282239 | 2671 | 41184 | 104x |
| impact eid_local | 1380 | 8749394 | 5379 | 33009 | 265x |

中央値: **77x**、tool 呼び出し 1221 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Drop | 31 | 42984 | 27 | 16.4x |
| impls Default | 20 | 24336 | 18 | 15.4x |
| impls Debug | 18 | 9115 | 6 | 6.2x |
| impls Sync | 18 | 32422 | 19 | 21.5x |
| impls Send | 17 | 32364 | 19 | 22.6x |

中央値: **16.4x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 909768 | 11445 | 79x |
| CHANGELOG.md | 419390 | 6157 | 68x |
| lib.rs | 319006 | 9713 | 33x |
| live.rs | 264241 | 9658 | 27x |
| oplog.rs | 123369 | 7532 | 16x |

中央値: **33x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.8x**

**faceted** (`kind:method vis:pub test:0`): 1294 件 347.25µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 5985 | 17 | 782623 | 5985 | 25 | 928128 | = |
| assert_eq | 2695 | 12 | 350583 | 2695 | 19 | 432459 | = |
| Engine | 3268 | 13 | 455710 | 3261 | 25 | 553421 | ≠ |
| entity | 2857 | 12 | 400530 | 2857 | 22 | 478449 | = |
| assert | 4485 | 12 | 585324 | 4479 | 24 | 727963 | ≠ |
| format | 1661 | 12 | 225821 | 1661 | 19 | 253303 | = |
| return | 1367 | 11 | 165737 | 1367 | 16 | 185217 | = |
| himo_id | 1025 | 11 | 144885 | 1025 | 15 | 167615 | = |
| engine | 3268 | 13 | 455710 | 3261 | 21 | 553421 | ≠ |
| cleanup | 1037 | 10 | 98432 | 1037 | 14 | 132235 | = |
| ValueType | 951 | 11 | 120947 | 951 | 15 | 140947 | = |
| enchudb_oplog | 905 | 11 | 122764 | 905 | 14 | 140717 | = |
| enchudb | 2589 | 12 | 337387 | 2542 | 20 | 383406 | ≠ |
| collect | 1194 | 10 | 161218 | 1185 | 16 | 178964 | ≠ |
| author | 1195 | 10 | 175774 | 1195 | 14 | 221204 | = |
| Ordering | 848 | 11 | 113021 | 848 | 14 | 128105 | = |
| record | 1557 | 11 | 228377 | 1557 | 16 | 282662 | = |
| insert | 1064 | 12 | 145692 | 1064 | 16 | 168331 | = |
| String | 1197 | 11 | 152261 | 1183 | 18 | 170613 | ≠ |
| commit | 1394 | 10 | 202327 | 1394 | 16 | 242408 | = |

**一致**: 14/20 問でヒット数が同一。差の内訳: 少ない 6 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 11.4ms vs text 16.3ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 728.333µs
index: 352 files / 5394 symbols / 62869 call-sites
  kind=fn                                              =    2486 件  [625ns]
  pub fn                                               =      86 件  [3.708µs]
  pub async fn 非test                                   =       0 件  [416ns]
  def new                                              =      69 件  [166ns]
  callers new (名前一致)                                   =    2934 件  [708ns]
  callers new (確実 逆引き)                                 =       2 件  [125ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 32 files / 8927 call-sites (うち repo 内 1751) / repo 内確定率 86.4% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| query | 134 | 130 | 0 | 4 |
| num | 70 | 68 | 0 | 2 |
| index | 67 | 49 | 0 | 18 |
| file_paths | 21 | 20 | 0 | 1 |
| open_ro | 20 | 19 | 0 | 1 |
| indexed | 19 | 18 | 0 | 1 |
| parse_opts | 18 | 17 | 0 | 1 |
| line_of | 14 | 13 | 0 | 1 |
| median_f | 14 | 13 | 0 | 1 |
| col_of | 12 | 11 | 0 | 1 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers query | 22444 | 4 | 7785 | 1 | 2.9x |
| callers txt | 27594 | 9 | 7312 | 1 | 3.8x |
| callers num | 24211 | 9 | 7227 | 1 | 3.4x |
| callers tmp | 8447 | 3 | 6150 | 1 | 1.4x |
| callers write_fixture | 5621 | 2 | 6929 | 1 | 0.8x |
| callers index | 31770 | 11 | 6039 | 1 | 5.3x |
| callers ref_of | 15173 | 7 | 4798 | 1 | 3.2x |
| callers file_paths | 9160 | 5 | 2323 | 1 | 3.9x |
| callers open_ro | 8722 | 5 | 2477 | 1 | 3.5x |
| callers indexed | 3072 | 2 | 2471 | 1 | 1.2x |
| callers parse_opts | 9358 | 5 | 1980 | 1 | 4.7x |
| callers kenning | 17791 | 8 | 2598 | 1 | 6.8x |
| callers line_of | 5636 | 3 | 2124 | 1 | 2.7x |
| callers median_f | 4897 | 3 | 1665 | 1 | 2.9x |
| callers median_u | 5027 | 3 | 1688 | 1 | 3.0x |
| callers tmp_tree | 2786 | 2 | 1754 | 1 | 1.6x |
| callers col_of | 3343 | 2 | 1876 | 1 | 1.8x |
| callers fmt_sym | 4702 | 3 | 1512 | 1 | 3.1x |
| callers changes_fixture | 2793 | 2 | 1600 | 1 | 1.7x |
| callers end_line_of | 5149 | 3 | 1833 | 1 | 2.8x |

**中央値**: 圧縮比 **3.0x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 8.0ms vs kenning 5.8ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers query | 123 | 12721 | 66 | 130+0 | 7785 | 6.4 |
| callers txt | 83 | 9089 | 67 | 88+0 | 7312 | 6.0 |
| callers num | 61 | 6745 | 64 | 68+0 | 7227 | 6.2 |
| callers tmp | 53 | 3453 | 68 | 53+0 | 6150 | 5.5 |
| callers write_fixture | 50 | 29907 | 68 | 50+0 | 6929 | 6.0 |
| callers index | 49 | 3234 | 77 | 49+0 | 6039 | 5.8 |
| callers ref_of | 31 | 4282 | 67 | 31+0 | 4798 | 6.0 |
| callers file_paths | 20 | 1826 | 63 | 20+0 | 2323 | 5.6 |
| callers open_ro | 19 | 1986 | 67 | 19+0 | 2477 | 6.6 |
| callers indexed | 18 | 1466 | 81 | 18+0 | 2471 | 5.8 |
| callers parse_opts | 17 | 1530 | 71 | 17+0 | 1980 | 6.2 |
| callers kenning | 16 | 1774 | 85 | 16+0 | 2598 | 6.6 |
| callers line_of | 12 | 1708 | 70 | 13+0 | 2124 | 5.5 |
| callers median_f | 0 | 0 | 63 | 13+0 | 1665 | 5.3 |
| callers median_u | 0 | 0 | 69 | 13+0 | 1688 | 5.9 |
| callers tmp_tree | 12 | 1045 | 65 | 12+0 | 1754 | 5.3 |
| callers col_of | 11 | 1590 | 63 | 11+0 | 1876 | 5.5 |
| callers fmt_sym | 1 | 101 | 61 | 11+0 | 1512 | 5.6 |
| callers changes_fixture | 10 | 843 | 65 | 10+0 | 1600 | 5.5 |
| callers end_line_of | 7 | 1182 | 65 | 10+0 | 1833 | 5.4 |

**中央値**: ast-grep 67ms / 1774B vs kenning 5.8ms / 2471B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact query | 76 | 189121 | 154 | 6876 | 28x |
| impact txt | 98 | 481024 | 289 | 9213 | 52x |
| impact num | 91 | 361246 | 248 | 8617 | 42x |
| impact tmp | 79 | 181738 | 159 | 9807 | 19x |
| impact write_fixture | 79 | 178912 | 158 | 9818 | 18x |

中央値: **28x**、tool 呼び出し 159 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| parse.rs | 111015 | 10323 | 11x |
| core.rs | 100218 | 11474 | 9x |
| query.rs | 82803 | 11126 | 7x |
| tests.rs | 54057 | 11897 | 5x |
| bench.rs | 40844 | 5643 | 7x |

中央値: **7x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1728 B / 2 回 → def 153 B / 1 回 = **10.6x**

**faceted** (`kind:method vis:pub test:0`): 6 件 117.25µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| String | 668 | 8 | 82803 | 668 | 7 | 93029 | = |
| unwrap | 593 | 6 | 73221 | 593 | 6 | 87542 | = |
| kenning | 488 | 9 | 69656 | 487 | 7 | 85383 | ≠ |
| contains | 325 | 8 | 47263 | 325 | 6 | 60506 | = |
| callers | 423 | 7 | 54980 | 423 | 7 | 80849 | = |
| assert | 528 | 8 | 73637 | 528 | 6 | 98234 | = |
| return | 248 | 8 | 23743 | 248 | 6 | 26350 | = |
| println | 375 | 8 | 52210 | 375 | 6 | 55274 | = |
| format | 199 | 8 | 28536 | 199 | 6 | 31662 | = |
| to_string | 270 | 9 | 35010 | 270 | 6 | 39721 | = |
| method | 231 | 8 | 34365 | 231 | 6 | 40246 | = |
| collect | 181 | 8 | 23067 | 181 | 6 | 25460 | = |
| container | 191 | 8 | 26139 | 191 | 6 | 28898 | = |
| assert_eq | 152 | 7 | 20195 | 152 | 6 | 26697 | = |
| is_empty | 141 | 8 | 15562 | 141 | 7 | 17338 | = |
| enchudb | 155 | 8 | 20850 | 134 | 6 | 24296 | ≠ |
| eprintln | 141 | 6 | 20753 | 141 | 6 | 21626 | = |
| Option | 132 | 8 | 16681 | 132 | 6 | 19096 | = |
| process | 120 | 8 | 15329 | 120 | 6 | 17357 | = |
| update | 156 | 7 | 22427 | 156 | 6 | 26703 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 7.8ms vs text 6.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 536.625µs
index: 32 files / 564 symbols / 8927 call-sites
  kind=fn                                              =     413 件  [250ns]
  pub fn                                               =      26 件  [375ns]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =       3 件  [167ns]
  callers new (名前一致)                                   =     249 件  [250ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```

