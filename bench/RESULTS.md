# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 770 files / 38428 call-sites (うち repo 内 21391) / repo 内確定率 70.8% / **baked (SCIP)** / db 実 14.7 MB (vocab 索引 1.6 MB)

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| join | 257 | 1 | 198 | 58 |
| fmt | 253 | 9 | 50 | 194 |
| ready | 245 | 40 | 48 | 157 |
| as_ref | 228 | 1 | 202 | 25 |
| reserve | 90 | 25 | 44 | 21 |
| pop | 86 | 29 | 50 | 7 |
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
| callers registration | 27112 | 10 | 7130 | 1 | 3.8x |
| callers unbounded_channel | 23621 | 17 | 8499 | 1 | 2.8x |
| callers shared | 15239 | 3 | 9691 | 1 | 1.6x |
| callers data | 12701 | 4 | 7866 | 1 | 1.6x |
| callers enable_all | 43935 | 38 | 6317 | 1 | 7.0x |
| callers insert_at | 11934 | 4 | 8425 | 1 | 1.4x |
| callers worker_threads | 41996 | 37 | 6823 | 1 | 6.2x |
| callers run_one | 6011 | 3 | 5765 | 1 | 1.0x |
| callers measure | 7252 | 3 | 6052 | 1 | 1.2x |
| callers sleep_until | 24365 | 16 | 6676 | 1 | 3.6x |
| callers filled | 26987 | 19 | 5068 | 1 | 5.3x |
| callers changed | 23502 | 13 | 7518 | 1 | 3.1x |
| callers asyncify | 19574 | 25 | 4769 | 1 | 4.1x |
| callers made_progress | 18864 | 13 | 4826 | 1 | 3.9x |
| callers remaining | 25863 | 18 | 5097 | 1 | 5.1x |
| callers poll_proceed | 18885 | 13 | 4538 | 1 | 4.2x |
| callers push_front | 18865 | 12 | 3785 | 1 | 5.0x |
| callers put_slice | 34037 | 24 | 10963 | 1 | 3.1x |
| callers run_until | 11254 | 6 | 3704 | 1 | 3.0x |
| callers child_token | 6534 | 4 | 3984 | 1 | 1.6x |

**中央値**: 圧縮比 **3.6x**、grep 経路の tool 呼び出し 13 回 → 1 回

**単発 wall-clock 中央値**: rg 15.2ms vs kenning 7.8ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers registration | 89 | 15839 | 191 | 112+0 | 7130 | 7.8 |
| callers unbounded_channel | 30 | 3663 | 195 | 66+0 | 8499 | 8.2 |
| callers shared | 66 | 10400 | 180 | 65+1 | 9691 | 7.4 |
| callers data | 0 | 0 | 192 | 63+1 | 7866 | 7.9 |
| callers enable_all | 64 | 14951 | 203 | 63+1 | 6317 | 8.3 |
| callers insert_at | 63 | 8442 | 183 | 63+0 | 8425 | 7.3 |
| callers worker_threads | 56 | 13681 | 193 | 56+1 | 6823 | 8.2 |
| callers run_one | 45 | 4116 | 191 | 45+0 | 5765 | 7.0 |
| callers measure | 42 | 4569 | 179 | 41+1 | 6052 | 7.5 |
| callers sleep_until | 39 | 5150 | 187 | 39+1 | 6676 | 8.3 |
| callers filled | 23 | 2719 | 191 | 36+0 | 5068 | 7.1 |
| callers changed | 42 | 4783 | 188 | 33+18 | 7518 | 8.0 |
| callers asyncify | 32 | 4586 | 192 | 32+0 | 4769 | 7.3 |
| callers made_progress | 27 | 2945 | 177 | 28+8 | 4826 | 7.7 |
| callers remaining | 24 | 2930 | 188 | 28+4 | 5097 | 7.8 |
| callers poll_proceed | 1 | 147 | 188 | 25+1 | 4538 | 7.7 |
| callers push_front | 24 | 3171 | 204 | 23+1 | 3785 | 7.2 |
| callers put_slice | 74 | 7756 | 210 | 22+52 | 10963 | 8.1 |
| callers run_until | 22 | 24679 | 194 | 22+2 | 3704 | 7.9 |
| callers child_token | 21 | 2881 | 185 | 21+0 | 3984 | 7.7 |

