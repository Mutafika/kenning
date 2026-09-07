# bench results

生成: `./bench/run.sh` (Darwin arm64) / 手法とモデルの定義は各表の直上に自己記述。
corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。

## corpus: tokio

corpus `~/.cache/kenning-bench/tokio` — 770 files / 38218 call-sites (うち repo 内 20390) / repo 内確定率 59.2% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| recv | 390 | 115 | 141 | 134 |
| load | 301 | 27 | 243 | 31 |
| iter | 247 | 33 | 152 | 62 |
| is_woken | 191 | 150 | 36 | 5 |
| get_ref | 137 | 80 | 20 | 37 |
| remove | 113 | 64 | 18 | 31 |
| try_write | 105 | 15 | 11 | 79 |
| poll_read | 103 | 25 | 16 | 62 |
| capacity | 99 | 25 | 40 | 34 |
| is_cancelled | 72 | 50 | 13 | 9 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 6 行 → 確実 2 + 候補 0 = 検討対象 4 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers sleep | 115856 | 70 | 15354 | 1 | 7.5x |
| callers registration | 27112 | 10 | 10059 | 1 | 2.7x |
| callers data | 12701 | 4 | 7708 | 1 | 1.6x |
| callers insert_at | 11934 | 4 | 8262 | 1 | 1.4x |
| callers unbounded_channel | 23621 | 17 | 10946 | 1 | 2.2x |
| callers task | 38461 | 22 | 9573 | 1 | 4.0x |
| callers run_one | 6011 | 3 | 5651 | 1 | 1.1x |
| callers changed | 23502 | 13 | 7404 | 1 | 3.2x |
| callers asyncify | 19574 | 25 | 4654 | 1 | 4.2x |
| callers filled | 26987 | 19 | 5602 | 1 | 4.8x |
| callers remaining | 25863 | 18 | 4981 | 1 | 5.2x |
| callers enable_all | 43935 | 38 | 9578 | 1 | 4.6x |
| callers sleep_until | 24365 | 16 | 6752 | 1 | 3.6x |
| callers push_front | 18865 | 12 | 3683 | 1 | 5.1x |
| callers put_slice | 34037 | 24 | 10820 | 1 | 3.1x |
| callers worker_threads | 41996 | 37 | 8923 | 1 | 4.7x |
| callers socketpair | 4362 | 3 | 3205 | 1 | 1.4x |
| callers current | 57043 | 35 | 11735 | 1 | 4.9x |
| callers poll_reserve | 5829 | 3 | 3485 | 1 | 1.7x |
| callers try_acquire_owned | 8278 | 5 | 3208 | 1 | 2.6x |

**中央値**: 圧縮比 **3.6x**、grep 経路の tool 呼び出し 17 回 → 1 回

**単発 wall-clock 中央値**: rg 22.9ms vs kenning 18.9ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers sleep | 152 | 19443 | 914 | 106+89 | 15354 | 27.6 |
| callers registration | 89 | 15839 | 705 | 79+19 | 10059 | 25.7 |
| callers data | 0 | 0 | 716 | 63+1 | 7708 | 28.1 |
| callers insert_at | 63 | 8442 | 592 | 63+0 | 8262 | 26.7 |
| callers unbounded_channel | 30 | 3663 | 425 | 52+14 | 10946 | 21.0 |
| callers task | 21 | 3548 | 444 | 51+18 | 9573 | 19.1 |
| callers run_one | 45 | 4116 | 378 | 45+0 | 5651 | 17.5 |
| callers changed | 42 | 4783 | 374 | 33+18 | 7404 | 17.9 |
| callers asyncify | 32 | 4586 | 343 | 32+0 | 4654 | 34.3 |
| callers filled | 23 | 2719 | 350 | 28+11 | 5602 | 19.8 |
| callers remaining | 24 | 2930 | 357 | 28+4 | 4981 | 16.0 |
| callers enable_all | 64 | 14951 | 331 | 26+48 | 9578 | 16.2 |
| callers sleep_until | 39 | 5150 | 335 | 23+17 | 6752 | 15.3 |
| callers push_front | 24 | 3171 | 300 | 22+2 | 3683 | 15.9 |
| callers put_slice | 74 | 7756 | 537 | 22+52 | 10820 | 26.7 |
| callers worker_threads | 56 | 13681 | 478 | 22+43 | 8923 | 18.9 |
| callers socketpair | 22 | 2739 | 416 | 21+1 | 3205 | 16.6 |
| callers current | 46 | 6214 | 403 | 20+54 | 11735 | 17.7 |
| callers poll_reserve | 20 | 2669 | 427 | 20+0 | 3485 | 17.1 |
| callers try_acquire_owned | 17 | 2222 | 489 | 20+0 | 3208 | 16.9 |

