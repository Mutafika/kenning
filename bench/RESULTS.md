# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 770 files / 38428 call-sites (うち repo 内 20836) / repo 内確定率 73.7% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| join | 257 | 1 | 198 | 58 |
| fmt | 253 | 9 | 50 | 194 |
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
| callers registration | 27112 | 10 | 10568 | 1 | 2.6x |
| callers shared | 15239 | 3 | 9282 | 1 | 1.6x |
| callers unbounded_channel | 23621 | 17 | 8328 | 1 | 2.8x |
| callers data | 12701 | 4 | 7708 | 1 | 1.6x |
| callers enable_all | 43935 | 38 | 6153 | 1 | 7.1x |
| callers insert_at | 11934 | 4 | 8262 | 1 | 1.4x |
| callers worker_threads | 41996 | 37 | 6655 | 1 | 6.3x |
| callers run_one | 6011 | 3 | 5651 | 1 | 1.1x |
| callers measure | 7252 | 3 | 5821 | 1 | 1.2x |
| callers sleep_until | 24365 | 16 | 6558 | 1 | 3.7x |
| callers filled | 26987 | 19 | 4955 | 1 | 5.4x |
| callers changed | 23502 | 13 | 7379 | 1 | 3.2x |
| callers asyncify | 19574 | 25 | 4654 | 1 | 4.2x |
| callers made_progress | 18864 | 13 | 4706 | 1 | 4.0x |
| callers remaining | 25863 | 18 | 4981 | 1 | 5.2x |
| callers poll_proceed | 18885 | 13 | 4419 | 1 | 4.3x |
| callers push_front | 18865 | 12 | 3668 | 1 | 5.1x |
| callers put_slice | 34037 | 24 | 10820 | 1 | 3.1x |
| callers run_until | 11254 | 6 | 3588 | 1 | 3.1x |
| callers child_token | 6534 | 4 | 3866 | 1 | 1.7x |

**中央値**: 圧縮比 **3.2x**、grep 経路の tool 呼び出し 13 回 → 1 回

**単発 wall-clock 中央値**: rg 14.6ms vs kenning 11.4ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers registration | 89 | 15839 | 173 | 91+21 | 10568 | 12.8 |
| callers shared | 66 | 10400 | 178 | 66+0 | 9282 | 11.5 |
| callers unbounded_channel | 30 | 3663 | 160 | 66+0 | 8328 | 13.8 |
| callers data | 0 | 0 | 173 | 63+1 | 7708 | 11.6 |
| callers enable_all | 64 | 14951 | 181 | 63+1 | 6153 | 12.0 |
| callers insert_at | 63 | 8442 | 217 | 63+0 | 8262 | 10.8 |
| callers worker_threads | 56 | 13681 | 222 | 56+1 | 6655 | 12.2 |
| callers run_one | 45 | 4116 | 220 | 45+0 | 5651 | 12.2 |
| callers measure | 42 | 4569 | 219 | 42+0 | 5821 | 10.6 |
| callers sleep_until | 39 | 5150 | 216 | 39+1 | 6558 | 11.8 |
| callers filled | 23 | 2719 | 201 | 36+0 | 4955 | 11.4 |
| callers changed | 42 | 4783 | 167 | 34+17 | 7379 | 11.3 |
| callers asyncify | 32 | 4586 | 180 | 32+0 | 4654 | 11.2 |
| callers made_progress | 27 | 2945 | 163 | 28+8 | 4706 | 11.4 |
| callers remaining | 24 | 2930 | 186 | 28+4 | 4981 | 11.2 |
| callers poll_proceed | 1 | 147 | 159 | 25+1 | 4419 | 11.3 |
| callers push_front | 24 | 3171 | 158 | 23+1 | 3668 | 11.0 |
| callers put_slice | 74 | 7756 | 186 | 22+52 | 10820 | 11.3 |
| callers run_until | 22 | 24679 | 184 | 22+2 | 3588 | 10.7 |
| callers child_token | 21 | 2881 | 175 | 21+0 | 3866 | 11.2 |

**中央値**: ast-grep 181ms / 4586B vs kenning 11.4ms / 6153B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact registration | 255 | 1937341 | 1240 | 19289 | 100x |
| impact shared | 280 | 2194557 | 1532 | 23739 | 92x |
| impact unbounded_channel | 116 | 1900699 | 1226 | 14134 | 134x |
| impact data | 29 | 43839 | 60 | 8705 | 5x |
| impact enable_all | 135 | 1871559 | 1206 | 14691 | 127x |