**中央値**: ast-grep 191ms / 4586B vs kenning 7.8ms / 6317B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact registration | 294 | 1949992 | 1258 | 27095 | 72x |
| impact unbounded_channel | 116 | 1900699 | 1226 | 20613 | 92x |
| impact shared | 73 | 243895 | 208 | 11042 | 22x |
| impact data | 29 | 43839 | 60 | 14371 | 3x |
| impact enable_all | 190 | 1933432 | 1313 | 20119 | 96x |

中央値: **72x**、tool 呼び出し 1226 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Debug | 140 | 117626 | 101 | 25.9x |
| impls Drop | 92 | 110618 | 89 | 23.4x |
| impls Future | 77 | 119429 | 95 | 26.9x |
| impls Sync | 60 | 59277 | 46 | 13.4x |
| impls Stream | 55 | 85591 | 72 | 18.3x |

中央値: **23.4x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| CHANGELOG.md | 143415 | 4888 | 29x |
| named_pipe.rs | 99154 | 11962 | 8x |
| udp.rs | 84068 | 12020 | 7x |
| bounded.rs | 64865 | 10493 | 6x |
| builder.rs | 59923 | 10801 | 6x |

中央値: **7x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1637 B / 2 回 → def 220 B / 1 回 = **6.4x**

**faceted** (`kind:method vis:pub test:0`): 1405 件 382.709µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 15 | 319769 | 2701 | 24 | 377187 | = |
| unwrap | 2686 | 15 | 318940 | 2686 | 24 | 369000 | = |
| Result | 2839 | 16 | 380851 | 2839 | 27 | 439632 | = |
| runtime | 2482 | 32 | 327851 | 2482 | 24 | 375751 | = |
| stream | 3200 | 15 | 415027 | 3200 | 22 | 477363 | = |
| github | 1571 | 15 | 189393 | 1571 | 17 | 247613 | = |
| assert | 6693 | 17 | 805288 | 6693 | 32 | 935669 | = |
| future | 2545 | 16 | 328368 | 2545 | 24 | 369447 | = |
| return | 3257 | 16 | 443672 | 3257 | 26 | 513451 | = |
| handle | 2879 | 16 | 378121 | 2879 | 23 | 431289 | = |
| thread | 2868 | 16 | 380972 | 2868 | 33 | 432457 | = |
| Context | 1584 | 16 | 213427 | 1584 | 21 | 241280 | = |
| feature | 1227 | 16 | 148931 | 1227 | 20 | 161494 | = |
| struct | 1414 | 16 | 166367 | 1414 | 23 | 191451 | = |
| channel | 1280 | 14 | 165719 | 1280 | 25 | 194600 | = |
| method | 1186 | 15 | 173192 | 1186 | 20 | 201872 | = |
| socket | 1847 | 15 | 237089 | 1847 | 17 | 274705 | = |
| buffer | 1225 | 15 | 166630 | 1225 | 18 | 195053 | = |
| unsafe | 1067 | 15 | 136843 | 1067 | 18 | 152451 | = |
| function | 1070 | 15 | 153227 | 1070 | 21 | 179752 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 15.8ms vs text 23.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 829.042µs
index: 770 files / 7723 symbols / 38428 call-sites
  kind=fn                                              =    2446 件  [583ns]
  pub fn                                               =     107 件  [4.25µs]
  pub async fn 非test                                   =      42 件  [4.25µs]
  def new                                              =     280 件  [208ns]
  callers new (名前一致)                                   =    2424 件  [625ns]
  callers new (確実 逆引き)                                 =       1 件  [125ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 207 files / 17953 call-sites (うち repo 内 10394) / repo 内確定率 92.7% / **baked (SCIP)** / db 実 8.3 MB (vocab 索引 0.8 MB)

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
| callers arg | 97701 | 11 | 8523 | 1 | 11.5x |
| callers parse_low_raw | 75670 | 3 | 8164 | 1 | 9.3x |
| callers create | 57957 | 10 | 7120 | 1 | 8.1x |
| callers is_ignore | 33306 | 10 | 7900 | 1 | 4.2x |
| callers create_dir | 12611 | 4 | 5810 | 1 | 2.2x |
| callers prev | 11173 | 2 | 8236 | 1 | 1.4x |
| callers unwrap_switch | 12063 | 3 | 7577 | 1 | 1.6x |
| callers exact | 11725 | 2 | 8438 | 1 | 1.4x |
| callers assert_err | 14363 | 7 | 6327 | 1 | 2.3x |
| callers expected_no_line_number | 9655 | 3 | 7260 | 1 | 1.3x |
| callers test | 8679 | 3 | 6165 | 1 | 1.4x |
| callers create_bytes | 11719 | 6 | 6096 | 1 | 1.9x |
| callers add_child | 8302 | 3 | 6428 | 1 | 1.3x |
| callers assert_paths | 5105 | 2 | 4518 | 1 | 1.1x |
| callers unwrap_value | 6281 | 3 | 4992 | 1 | 1.3x |
| callers inexact | 6270 | 2 | 4686 | 1 | 1.3x |
| callers as_byte | 16289 | 10 | 4684 | 1 | 3.5x |
| callers text | 4050 | 2 | 3632 | 1 | 1.1x |
| callers seq | 4753 | 2 | 3880 | 1 | 1.2x |
| callers bstr | 4612 | 2 | 3752 | 1 | 1.2x |

**中央値**: 圧縮比 **1.4x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 10.0ms vs kenning 6.2ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers arg | 8 | 710 | 135 | 674+8 | 8523 | 6.2 |
| callers parse_low_raw | 545 | 77997 | 132 | 545+0 | 8164 | 6.7 |
| callers create | 9 | 979 | 132 | 442+5 | 7120 | 6.4 |
| callers is_ignore | 4 | 441 | 115 | 134+0 | 7900 | 6.6 |
| callers create_dir | 1 | 90 | 115 | 100+0 | 5810 | 6.2 |
| callers prev | 0 | 0 | 137 | 72+0 | 8236 | 5.7 |
| callers unwrap_switch | 39 | 4612 | 125 | 69+0 | 7577 | 6.3 |
| callers exact | 9 | 1277 | 129 | 66+8 | 8438 | 5.4 |
| callers assert_err | 0 | 0 | 118 | 60+0 | 6327 | 6.4 |
| callers expected_no_line_number | 55 | 26623 | 126 | 55+0 | 7260 | 5.4 |
| callers test | 55 | 35090 | 130 | 55+0 | 6165 | 5.7 |
| callers create_bytes | 1 | 113 | 119 | 47+0 | 6096 | 6.6 |
| callers add_child | 38 | 6645 | 117 | 38+0 | 6428 | 5.5 |
| callers assert_paths | 33 | 12988 | 112 | 33+0 | 4518 | 6.3 |
| callers unwrap_value | 32 | 4087 | 122 | 32+0 | 4992 | 7.1 |
| callers inexact | 2 | 1063 | 118 | 29+1 | 4686 | 6.1 |
| callers as_byte | 27 | 3717 | 122 | 27+0 | 4684 | 6.8 |
| callers text | 0 | 0 | 126 | 27+0 | 3632 | 5.4 |
| callers seq | 0 | 0 | 123 | 25+0 | 3880 | 5.8 |
| callers bstr | 0 | 0 | 131 | 23+0 | 3752 | 6.2 |

**中央値**: ast-grep 125ms / 1063B vs kenning 6.2ms / 6327B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact arg | 1 | 97701 | 11 | 406 | 241x |
| impact parse_low_raw | 105 | 207703 | 211 | 6327 | 33x |
| impact create | 2 | 59611 | 12 | 495 | 120x |
| impact is_ignore | 74 | 302364 | 227 | 15362 | 20x |
| impact create_dir | 2 | 14265 | 6 | 499 | 29x |

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
| defs.rs | 235436 | 7819 | 30x |
| raw.csv | 226710 | 106 | 2139x |
| standard.rs | 136288 | 12373 | 11x |
| raw.csv | 114939 | 109 | 1054x |
| raw.csv | 91922 | 107 | 859x |

中央値: **859x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1445 B / 2 回 → def 239 B / 1 回 = **6.1x**

**faceted** (`kind:method vis:pub test:0`): 477 件 182.959µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 11 | 856322 | 3298 | 12 | 746164 | = |
| ignore | 3475 | 10 | 566374 | 3473 | 14 | 584698 | ≠ |
| OpenSubtitles2016 | 2349 | 9 | 637772 | 2349 | 11 | 547110 | = |
| ripgrep | 1886 | 8 | 244360 | 1885 | 17 | 261103 | ≠ |
| benchsuite | 1896 | 9 | 507042 | 1896 | 10 | 440490 | = |
| Sherlock | 2133 | 11 | 421572 | 2139 | 13 | 383183 | ≠ |
| Holmes | 1679 | 9 | 396369 | 1688 | 12 | 349402 | ≠ |
| sample | 1580 | 10 | 406148 | 1580 | 10 | 343953 | = |
| Холмс | 1406 | 9 | 379069 | 1406 | 10 | 339776 | = |
| Шерлок | 1206 | 10 | 330816 | 1206 | 10 | 295218 | = |
| assert_eq | 1296 | 10 | 158656 | 1296 | 11 | 177179 | = |
| unwrap | 1306 | 10 | 165821 | 1306 | 12 | 185555 | = |
| LC_ALL | 1103 | 9 | 281795 | 1103 | 8 | 244529 | = |
| PM_RESUME | 969 | 10 | 175046 | 969 | 9 | 173294 | = |
| matcher | 1200 | 10 | 151245 | 1194 | 11 | 171785 | ≠ |
| github | 954 | 9 | 121749 | 886 | 10 | 116086 | ≠ |
| BurntSushi | 806 | 8 | 102017 | 806 | 10 | 103301 | = |
| matches | 972 | 10 | 123456 | 972 | 12 | 138859 | = |
| return | 1307 | 10 | 164960 | 1308 | 13 | 188021 | ≠ |
| pattern | 1002 | 10 | 143447 | 1002 | 12 | 154678 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.8ms vs text 11.2ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 1.022708ms
index: 207 files / 3192 symbols / 17953 call-sites
  kind=fn                                              =     705 件  [291ns]
  pub fn                                               =      30 件  [1.541µs]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =      84 件  [208ns]
  callers new (名前一致)                                   =    1093 件  [416ns]
  callers new (確実 逆引き)                                 =       3 件  [166ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 366 files / 65719 call-sites (うち repo 内 29711) / repo 内確定率 89.4% / **baked (SCIP)** / db 実 14.2 MB (vocab 索引 1.6 MB)

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| clone | 1173 | 3 | 1157 | 13 |
| tag | 179 | 153 | 0 | 26 |
| with_capacity | 152 | 39 | 104 | 9 |
| from | 82 | 7 | 75 | 0 |
| untie | 73 | 68 | 2 | 3 |
| create_growable_with_capacity | 65 | 60 | 0 | 5 |
| broadcast | 51 | 44 | 0 | 7 |
| get_text_owned | 48 | 46 | 0 | 2 |
| sync | 37 | 22 | 0 | 15 |
| sync_tables_enabled | 33 | 26 | 0 | 7 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 4 + 候補 0 = 検討対象 4 行、ノイズ率 27%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 161632 | 75 | 6815 | 1 | 23.7x |
| callers define_himo | 187033 | 90 | 7817 | 1 | 23.9x |
| callers number | 144253 | 60 | 8145 | 1 | 17.7x |
| callers entity_in | 185239 | 91 | 8817 | 1 | 21.0x |
| callers eid_local | 94144 | 38 | 8148 | 1 | 11.6x |
| callers create_standalone | 115682 | 56 | 8081 | 1 | 14.3x |
| callers define_himo_in | 188967 | 96 | 9697 | 1 | 19.5x |
| callers define_table | 176230 | 94 | 8621 | 1 | 20.4x |
| callers flush_writes | 125831 | 70 | 7440 | 1 | 16.9x |
| callers tie_to | 108884 | 55 | 8466 | 1 | 12.9x |
| callers primary_key | 124986 | 60 | 8915 | 1 | 14.0x |
| callers oplog_sync | 135004 | 74 | 8167 | 1 | 16.5x |
| callers pull_once | 77007 | 36 | 8821 | 1 | 8.7x |
| callers oplog_commit | 111598 | 65 | 7696 | 1 | 14.5x |
| callers open_concurrent_with_oplog | 87920 | 45 | 10302 | 1 | 8.5x |
| callers make_eid | 82216 | 39 | 8616 | 1 | 9.5x |
| callers remove_db | 85514 | 49 | 8404 | 1 | 10.2x |
| callers open_standalone | 79502 | 44 | 8909 | 1 | 8.9x |
| callers tie_text | 71979 | 38 | 8169 | 1 | 8.8x |
| callers publish_since | 61055 | 30 | 8727 | 1 | 7.0x |

**中央値**: 圧縮比 **14.3x**、grep 経路の tool 呼び出し 60 回 → 1 回

**単発 wall-clock 中央値**: rg 11.6ms vs kenning 9.6ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 439 | 44639 | 315 | 439+3 | 6815 | 9.7 |
| callers define_himo | 399 | 46822 | 341 | 399+0 | 7817 | 8.9 |
| callers number | 285 | 40786 | 305 | 295+0 | 8145 | 8.5 |
| callers entity_in | 272 | 34159 | 325 | 274+0 | 8817 | 10.2 |
| callers eid_local | 190 | 27334 | 328 | 233+1 | 8148 | 9.2 |
| callers create_standalone | 230 | 29226 | 325 | 230+0 | 8081 | 9.4 |
| callers define_himo_in | 228 | 34164 | 312 | 229+0 | 9697 | 9.7 |
| callers define_table | 208 | 25551 | 320 | 208+0 | 8621 | 9.6 |
| callers flush_writes | 178 | 17358 | 323 | 179+0 | 7440 | 10.5 |
| callers tie_to | 179 | 22133 | 326 | 179+0 | 8466 | 9.6 |
| callers primary_key | 166 | 48195 | 329 | 167+0 | 8915 | 9.2 |
| callers oplog_sync | 144 | 15175 | 311 | 145+0 | 8167 | 10.7 |
| callers pull_once | 120 | 12528 | 300 | 135+0 | 8821 | 8.7 |
| callers oplog_commit | 131 | 12904 | 317 | 131+0 | 7696 | 8.0 |
| callers open_concurrent_with_oplog | 119 | 17708 | 342 | 119+0 | 10302 | 11.7 |
| callers make_eid | 104 | 14430 | 344 | 116+0 | 8616 | 11.0 |
| callers remove_db | 116 | 13661 | 317 | 116+0 | 8404 | 8.1 |
| callers open_standalone | 112 | 14614 | 336 | 113+1 | 8909 | 10.3 |
| callers tie_text | 109 | 13437 | 372 | 109+1 | 8169 | 11.9 |
| callers publish_since | 102 | 12259 | 335 | 108+0 | 8727 | 7.8 |

**中央値**: ast-grep 325ms / 22133B vs kenning 9.6ms / 8466B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 418 | 1744743 | 1288 | 23574 | 74x |
| impact define_himo | 469 | 1788630 | 1372 | 24293 | 74x |
| impact number | 163 | 729340 | 525 | 12895 | 57x |
| impact entity_in | 782 | 4657378 | 2914 | 49678 | 94x |
| impact eid_local | 1486 | 9414119 | 5754 | 39654 | 237x |

中央値: **74x**、tool 呼び出し 1372 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Drop | 32 | 43582 | 27 | 16.1x |
| impls Default | 20 | 24306 | 18 | 15.4x |
| impls Debug | 18 | 9115 | 6 | 6.2x |
| impls Sync | 18 | 31882 | 19 | 21.1x |
| impls Send | 17 | 34130 | 20 | 23.8x |

中央値: **16.1x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 939368 | 11308 | 83x |
| CHANGELOG.md | 448494 | 6302 | 71x |
| lib.rs | 320355 | 9738 | 33x |
| live.rs | 264259 | 9683 | 27x |
| oplog.rs | 140009 | 8049 | 17x |

中央値: **33x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.8x**

**faceted** (`kind:method vis:pub test:0`): 1302 件 232.958µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 6311 | 21 | 829063 | 6311 | 26 | 986392 | = |
| assert_eq | 2851 | 11 | 373880 | 2851 | 21 | 462360 | = |
| Engine | 3412 | 10 | 479704 | 3405 | 22 | 581761 | ≠ |
| entity | 2980 | 12 | 420926 | 2980 | 21 | 503744 | = |
| assert | 4768 | 12 | 628457 | 4762 | 23 | 782260 | ≠ |
| format | 1730 | 12 | 235620 | 1730 | 20 | 264799 | = |
| return | 1392 | 10 | 168862 | 1392 | 17 | 188841 | = |
| himo_id | 1061 | 11 | 150258 | 1061 | 15 | 174510 | = |
| engine | 3412 | 12 | 479704 | 3405 | 22 | 581761 | ≠ |
| cleanup | 1098 | 12 | 104735 | 1098 | 15 | 140577 | = |
| ValueType | 1000 | 10 | 128071 | 1000 | 15 | 149432 | = |
| enchudb_oplog | 928 | 11 | 126267 | 928 | 16 | 144835 | = |
| collect | 1237 | 11 | 167243 | 1228 | 16 | 186525 | ≠ |
| enchudb | 2657 | 12 | 347784 | 2610 | 21 | 392879 | ≠ |
| Ordering | 919 | 10 | 123068 | 919 | 14 | 140084 | = |
| author | 1241 | 11 | 183683 | 1241 | 14 | 230688 | = |
| record | 1697 | 11 | 252436 | 1697 | 17 | 312919 | = |
| insert | 1098 | 12 | 150521 | 1098 | 17 | 174418 | = |
| String | 1221 | 12 | 155666 | 1207 | 18 | 174526 | ≠ |
| commit | 1462 | 11 | 212830 | 1462 | 17 | 255534 | = |

**一致**: 14/20 問でヒット数が同一。差の内訳: 少ない 6 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 11.5ms vs text 17.4ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 918.083µs
index: 366 files / 5573 symbols / 65719 call-sites
  kind=fn                                              =    2613 件  [625ns]
  pub fn                                               =      86 件  [3.666µs]
  pub async fn 非test                                   =       0 件  [375ns]
  def new                                              =      69 件  [166ns]
  callers new (名前一致)                                   =    3065 件  [709ns]
  callers new (確実 逆引き)                                 =       2 件  [125ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 49 files / 10024 call-sites (うち repo 内 2007) / repo 内確定率 84.7% / **baked (SCIP)** / db 実 4.7 MB (vocab 索引 0.4 MB)

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| tmp | 75 | 66 | 0 | 9 |
| parse_opts | 19 | 18 | 0 | 1 |
| median_f | 14 | 13 | 0 | 1 |
| read_meta | 11 | 10 | 0 | 1 |
| suggest_similar | 10 | 9 | 0 | 1 |
| sym_qual | 10 | 9 | 0 | 1 |
| edit_and_changes | 8 | 7 | 0 | 1 |
| fs_gate | 8 | 7 | 0 | 1 |
| meta_root | 8 | 7 | 0 | 1 |
| type_recv | 8 | 7 | 0 | 1 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers query | 26074 | 4 | 7947 | 1 | 3.3x |
| callers txt | 30562 | 10 | 7020 | 1 | 4.4x |
| callers num | 28576 | 10 | 7442 | 1 | 3.8x |
| callers tmp | 9305 | 3 | 6310 | 1 | 1.5x |
| callers index | 35324 | 11 | 6328 | 1 | 5.6x |
| callers write_fixture | 6542 | 2 | 7085 | 1 | 0.9x |
| callers ref_of | 15860 | 7 | 5628 | 1 | 2.8x |
| callers file_paths | 9251 | 5 | 2546 | 1 | 3.6x |
| callers open_ro | 8969 | 5 | 2711 | 1 | 3.3x |
| callers indexed | 3151 | 2 | 2728 | 1 | 1.2x |
| callers parse_opts | 10031 | 5 | 2196 | 1 | 4.6x |
| callers kenning | 21101 | 10 | 2712 | 1 | 7.8x |
| callers crate_key | 4113 | 2 | 2214 | 1 | 1.9x |
| callers line_of | 5640 | 3 | 2239 | 1 | 2.5x |
| callers median_f | 4897 | 3 | 1780 | 1 | 2.8x |
| callers median_u | 5027 | 3 | 1803 | 1 | 2.8x |
| callers tmp_tree | 3371 | 2 | 2025 | 1 | 1.7x |
| callers omitted | 6686 | 4 | 1753 | 1 | 3.8x |
| callers col_of | 3347 | 2 | 1990 | 1 | 1.7x |
| callers expr_recv | 3833 | 2 | 1993 | 1 | 1.9x |

**中央値**: 圧縮比 **2.8x**、grep 経路の tool 呼び出し 4 回 → 1 回

**単発 wall-clock 中央値**: rg 8.3ms vs kenning 5.3ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers query | 149 | 15381 | 80 | 161+0 | 7947 | 5.3 |
| callers txt | 94 | 10163 | 85 | 98+0 | 7020 | 5.4 |
| callers num | 69 | 7844 | 80 | 81+0 | 7442 | 6.2 |
| callers tmp | 66 | 4285 | 80 | 66+0 | 6310 | 5.5 |
| callers index | 62 | 4079 | 85 | 62+0 | 6328 | 6.0 |
| callers write_fixture | 62 | 42295 | 70 | 62+0 | 7085 | 5.1 |
| callers ref_of | 35 | 4792 | 73 | 36+0 | 5628 | 5.5 |
| callers file_paths | 21 | 1915 | 66 | 21+0 | 2546 | 5.3 |
| callers open_ro | 20 | 2089 | 64 | 20+0 | 2711 | 5.1 |
| callers indexed | 19 | 1543 | 73 | 19+0 | 2728 | 5.3 |
| callers parse_opts | 18 | 1612 | 72 | 18+0 | 2196 | 5.6 |
| callers kenning | 16 | 1774 | 75 | 16+0 | 2712 | 5.0 |
| callers crate_key | 13 | 1417 | 70 | 13+0 | 2214 | 5.0 |
| callers line_of | 12 | 1713 | 76 | 13+0 | 2239 | 5.3 |
| callers median_f | 0 | 0 | 65 | 13+0 | 1780 | 5.8 |
| callers median_u | 0 | 0 | 65 | 13+0 | 1803 | 4.7 |
| callers tmp_tree | 13 | 1136 | 68 | 13+0 | 2025 | 6.0 |
| callers omitted | 0 | 0 | 65 | 12+0 | 1753 | 5.1 |
| callers col_of | 11 | 1595 | 69 | 11+0 | 1990 | 5.3 |
| callers expr_recv | 11 | 1435 | 66 | 11+0 | 1993 | 5.1 |

**中央値**: ast-grep 72ms / 1774B vs kenning 5.3ms / 2711B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact query | 90 | 225158 | 182 | 6950 | 32x |
| impact txt | 107 | 534236 | 314 | 10098 | 53x |
| impact num | 98 | 393107 | 267 | 9321 | 42x |
| impact tmp | 93 | 214939 | 187 | 10029 | 21x |
| impact index | 89 | 232498 | 187 | 9804 | 24x |

中央値: **32x**、tool 呼び出し 187 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| parse.rs | 136178 | 11092 | 12x |
| core.rs | 124439 | 11501 | 11x |
| query.rs | 90255 | 10851 | 8x |
| tests.rs | 59625 | 12003 | 5x |
| graph.rs | 53184 | 9459 | 6x |

中央値: **8x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1728 B / 2 回 → def 153 B / 1 回 = **11.6x**

**faceted** (`kind:method vis:pub test:0`): 6 件 99.333µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| String | 740 | 7 | 92058 | 740 | 7 | 103684 | = |
| kenning | 581 | 6 | 84444 | 580 | 7 | 106856 | ≠ |
| contains | 391 | 8 | 57566 | 391 | 7 | 73126 | = |
| unwrap | 648 | 7 | 80749 | 648 | 7 | 97234 | = |
| callers | 482 | 8 | 65773 | 482 | 7 | 92876 | = |
| assert | 594 | 8 | 84178 | 594 | 7 | 112016 | = |
| return | 283 | 7 | 27290 | 283 | 6 | 30242 | = |
| enchudb | 261 | 6 | 37670 | 240 | 7 | 41975 | ≠ |
| println | 397 | 7 | 55527 | 397 | 7 | 58649 | = |
| format | 231 | 8 | 33362 | 231 | 7 | 37081 | = |
| to_string | 299 | 6 | 38671 | 299 | 7 | 44130 | = |
| method | 271 | 8 | 43008 | 271 | 7 | 48063 | = |
| collect | 208 | 7 | 27313 | 208 | 6 | 30057 | = |
| is_empty | 164 | 7 | 18229 | 164 | 7 | 20345 | = |
| container | 197 | 8 | 27444 | 197 | 7 | 30573 | = |
| assert_eq | 160 | 6 | 21375 | 160 | 6 | 28244 | = |
| call_t | 147 | 7 | 17419 | 147 | 6 | 19272 | = |
| eprintln | 147 | 7 | 21865 | 147 | 5 | 22690 | = |
| EntityId | 121 | 8 | 15545 | 121 | 6 | 17093 | = |
| Option | 152 | 6 | 19212 | 152 | 7 | 21717 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 7.2ms vs text 6.6ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 666.875µs
index: 49 files / 627 symbols / 10024 call-sites
  kind=fn                                              =     446 件  [208ns]
  pub fn                                               =      27 件  [333ns]
  pub async fn 非test                                   =       0 件  [333ns]
  def new                                              =       3 件  [166ns]
  callers new (名前一致)                                   =     285 件  [208ns]
  callers new (確実 逆引き)                                 =       1 件  [125ns]
```