**中央値**: ast-grep 425ms / 4586B vs kenning 18.9ms / 7708B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact sleep | 75 | 366545 | 327 | 8548 | 43x |
| impact registration | 197 | 1876681 | 1168 | 15111 | 124x |
| impact data | 29 | 43839 | 60 | 8705 | 5x |
| impact insert_at | 40 | 136135 | 124 | 5900 | 23x |
| impact unbounded_channel | 104 | 1611858 | 1045 | 14323 | 113x |

中央値: **43x**、tool 呼び出し 327 回 → 1 回

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

中央値: grep 1752 B / 2 回 → def 238 B / 1 回 = **6.9x**

**faceted** (`kind:method vis:pub test:0`): 1300 件 433.625µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 33 | 319769 | 2701 | 30 | 374607 | = |
| unwrap | 2686 | 20 | 318940 | 2686 | 29 | 367798 | = |
| Result | 2839 | 20 | 380851 | 2839 | 33 | 435412 | = |
| runtime | 2482 | 19 | 327851 | 2482 | 27 | 370248 | = |
| stream | 3200 | 16 | 415027 | 3200 | 25 | 476035 | = |
| github | 1571 | 17 | 189393 | 1571 | 19 | 248234 | = |
| assert | 6693 | 17 | 805288 | 6693 | 38 | 932314 | = |
| future | 2545 | 20 | 328368 | 2545 | 31 | 368578 | = |
| return | 3257 | 19 | 443672 | 3257 | 32 | 507918 | = |
| handle | 2879 | 20 | 378121 | 2879 | 29 | 427659 | = |
| thread | 2868 | 17 | 380972 | 2868 | 34 | 428817 | = |
| Context | 1584 | 21 | 213427 | 1584 | 31 | 240731 | = |
| feature | 1227 | 23 | 148931 | 1227 | 24 | 161248 | = |
| struct | 1414 | 21 | 166367 | 1414 | 30 | 190203 | = |
| channel | 1280 | 18 | 165719 | 1280 | 26 | 194170 | = |
| method | 1186 | 17 | 173192 | 1186 | 29 | 197761 | = |
| socket | 1847 | 19 | 237089 | 1847 | 22 | 274055 | = |
| buffer | 1225 | 18 | 166630 | 1225 | 21 | 191065 | = |
| unsafe | 1067 | 17 | 136843 | 1067 | 24 | 151971 | = |
| function | 1070 | 18 | 153227 | 1070 | 25 | 178654 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 19.0ms vs text 28.9ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 973.333µs
index: 770 files / 7156 symbols / 38218 call-sites
  kind=fn                                              =    2300 件  [666ns]
  pub fn                                               =      89 件  [4.708µs]
  pub async fn 非test                                   =      38 件  [5.042µs]
  def new                                              =     271 件  [250ns]
  callers new (名前一致)                                   =    2445 件  [750ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```


## corpus: ripgrep

corpus `~/.cache/kenning-bench/ripgrep` — 207 files / 18055 call-sites (うち repo 内 10508) / repo 内確定率 91.5% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| args | 189 | 170 | 18 | 1 |
| printer_contents | 104 | 101 | 0 | 3 |
| exact | 78 | 66 | 8 | 4 |
| get | 64 | 20 | 36 | 8 |
| multi_line | 58 | 48 | 3 | 7 |
| name_short | 56 | 14 | 0 | 42 |
| after_context | 47 | 44 | 1 | 2 |
| doc_variable | 43 | 6 | 0 | 37 |
| inexact | 35 | 29 | 1 | 5 |
| escape | 34 | 21 | 2 | 11 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 3 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers arg | 97701 | 11 | 8983 | 1 | 10.9x |
| callers parse_low_raw | 75670 | 3 | 7995 | 1 | 9.5x |
| callers create | 57957 | 10 | 7230 | 1 | 8.0x |
| callers is_ignore | 33306 | 10 | 8308 | 1 | 4.0x |
| callers create_dir | 12611 | 4 | 5869 | 1 | 2.1x |
| callers prev | 11173 | 2 | 8078 | 1 | 1.4x |
| callers unwrap_switch | 12063 | 3 | 7410 | 1 | 1.6x |
| callers exact | 11725 | 2 | 8279 | 1 | 1.4x |
| callers assert_err | 14363 | 7 | 6546 | 1 | 2.2x |
| callers expected_no_line_number | 9655 | 3 | 7083 | 1 | 1.4x |
| callers test | 8679 | 3 | 6007 | 1 | 1.4x |
| callers create_bytes | 11719 | 6 | 6199 | 1 | 1.9x |
| callers add_child | 8302 | 3 | 6312 | 1 | 1.3x |
| callers assert_paths | 5105 | 2 | 4399 | 1 | 1.2x |
| callers unwrap_value | 6281 | 3 | 4873 | 1 | 1.3x |
| callers inexact | 6270 | 2 | 4572 | 1 | 1.4x |
| callers as_byte | 16289 | 10 | 4570 | 1 | 3.6x |
| callers text | 4050 | 2 | 3521 | 1 | 1.2x |
| callers seq | 4753 | 2 | 3770 | 1 | 1.3x |
| callers bstr | 4612 | 2 | 3641 | 1 | 1.3x |

**中央値**: 圧縮比 **1.4x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 11.5ms vs kenning 9.5ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers arg | 8 | 710 | 211 | 670+12 | 8983 | 9.9 |
| callers parse_low_raw | 545 | 77997 | 179 | 545+0 | 7995 | 10.3 |
| callers create | 9 | 979 | 175 | 440+7 | 7230 | 9.3 |
| callers is_ignore | 4 | 441 | 161 | 134+3 | 8308 | 8.8 |
| callers create_dir | 1 | 90 | 146 | 99+1 | 5869 | 9.1 |
| callers prev | 0 | 0 | 191 | 72+0 | 8078 | 8.6 |
| callers unwrap_switch | 39 | 4612 | 186 | 69+0 | 7410 | 10.5 |
| callers exact | 9 | 1277 | 196 | 66+8 | 8279 | 9.7 |
| callers assert_err | 0 | 0 | 170 | 58+2 | 6546 | 9.9 |
| callers expected_no_line_number | 55 | 26623 | 154 | 55+0 | 7083 | 9.4 |
| callers test | 55 | 35090 | 201 | 55+0 | 6007 | 9.8 |
| callers create_bytes | 1 | 113 | 195 | 39+8 | 6199 | 10.7 |
| callers add_child | 38 | 6645 | 167 | 38+0 | 6312 | 9.8 |
| callers assert_paths | 33 | 12988 | 172 | 33+0 | 4399 | 9.5 |
| callers unwrap_value | 32 | 4087 | 176 | 32+0 | 4873 | 8.9 |
| callers inexact | 2 | 1063 | 150 | 29+1 | 4572 | 8.9 |
| callers as_byte | 27 | 3717 | 197 | 27+0 | 4570 | 21.3 |
| callers text | 0 | 0 | 171 | 27+0 | 3521 | 8.4 |
| callers seq | 0 | 0 | 184 | 25+0 | 3770 | 9.1 |
| callers bstr | 0 | 0 | 162 | 23+0 | 3641 | 9.0 |

**中央値**: ast-grep 176ms / 1063B vs kenning 9.5ms / 6312B — 構造一致としては同数を拾うが、
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

**faceted** (`kind:method vis:pub test:0`): 477 件 248.458µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 11 | 856322 | 3298 | 13 | 746164 | = |
| ignore | 3475 | 10 | 566374 | 3473 | 15 | 585001 | ≠ |
| OpenSubtitles2016 | 2349 | 9 | 637772 | 2349 | 11 | 547110 | = |
| ripgrep | 1886 | 10 | 244360 | 1885 | 12 | 262354 | ≠ |
| benchsuite | 1896 | 9 | 507042 | 1896 | 9 | 440490 | = |
| Sherlock | 2133 | 9 | 421572 | 2139 | 11 | 383183 | ≠ |
| Holmes | 1679 | 9 | 396369 | 1688 | 12 | 349402 | ≠ |
| sample | 1580 | 10 | 406148 | 1580 | 10 | 343953 | = |
| Холмс | 1406 | 10 | 379069 | 1406 | 10 | 339776 | = |
| Шерлок | 1206 | 9 | 330816 | 1206 | 10 | 295258 | = |
| assert_eq | 1296 | 10 | 158656 | 1296 | 11 | 177179 | = |
| unwrap | 1306 | 8 | 165821 | 1306 | 11 | 185555 | = |
| LC_ALL | 1103 | 9 | 281795 | 1103 | 8 | 244529 | = |
| PM_RESUME | 969 | 9 | 175046 | 969 | 8 | 173294 | = |
| matcher | 1200 | 10 | 151245 | 1194 | 11 | 171829 | ≠ |
| github | 954 | 10 | 121749 | 886 | 10 | 116086 | ≠ |
| BurntSushi | 806 | 10 | 102017 | 806 | 9 | 103321 | = |
| matches | 972 | 9 | 123456 | 972 | 11 | 139052 | = |
| return | 1307 | 10 | 164960 | 1308 | 12 | 188021 | ≠ |
| pattern | 1002 | 9 | 143447 | 1002 | 11 | 155061 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.6ms vs text 11.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 585.083µs
index: 207 files / 3192 symbols / 18055 call-sites
  kind=fn                                              =     705 件  [208ns]
  pub fn                                               =      30 件  [1.5µs]
  pub async fn 非test                                   =       0 件  [125ns]
  def new                                              =      84 件  [166ns]
  callers new (名前一致)                                   =    1102 件  [375ns]
  callers new (確実 逆引き)                                 =       3 件  [83ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 295 files / 44140 call-sites (うち repo 内 20296) / repo 内確定率 80.1% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| new | 2155 | 629 | 1424 | 102 |
| insert | 329 | 208 | 91 | 30 |
| load | 307 | 15 | 281 | 11 |
| himo_id | 243 | 220 | 0 | 23 |
| define_himo_in | 194 | 181 | 0 | 13 |
| make_engine | 127 | 0 | 111 | 16 |
| max | 126 | 20 | 85 | 21 |
| unsigned | 54 | 52 | 1 | 1 |
| get_by_id | 44 | 40 | 0 | 4 |
| slice | 40 | 35 | 0 | 5 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 6 行 → 確実 3 + 候補 0 = 検討対象 4 行、ノイズ率 31%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers tie | 140495 | 67 | 6638 | 1 | 21.2x |
| callers define_himo | 160026 | 79 | 7569 | 1 | 21.1x |
| callers entity_in | 156951 | 79 | 8675 | 1 | 18.1x |
| callers create_standalone | 106037 | 52 | 7843 | 1 | 13.5x |
| callers define_himo_in | 161746 | 84 | 9490 | 1 | 17.0x |
| callers eid_local | 70423 | 30 | 7939 | 1 | 8.9x |
| callers define_table | 154109 | 83 | 8570 | 1 | 18.0x |
| callers flush_writes | 110881 | 64 | 7126 | 1 | 15.6x |
| callers number | 63553 | 29 | 7752 | 1 | 8.2x |
| callers oplog_sync | 108003 | 62 | 7873 | 1 | 13.7x |
| callers tie_to | 82714 | 46 | 8298 | 1 | 10.0x |
| callers pull_once | 57695 | 28 | 8508 | 1 | 6.8x |
| callers oplog_commit | 93794 | 56 | 7483 | 1 | 12.5x |
| callers open_concurrent_with_oplog | 79958 | 41 | 10030 | 1 | 8.0x |
| callers open_standalone | 73729 | 41 | 8809 | 1 | 8.4x |
| callers publish_since | 51797 | 25 | 8558 | 1 | 6.1x |
| callers tie_text | 66964 | 36 | 7929 | 1 | 8.4x |
| callers make_eid | 67840 | 34 | 8468 | 1 | 8.0x |
| callers transfer_oplog_to_sync_ops | 70834 | 38 | 7976 | 1 | 8.9x |
| callers remove_db | 73537 | 44 | 7669 | 1 | 9.6x |

**中央値**: 圧縮比 **10.0x**、grep 経路の tool 呼び出し 46 回 → 1 回

**単発 wall-clock 中央値**: rg 14.0ms vs kenning 17.6ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 382 | 38098 | 347 | 382+3 | 6638 | 11.4 |
| callers define_himo | 350 | 40571 | 322 | 350+0 | 7569 | 12.7 |
| callers entity_in | 230 | 28501 | 316 | 231+0 | 8675 | 12.5 |
| callers create_standalone | 218 | 27457 | 325 | 218+0 | 7843 | 13.5 |
| callers define_himo_in | 181 | 27272 | 332 | 181+0 | 9490 | 13.4 |
| callers eid_local | 142 | 20070 | 343 | 178+1 | 7939 | 13.0 |
| callers define_table | 177 | 21829 | 378 | 177+0 | 8570 | 13.6 |
| callers flush_writes | 166 | 16131 | 723 | 166+0 | 7126 | 21.3 |
| callers number | 128 | 22251 | 1001 | 138+0 | 7752 | 22.1 |
| callers oplog_sync | 128 | 13343 | 986 | 128+0 | 7873 | 26.1 |
| callers tie_to | 127 | 14971 | 752 | 127+0 | 8298 | 22.9 |
| callers pull_once | 108 | 11305 | 722 | 122+0 | 8508 | 20.6 |
| callers oplog_commit | 117 | 11398 | 484 | 117+0 | 7483 | 22.6 |
| callers open_concurrent_with_oplog | 114 | 16909 | 440 | 114+0 | 10030 | 17.8 |
| callers open_standalone | 105 | 13714 | 535 | 105+1 | 8809 | 18.9 |
| callers publish_since | 95 | 11458 | 425 | 101+0 | 8558 | 16.0 |
| callers tie_text | 100 | 12269 | 488 | 100+1 | 7929 | 16.8 |
| callers make_eid | 77 | 10302 | 385 | 89+0 | 8468 | 18.9 |
| callers transfer_oplog_to_sync_ops | 84 | 9717 | 526 | 84+0 | 7976 | 17.1 |
| callers remove_db | 83 | 9772 | 549 | 83+0 | 7669 | 17.6 |

**中央値**: ast-grep 484ms / 16131B vs kenning 17.6ms / 7976B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 354 | 1230497 | 998 | 22060 | 56x |
| impact define_himo | 370 | 1348777 | 1063 | 20655 | 65x |
| impact entity_in | 588 | 3394338 | 2218 | 40494 | 84x |
| impact create_standalone | 424 | 1963634 | 1386 | 19357 | 101x |
| impact define_himo_in | 312 | 1827164 | 1135 | 21338 | 86x |

中央値: **84x**、tool 呼び出し 1135 回 → 1 回

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
| engine.rs | 826598 | 11610 | 71x |
| CHANGELOG.md | 368129 | 6496 | 57x |
| lib.rs | 133158 | 10031 | 13x |
| oplog.rs | 106684 | 7519 | 14x |
| sync.rs | 105249 | 10332 | 10x |

中央値: **14x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.7x**

**faceted** (`kind:method vis:pub test:0`): 1049 件 323.667µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 4641 | 51 | 588311 | 4641 | 40 | 708537 | = |
| assert_eq | 2122 | 18 | 264621 | 2122 | 38 | 329341 | = |
| Engine | 2793 | 24 | 387712 | 2787 | 40 | 472375 | ≠ |
| format | 1355 | 18 | 179180 | 1355 | 28 | 204033 | = |
| assert | 3574 | 60 | 448725 | 3568 | 45 | 564122 | ≠ |
| entity | 2233 | 31 | 303670 | 2233 | 42 | 372073 | = |
| return | 1039 | 34 | 124530 | 1039 | 24 | 140702 | = |
| cleanup | 876 | 29 | 82901 | 876 | 24 | 111948 | = |
| ValueType | 846 | 17 | 106174 | 846 | 19 | 123558 | = |
| enchudb | 2239 | 15 | 288666 | 2199 | 24 | 329404 | ≠ |
| author | 1160 | 17 | 170149 | 1160 | 22 | 215453 | = |
| engine | 2793 | 16 | 387712 | 2787 | 29 | 472375 | ≠ |
| enchudb_oplog | 793 | 15 | 105879 | 793 | 18 | 121694 | = |
| himo_id | 749 | 13 | 103792 | 749 | 17 | 119952 | = |
| record | 1411 | 13 | 205089 | 1411 | 19 | 253253 | = |
| Ordering | 676 | 15 | 89131 | 676 | 17 | 100478 | = |
| schema | 936 | 16 | 133715 | 923 | 20 | 159314 | ≠ |
| remove_file | 620 | 15 | 76292 | 620 | 19 | 87526 | = |
| String | 948 | 14 | 119279 | 936 | 21 | 134739 | ≠ |
| transport | 743 | 12 | 101927 | 740 | 16 | 127679 | ≠ |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 7 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 16.8ms vs text 24.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 982.375µs
index: 295 files / 4092 symbols / 44140 call-sites
  kind=fn                                              =    2077 件  [625ns]
  pub fn                                               =      74 件  [4.417µs]
  pub async fn 非test                                   =       0 件  [333ns]
  def new                                              =      50 件  [250ns]
  callers new (名前一致)                                   =    2053 件  [708ns]
  callers new (確実 逆引き)                                 =      10 件  [166ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 31 files / 6247 call-sites (うち repo 内 990) / repo 内確定率 81.5% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| query | 93 | 76 | 14 | 3 |
| num | 59 | 9 | 49 | 1 |
| tmp | 32 | 29 | 2 | 1 |
| drop | 25 | 8 | 14 | 3 |
| line_of | 14 | 12 | 1 | 1 |
| median_f | 14 | 13 | 0 | 1 |
| col_of | 12 | 11 | 0 | 1 |
| fmt_sym | 12 | 11 | 0 | 1 |
| hit | 12 | 10 | 0 | 2 |
| acquire | 10 | 8 | 0 | 2 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 3 行 → 確実 1 + 候補 0 = 検討対象 2 行、ノイズ率 33%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers query | 13654 | 3 | 10167 | 1 | 1.3x |
| callers tmp | 3286 | 2 | 3925 | 1 | 0.8x |
| callers write_fixture | 3631 | 2 | 3996 | 1 | 0.9x |
| callers indexed | 3072 | 2 | 2471 | 1 | 1.2x |
| callers open_ro | 6492 | 4 | 2343 | 1 | 2.8x |
| callers file_paths | 5112 | 3 | 1968 | 1 | 2.6x |
| callers parse_opts | 7154 | 4 | 1881 | 1 | 3.8x |
| callers median_f | 4897 | 3 | 1665 | 1 | 2.9x |
| callers median_u | 5027 | 3 | 1688 | 1 | 3.0x |
| callers index | 29251 | 11 | 3623 | 1 | 8.1x |
| callers kenning | 17419 | 8 | 1876 | 1 | 9.3x |
| callers line_of | 5636 | 3 | 2224 | 1 | 2.5x |
| callers col_of | 3343 | 2 | 1863 | 1 | 1.8x |
| callers fmt_sym | 4702 | 3 | 1512 | 1 | 3.1x |
| callers tmp_tree | 2699 | 2 | 1614 | 1 | 1.7x |
| callers txt | 19407 | 8 | 8057 | 1 | 2.4x |
| callers end_line_of | 5151 | 3 | 1820 | 1 | 2.8x |
| callers hit | 5776 | 3 | 1523 | 1 | 3.8x |
| callers defs_of | 7013 | 4 | 1269 | 1 | 5.5x |
| callers indexed_fixture | 3119 | 2 | 1653 | 1 | 1.9x |

**中央値**: 圧縮比 **2.8x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 9.4ms vs kenning 7.3ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers query | 85 | 8763 | 84 | 76+14 | 10167 | 7.7 |
| callers tmp | 31 | 2022 | 75 | 29+2 | 3925 | 6.9 |
| callers write_fixture | 29 | 13982 | 77 | 25+3 | 3996 | 7.0 |
| callers indexed | 18 | 1466 | 81 | 18+0 | 2471 | 7.5 |
| callers open_ro | 18 | 1867 | 95 | 18+0 | 2343 | 10.0 |
| callers file_paths | 17 | 1510 | 77 | 17+0 | 1968 | 6.8 |
| callers parse_opts | 16 | 1446 | 93 | 16+0 | 1881 | 7.2 |
| callers median_f | 0 | 0 | 81 | 13+0 | 1665 | 9.0 |
| callers median_u | 0 | 0 | 84 | 13+0 | 1688 | 7.6 |
| callers index | 27 | 1794 | 119 | 12+15 | 3623 | 12.1 |
| callers kenning | 12 | 1028 | 106 | 12+0 | 1876 | 6.8 |
| callers line_of | 12 | 1700 | 97 | 12+1 | 2224 | 8.3 |
| callers col_of | 11 | 1583 | 83 | 11+0 | 1863 | 8.6 |
| callers fmt_sym | 1 | 101 | 97 | 11+0 | 1512 | 7.2 |
| callers tmp_tree | 11 | 961 | 95 | 11+0 | 1614 | 7.3 |
| callers txt | 52 | 6074 | 82 | 11+44 | 8057 | 8.2 |
| callers end_line_of | 7 | 1175 | 74 | 10+0 | 1820 | 6.5 |
| callers hit | 3 | 352 | 78 | 10+0 | 1523 | 6.9 |
| callers defs_of | 9 | 932 | 83 | 9+0 | 1269 | 6.9 |
| callers indexed_fixture | 9 | 929 | 77 | 9+0 | 1653 | 7.1 |

**中央値**: ast-grep 83ms / 1466B vs kenning 7.3ms / 1881B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact query | 44 | 102369 | 89 | 5390 | 19x |
| impact tmp | 47 | 99243 | 94 | 5736 | 17x |
| impact write_fixture | 44 | 92875 | 88 | 5407 | 17x |
| impact indexed | 19 | 41168 | 38 | 2309 | 18x |
| impact open_ro | 32 | 113172 | 85 | 2907 | 39x |

中央値: **18x**、tool 呼び出し 88 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| query.rs | 81915 | 11126 | 7x |
| core.rs | 58574 | 11493 | 5x |
| parse.rs | 51062 | 10848 | 5x |
| tests.rs | 47023 | 9903 | 5x |
| graph.rs | 37262 | 6498 | 6x |

中央値: **5x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1728 B / 2 回 → def 152 B / 1 回 = **11.1x**

**faceted** (`kind:method vis:pub test:0`): 5 件 160µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 447 | 8 | 52959 | 447 | 8 | 63404 | = |
| kenning | 419 | 8 | 57923 | 418 | 8 | 71872 | ≠ |
| String | 426 | 8 | 52745 | 426 | 8 | 59498 | = |
| callers | 335 | 11 | 41526 | 335 | 8 | 64322 | = |
| contains | 210 | 9 | 28629 | 210 | 7 | 36810 | = |
| assert | 391 | 9 | 52183 | 391 | 8 | 69805 | = |
| println | 333 | 9 | 46346 | 333 | 7 | 48900 | = |
| return | 170 | 8 | 15624 | 170 | 7 | 17502 | = |
| container | 175 | 9 | 23424 | 175 | 7 | 26397 | = |
| collect | 144 | 9 | 17128 | 144 | 7 | 19016 | = |
| enchudb | 136 | 10 | 17864 | 120 | 14 | 21149 | ≠ |
| eprintln | 124 | 9 | 18145 | 124 | 10 | 18877 | = |
| assert_eq | 119 | 12 | 15482 | 119 | 9 | 20460 | = |
| format | 111 | 15 | 15879 | 111 | 8 | 17526 | = |
| method | 143 | 11 | 20268 | 143 | 18 | 24170 | = |
| to_string | 164 | 8 | 20878 | 164 | 7 | 23939 | = |
| is_empty | 98 | 9 | 10347 | 98 | 6 | 11518 | = |
| update | 136 | 24 | 19714 | 136 | 13 | 23509 | = |
| symbol | 155 | 8 | 21196 | 155 | 8 | 25484 | = |
| entity | 175 | 10 | 20725 | 175 | 7 | 22978 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 9.0ms vs text 8.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 881.333µs
index: 31 files / 395 symbols / 6247 call-sites
  kind=fn                                              =     297 件  [166ns]
  pub fn                                               =      25 件  [416ns]
  pub async fn 非test                                   =       0 件  [208ns]
  def new                                              =       2 件  [208ns]
  callers new (名前一致)                                   =     173 件  [250ns]
  callers new (確実 逆引き)                                 =       1 件  [166ns]
```