中央値: **100x**、tool 呼び出し 1226 回 → 1 回

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

中央値: grep 1637 B / 2 回 → def 220 B / 1 回 = **6.4x**

**faceted** (`kind:method vis:pub test:0`): 1405 件 338.875µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 30 | 319769 | 2701 | 25 | 377187 | = |
| unwrap | 2686 | 14 | 318940 | 2686 | 24 | 369000 | = |
| Result | 2839 | 16 | 380851 | 2839 | 24 | 439632 | = |
| runtime | 2482 | 16 | 327851 | 2482 | 22 | 375751 | = |
| stream | 3200 | 15 | 415027 | 3200 | 22 | 477363 | = |
| github | 1571 | 15 | 189393 | 1571 | 17 | 247613 | = |
| assert | 6693 | 14 | 805288 | 6693 | 30 | 935669 | = |
| future | 2545 | 16 | 328368 | 2545 | 24 | 369447 | = |
| return | 3257 | 14 | 443672 | 3257 | 25 | 513451 | = |
| handle | 2879 | 15 | 378121 | 2879 | 23 | 431289 | = |
| thread | 2868 | 14 | 380972 | 2868 | 24 | 432457 | = |
| Context | 1584 | 15 | 213427 | 1584 | 22 | 241280 | = |
| feature | 1227 | 15 | 148931 | 1227 | 20 | 161494 | = |
| struct | 1414 | 15 | 166367 | 1414 | 24 | 191451 | = |
| channel | 1280 | 16 | 165719 | 1280 | 18 | 194600 | = |
| method | 1186 | 14 | 173192 | 1186 | 19 | 201872 | = |
| socket | 1847 | 15 | 237089 | 1847 | 17 | 274705 | = |
| buffer | 1225 | 15 | 166630 | 1225 | 18 | 195053 | = |
| unsafe | 1067 | 14 | 136843 | 1067 | 18 | 152451 | = |
| function | 1070 | 14 | 153227 | 1070 | 20 | 179752 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 15.2ms vs text 22.3ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 741.5µs
index: 770 files / 7723 symbols / 38428 call-sites
  kind=fn                                              =    2446 件  [583ns]
  pub fn                                               =     107 件  [4.042µs]
  pub async fn 非test                                   =      42 件  [4µs]
  def new                                              =     280 件  [208ns]
  callers new (名前一致)                                   =    2424 件  [625ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 207 files / 17953 call-sites (うち repo 内 10394) / repo 内確定率 92.7% / **baked (SCIP)**

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

**単発 wall-clock 中央値**: rg 9.7ms vs kenning 7.2ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers arg | 8 | 710 | 137 | 674+8 | 8364 | 9.9 |
| callers parse_low_raw | 545 | 77997 | 113 | 545+0 | 7995 | 8.4 |
| callers create | 9 | 979 | 116 | 442+5 | 6958 | 8.9 |
| callers is_ignore | 4 | 441 | 95 | 134+0 | 7735 | 7.1 |
| callers create_dir | 1 | 90 | 108 | 100+0 | 5644 | 7.1 |
| callers prev | 0 | 0 | 114 | 72+0 | 8078 | 7.7 |
| callers unwrap_switch | 39 | 4612 | 107 | 69+0 | 7410 | 7.1 |
| callers exact | 9 | 1277 | 110 | 66+8 | 8279 | 7.3 |
| callers assert_err | 0 | 0 | 114 | 60+0 | 6163 | 7.2 |
| callers expected_no_line_number | 55 | 26623 | 98 | 55+0 | 7083 | 6.7 |
| callers test | 55 | 35090 | 123 | 55+0 | 6007 | 6.8 |
| callers create_bytes | 1 | 113 | 115 | 47+0 | 5977 | 7.5 |
| callers add_child | 38 | 6645 | 112 | 38+0 | 6312 | 7.0 |
| callers assert_paths | 33 | 12988 | 105 | 33+0 | 4399 | 7.1 |
| callers unwrap_value | 32 | 4087 | 121 | 32+0 | 4873 | 7.7 |
| callers inexact | 2 | 1063 | 96 | 29+1 | 4572 | 7.0 |
| callers as_byte | 27 | 3717 | 106 | 27+0 | 4570 | 7.3 |
| callers text | 0 | 0 | 130 | 27+0 | 3521 | 6.8 |
| callers seq | 0 | 0 | 117 | 25+0 | 3770 | 7.2 |
| callers bstr | 0 | 0 | 131 | 23+0 | 3641 | 7.2 |

**中央値**: ast-grep 114ms / 1063B vs kenning 7.2ms / 6163B — 構造一致としては同数を拾うが、
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

**faceted** (`kind:method vis:pub test:0`): 477 件 167.875µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 10 | 856322 | 3298 | 10 | 746164 | = |
| ignore | 3475 | 9 | 566374 | 3473 | 13 | 584698 | ≠ |
| OpenSubtitles2016 | 2349 | 9 | 637772 | 2349 | 9 | 547110 | = |
| ripgrep | 1886 | 8 | 244360 | 1885 | 11 | 261103 | ≠ |
| benchsuite | 1896 | 8 | 507042 | 1896 | 9 | 440490 | = |
| Sherlock | 2133 | 9 | 421572 | 2139 | 11 | 383183 | ≠ |
| Holmes | 1679 | 9 | 396369 | 1688 | 10 | 349402 | ≠ |
| sample | 1580 | 9 | 406148 | 1580 | 9 | 343953 | = |
| Холмс | 1406 | 9 | 379069 | 1406 | 8 | 339776 | = |
| Шерлок | 1206 | 8 | 330816 | 1206 | 9 | 295218 | = |
| assert_eq | 1296 | 9 | 158656 | 1296 | 10 | 177179 | = |
| unwrap | 1306 | 11 | 165821 | 1306 | 12 | 185555 | = |
| LC_ALL | 1103 | 9 | 281795 | 1103 | 9 | 244529 | = |
| PM_RESUME | 969 | 8 | 175046 | 969 | 8 | 173294 | = |
| matcher | 1200 | 9 | 151245 | 1194 | 11 | 171785 | ≠ |
| github | 954 | 8 | 121749 | 886 | 10 | 116086 | ≠ |
| BurntSushi | 806 | 9 | 102017 | 806 | 9 | 103301 | = |
| matches | 972 | 9 | 123456 | 972 | 11 | 138859 | = |
| return | 1307 | 9 | 164960 | 1308 | 12 | 188021 | ≠ |
| pattern | 1002 | 8 | 143447 | 1002 | 11 | 154678 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.0ms vs text 10.4ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 609.708µs
index: 207 files / 3192 symbols / 17953 call-sites
  kind=fn                                              =     705 件  [291ns]
  pub fn                                               =      30 件  [1.583µs]
  pub async fn 非test                                   =       0 件  [333ns]
  def new                                              =      84 件  [208ns]
  callers new (名前一致)                                   =    1093 件  [416ns]
  callers new (確実 逆引き)                                 =       3 件  [166ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 354 files / 63392 call-sites (うち repo 内 28684) / repo 内確定率 89.5% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| tmp | 560 | 498 | 0 | 62 |
| commit | 436 | 389 | 2 | 45 |
| number | 316 | 286 | 0 | 30 |
| flush | 240 | 178 | 22 | 40 |
| define_himo_in | 228 | 215 | 0 | 13 |
| primary_key | 181 | 160 | 0 | 21 |
| enable_sync_tables | 130 | 89 | 0 | 41 |
| set_peer_id | 119 | 114 | 0 | 5 |
| col | 65 | 52 | 2 | 11 |
| from_bytes | 53 | 44 | 3 | 6 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 4 + 候補 0 = 検討対象 4 行、ノイズ率 29%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 157649 | 73 | 6656 | 1 | 23.7x |
| callers define_himo | 178078 | 86 | 7610 | 1 | 23.4x |
| callers number | 137394 | 57 | 7983 | 1 | 17.2x |
| callers entity_in | 177254 | 87 | 8652 | 1 | 20.5x |
| callers eid_local | 93816 | 38 | 7977 | 1 | 11.8x |
| callers create_standalone | 111509 | 54 | 7908 | 1 | 14.1x |
| callers define_himo_in | 178702 | 91 | 9451 | 1 | 18.9x |
| callers define_table | 168574 | 90 | 8531 | 1 | 19.8x |
| callers flush_writes | 121363 | 68 | 7272 | 1 | 16.7x |
| callers tie_to | 102800 | 52 | 8324 | 1 | 12.3x |
| callers primary_key | 116922 | 56 | 8748 | 1 | 13.4x |
| callers oplog_sync | 130229 | 72 | 8001 | 1 | 16.3x |
| callers pull_once | 72063 | 34 | 8508 | 1 | 8.5x |
| callers oplog_commit | 105910 | 62 | 7552 | 1 | 14.0x |
| callers open_concurrent_with_oplog | 85775 | 44 | 10069 | 1 | 8.5x |
| callers remove_db | 85361 | 49 | 8239 | 1 | 10.4x |
| callers make_eid | 79845 | 38 | 8485 | 1 | 9.4x |
| callers open_standalone | 79435 | 44 | 8738 | 1 | 9.1x |
| callers publish_since | 58359 | 29 | 8558 | 1 | 6.8x |
| callers tie_text | 69749 | 37 | 7969 | 1 | 8.8x |

**中央値**: 圧縮比 **14.0x**、grep 経路の tool 呼び出し 56 回 → 1 回

**単発 wall-clock 中央値**: rg 10.6ms vs kenning 12.2ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 435 | 44151 | 305 | 435+3 | 6656 | 12.1 |
| callers define_himo | 384 | 44799 | 298 | 384+0 | 7610 | 12.2 |
| callers number | 276 | 39939 | 288 | 286+0 | 7983 | 11.6 |
| callers entity_in | 262 | 32792 | 281 | 264+0 | 8652 | 12.2 |
| callers eid_local | 188 | 27057 | 277 | 231+1 | 7977 | 12.1 |
| callers create_standalone | 222 | 28022 | 300 | 222+0 | 7908 | 12.2 |
| callers define_himo_in | 214 | 31897 | 268 | 215+0 | 9451 | 12.4 |
| callers define_table | 198 | 24294 | 298 | 198+0 | 8531 | 12.8 |
| callers flush_writes | 176 | 17124 | 297 | 177+0 | 7272 | 11.9 |
| callers tie_to | 175 | 21588 | 264 | 175+0 | 8324 | 12.5 |
| callers primary_key | 159 | 47004 | 302 | 160+0 | 8748 | 11.1 |
| callers oplog_sync | 142 | 14928 | 294 | 143+0 | 8001 | 13.4 |
| callers pull_once | 116 | 12033 | 277 | 130+0 | 8508 | 10.2 |
| callers oplog_commit | 127 | 12440 | 304 | 127+0 | 7552 | 10.5 |
| callers open_concurrent_with_oplog | 118 | 17539 | 276 | 118+0 | 10069 | 13.7 |
| callers remove_db | 116 | 13661 | 264 | 116+0 | 8239 | 10.0 |
| callers make_eid | 101 | 13927 | 290 | 113+0 | 8485 | 14.0 |
| callers open_standalone | 112 | 14614 | 278 | 113+1 | 8738 | 12.1 |
| callers publish_since | 99 | 11886 | 269 | 105+0 | 8558 | 10.4 |
| callers tie_text | 103 | 12660 | 298 | 103+1 | 7969 | 12.2 |

**中央値**: ast-grep 290ms / 21588B vs kenning 12.2ms / 8324B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 415 | 1698110 | 1262 | 22702 | 75x |
| impact define_himo | 453 | 1718435 | 1322 | 22646 | 76x |
| impact number | 158 | 704149 | 509 | 12749 | 55x |
| impact entity_in | 739 | 4380893 | 2754 | 41964 | 104x |
| impact eid_local | 1434 | 8942479 | 5502 | 32889 | 272x |

中央値: **76x**、tool 呼び出し 1322 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Drop | 31 | 43086 | 27 | 16.4x |
| impls Default | 20 | 24336 | 18 | 15.4x |
| impls Debug | 18 | 9115 | 6 | 6.2x |
| impls Sync | 18 | 32422 | 19 | 21.5x |
| impls Send | 17 | 34707 | 20 | 24.2x |

中央値: **16.4x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 917705 | 11445 | 80x |
| CHANGELOG.md | 425964 | 5979 | 71x |
| lib.rs | 319006 | 9713 | 33x |
| live.rs | 264241 | 9658 | 27x |
| oplog.rs | 123369 | 7532 | 16x |

中央値: **33x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.8x**

**faceted** (`kind:method vis:pub test:0`): 1294 件 282.791µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 6057 | 21 | 792503 | 6057 | 24 | 940373 | = |
| assert_eq | 2698 | 11 | 351030 | 2698 | 18 | 433026 | = |
| Engine | 3290 | 9 | 459233 | 3283 | 19 | 557205 | ≠ |
| entity | 2871 | 9 | 402637 | 2871 | 19 | 480886 | = |
| assert | 4508 | 10 | 589197 | 4502 | 21 | 732444 | ≠ |
| format | 1675 | 11 | 227965 | 1675 | 18 | 255989 | = |
| return | 1374 | 9 | 166642 | 1374 | 14 | 186267 | = |
| himo_id | 1028 | 11 | 145278 | 1028 | 13 | 168045 | = |
| engine | 3290 | 11 | 459233 | 3283 | 19 | 557205 | ≠ |
| cleanup | 1037 | 10 | 98434 | 1037 | 14 | 132237 | = |
| ValueType | 959 | 10 | 122140 | 959 | 14 | 142386 | = |
| enchudb_oplog | 909 | 10 | 123416 | 909 | 13 | 141429 | = |
| enchudb | 2608 | 10 | 340313 | 2561 | 18 | 385234 | ≠ |
| collect | 1200 | 9 | 162057 | 1191 | 15 | 179967 | ≠ |
| author | 1195 | 10 | 175780 | 1195 | 13 | 221110 | = |
| Ordering | 865 | 11 | 115393 | 865 | 13 | 130873 | = |
| record | 1580 | 9 | 232382 | 1580 | 15 | 287648 | = |
| insert | 1067 | 10 | 146078 | 1067 | 15 | 168755 | = |
| String | 1199 | 11 | 152644 | 1185 | 16 | 171066 | ≠ |
| commit | 1416 | 11 | 205909 | 1416 | 15 | 246996 | = |

**一致**: 14/20 問でヒット数が同一。差の内訳: 少ない 6 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 10.5ms vs text 15.3ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 623.041µs
index: 354 files / 5425 symbols / 63392 call-sites
  kind=fn                                              =    2510 件  [583ns]
  pub fn                                               =      86 件  [3.666µs]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =      69 件  [166ns]
  callers new (名前一致)                                   =    2972 件  [708ns]
  callers new (確実 逆引き)                                 =       2 件  [125ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 49 files / 9103 call-sites (うち repo 内 1804) / repo 内確定率 86.3% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| new | 288 | 7 | 243 | 38 |
| query | 145 | 141 | 0 | 4 |
| txt | 90 | 88 | 0 | 2 |
| next | 70 | 5 | 56 | 9 |
| num | 70 | 68 | 0 | 2 |
| kenning | 28 | 16 | 0 | 12 |
| file_paths | 21 | 20 | 0 | 1 |
| indexed | 19 | 18 | 0 | 1 |
| hit | 12 | 10 | 0 | 2 |
| update | 12 | 6 | 0 | 6 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers query | 23784 | 4 | 7785 | 1 | 3.1x |
| callers txt | 29370 | 10 | 7312 | 1 | 4.0x |
| callers num | 24211 | 9 | 7227 | 1 | 3.4x |
| callers tmp | 8711 | 3 | 6126 | 1 | 1.4x |
| callers write_fixture | 5968 | 2 | 6880 | 1 | 0.9x |
| callers index | 32369 | 11 | 6164 | 1 | 5.3x |
| callers ref_of | 15356 | 7 | 4988 | 1 | 3.1x |
| callers file_paths | 9160 | 5 | 2324 | 1 | 3.9x |
| callers open_ro | 8722 | 5 | 2478 | 1 | 3.5x |
| callers indexed | 3072 | 2 | 2471 | 1 | 1.2x |
| callers parse_opts | 9358 | 5 | 1981 | 1 | 4.7x |
| callers kenning | 20761 | 10 | 2598 | 1 | 8.0x |
| callers line_of | 5636 | 3 | 2125 | 1 | 2.7x |
| callers median_f | 4897 | 3 | 1665 | 1 | 2.9x |
| callers median_u | 5027 | 3 | 1688 | 1 | 3.0x |
| callers tmp_tree | 2786 | 2 | 1754 | 1 | 1.6x |
| callers col_of | 3343 | 2 | 1877 | 1 | 1.8x |
| callers fmt_sym | 4702 | 3 | 1512 | 1 | 3.1x |
| callers changes_fixture | 2790 | 2 | 1600 | 1 | 1.7x |
| callers crate_key | 3631 | 2 | 1688 | 1 | 2.2x |

**中央値**: 圧縮比 **3.1x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 7.3ms vs kenning 5.0ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers query | 132 | 13667 | 64 | 141+0 | 7785 | 5.1 |
| callers txt | 84 | 9198 | 58 | 88+0 | 7312 | 7.0 |
| callers num | 61 | 6745 | 56 | 68+0 | 7227 | 6.2 |
| callers tmp | 57 | 3709 | 58 | 57+0 | 6126 | 4.8 |
| callers write_fixture | 54 | 32926 | 54 | 54+0 | 6880 | 4.8 |
| callers index | 53 | 3494 | 64 | 53+0 | 6164 | 5.2 |
| callers ref_of | 31 | 4288 | 58 | 32+0 | 4988 | 5.3 |
| callers file_paths | 20 | 1827 | 56 | 20+0 | 2324 | 5.3 |
| callers open_ro | 19 | 1987 | 52 | 19+0 | 2478 | 5.1 |
| callers indexed | 18 | 1466 | 59 | 18+0 | 2471 | 4.8 |
| callers parse_opts | 17 | 1531 | 59 | 17+0 | 1981 | 5.0 |
| callers kenning | 16 | 1774 | 62 | 16+0 | 2598 | 5.6 |
| callers line_of | 12 | 1709 | 57 | 13+0 | 2125 | 5.0 |
| callers median_f | 0 | 0 | 59 | 13+0 | 1665 | 4.6 |
| callers median_u | 0 | 0 | 52 | 13+0 | 1688 | 4.8 |
| callers tmp_tree | 12 | 1045 | 50 | 12+0 | 1754 | 5.0 |
| callers col_of | 11 | 1591 | 59 | 11+0 | 1877 | 4.7 |
| callers fmt_sym | 1 | 101 | 55 | 11+0 | 1512 | 4.6 |
| callers changes_fixture | 10 | 843 | 54 | 10+0 | 1600 | 4.8 |
| callers crate_key | 10 | 1040 | 60 | 10+0 | 1688 | 4.9 |

**中央値**: ast-grep 58ms / 1774B vs kenning 5.0ms / 2471B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact query | 80 | 199972 | 162 | 6877 | 29x |
| impact txt | 99 | 491442 | 294 | 9307 | 53x |
| impact num | 91 | 362049 | 249 | 8619 | 42x |
| impact tmp | 83 | 191626 | 167 | 9838 | 19x |
| impact write_fixture | 83 | 188883 | 166 | 9849 | 19x |

中央値: **29x**、tool 呼び出し 167 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| parse.rs | 119557 | 10519 | 11x |
| core.rs | 106797 | 11474 | 9x |
| query.rs | 84336 | 10832 | 8x |
| tests.rs | 54489 | 11897 | 5x |
| bench.rs | 41270 | 5643 | 7x |

中央値: **8x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1728 B / 2 回 → def 153 B / 1 回 = **10.6x**

**faceted** (`kind:method vis:pub test:0`): 6 件 96.208µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| String | 678 | 7 | 84046 | 678 | 6 | 94491 | = |
| kenning | 552 | 7 | 78972 | 551 | 6 | 98076 | ≠ |
| unwrap | 600 | 7 | 74212 | 600 | 6 | 88758 | = |
| contains | 344 | 7 | 50250 | 344 | 6 | 64164 | = |
| callers | 450 | 6 | 58964 | 450 | 6 | 86199 | = |
| assert | 544 | 7 | 76181 | 544 | 5 | 101516 | = |
| return | 264 | 7 | 25427 | 264 | 5 | 28058 | = |
| println | 376 | 7 | 52406 | 376 | 5 | 55448 | = |
| enchudb | 242 | 7 | 32256 | 221 | 5 | 36902 | ≠ |
| format | 206 | 7 | 29593 | 206 | 5 | 32802 | = |
| to_string | 273 | 7 | 35426 | 273 | 5 | 40222 | = |
| method | 246 | 7 | 37487 | 246 | 5 | 43023 | = |
| container | 200 | 6 | 27621 | 200 | 5 | 30924 | = |
| collect | 184 | 6 | 23579 | 184 | 5 | 26003 | = |
| is_empty | 147 | 6 | 16403 | 147 | 5 | 18233 | = |
| assert_eq | 154 | 7 | 20476 | 154 | 5 | 27075 | = |
| eprintln | 141 | 7 | 20776 | 141 | 5 | 21650 | = |
| Option | 141 | 7 | 17820 | 141 | 5 | 20136 | = |
| update | 160 | 6 | 23650 | 160 | 5 | 27361 | = |
| process | 120 | 7 | 15333 | 120 | 5 | 17321 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 6.8ms vs text 5.2ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 532.458µs
index: 49 files / 582 symbols / 9103 call-sites
  kind=fn                                              =     418 件  [250ns]
  pub fn                                               =      26 件  [375ns]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =       3 件  [167ns]
  callers new (名前一致)                                   =     250 件  [291ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```

