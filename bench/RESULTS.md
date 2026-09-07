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

**単発 wall-clock 中央値**: rg 15.6ms vs kenning 10.2ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers sleep | 152 | 19443 | 208 | 106+89 | 15354 | 14.9 |
| callers registration | 89 | 15839 | 220 | 79+19 | 10059 | 10.2 |
| callers data | 0 | 0 | 204 | 63+1 | 7708 | 9.9 |
| callers insert_at | 63 | 8442 | 230 | 63+0 | 8262 | 9.5 |
| callers unbounded_channel | 30 | 3663 | 204 | 52+14 | 10946 | 12.3 |
| callers task | 21 | 3548 | 239 | 51+18 | 9573 | 12.1 |
| callers run_one | 45 | 4116 | 217 | 45+0 | 5651 | 10.1 |
| callers changed | 42 | 4783 | 237 | 33+18 | 7404 | 10.3 |
| callers asyncify | 32 | 4586 | 198 | 32+0 | 4654 | 10.0 |
| callers filled | 23 | 2719 | 210 | 28+11 | 5602 | 10.1 |
| callers remaining | 24 | 2930 | 200 | 28+4 | 4981 | 10.2 |
| callers enable_all | 64 | 14951 | 240 | 26+48 | 9578 | 10.8 |
| callers sleep_until | 39 | 5150 | 214 | 23+17 | 6752 | 9.8 |
| callers push_front | 24 | 3171 | 209 | 22+2 | 3683 | 10.1 |
| callers put_slice | 74 | 7756 | 216 | 22+52 | 10820 | 10.2 |
| callers worker_threads | 56 | 13681 | 241 | 22+43 | 8923 | 31.5 |
| callers socketpair | 22 | 2739 | 217 | 21+1 | 3205 | 9.7 |
| callers current | 46 | 6214 | 237 | 20+54 | 11735 | 11.0 |
| callers poll_reserve | 20 | 2669 | 214 | 20+0 | 3485 | 9.5 |
| callers try_acquire_owned | 17 | 2222 | 200 | 20+0 | 3208 | 9.6 |

**中央値**: ast-grep 216ms / 4586B vs kenning 10.2ms / 7708B — 構造一致としては同数を拾うが、
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

**faceted** (`kind:method vis:pub test:0`): 1300 件 309.375µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| assert_eq | 2701 | 26 | 319769 | 2701 | 23 | 374607 | = |
| unwrap | 2686 | 16 | 318940 | 2686 | 23 | 367798 | = |
| Result | 2839 | 16 | 380851 | 2839 | 25 | 435412 | = |
| runtime | 2482 | 16 | 327851 | 2482 | 22 | 370248 | = |
| stream | 3200 | 15 | 415027 | 3200 | 22 | 476035 | = |
| github | 1571 | 15 | 189393 | 1571 | 17 | 248234 | = |
| assert | 6693 | 15 | 805288 | 6693 | 31 | 932314 | = |
| future | 2545 | 15 | 328368 | 2545 | 25 | 368578 | = |
| return | 3257 | 16 | 443672 | 3257 | 26 | 507918 | = |
| handle | 2879 | 14 | 378121 | 2879 | 23 | 427659 | = |
| thread | 2868 | 16 | 380972 | 2868 | 23 | 428817 | = |
| Context | 1584 | 16 | 213427 | 1584 | 22 | 240731 | = |
| feature | 1227 | 16 | 148931 | 1227 | 20 | 161248 | = |
| struct | 1414 | 15 | 166367 | 1414 | 24 | 190203 | = |
| channel | 1280 | 16 | 165719 | 1280 | 17 | 194170 | = |
| method | 1186 | 15 | 173192 | 1186 | 20 | 197761 | = |
| socket | 1847 | 16 | 237089 | 1847 | 19 | 274055 | = |
| buffer | 1225 | 30 | 166630 | 1225 | 19 | 191065 | = |
| unsafe | 1067 | 16 | 136843 | 1067 | 18 | 151971 | = |
| function | 1070 | 16 | 153227 | 1070 | 21 | 178654 | = |

**一致**: 20/20 問でヒット数が同一。**wall 中央値**: rg 15.7ms vs text 22.0ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 750.083µs
index: 770 files / 7156 symbols / 38218 call-sites
  kind=fn                                              =    2300 件  [583ns]
  pub fn                                               =      89 件  [3.5µs]
  pub async fn 非test                                   =      38 件  [3.916µs]
  def new                                              =     271 件  [166ns]
  callers new (名前一致)                                   =    2445 件  [583ns]
  callers new (確実 逆引き)                                 =       1 件  [83ns]
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

**単発 wall-clock 中央値**: rg 10.3ms vs kenning 7.7ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers arg | 8 | 710 | 136 | 670+12 | 8983 | 10.7 |
| callers parse_low_raw | 545 | 77997 | 123 | 545+0 | 7995 | 7.8 |
| callers create | 9 | 979 | 158 | 440+7 | 7230 | 8.6 |
| callers is_ignore | 4 | 441 | 130 | 134+3 | 8308 | 7.4 |
| callers create_dir | 1 | 90 | 126 | 99+1 | 5869 | 7.7 |
| callers prev | 0 | 0 | 148 | 72+0 | 8078 | 7.2 |
| callers unwrap_switch | 39 | 4612 | 147 | 69+0 | 7410 | 7.5 |
| callers exact | 9 | 1277 | 143 | 66+8 | 8279 | 7.7 |
| callers assert_err | 0 | 0 | 138 | 58+2 | 6546 | 8.5 |
| callers expected_no_line_number | 55 | 26623 | 144 | 55+0 | 7083 | 7.2 |
| callers test | 55 | 35090 | 136 | 55+0 | 6007 | 7.5 |
| callers create_bytes | 1 | 113 | 120 | 39+8 | 6199 | 8.0 |
| callers add_child | 38 | 6645 | 132 | 38+0 | 6312 | 8.2 |
| callers assert_paths | 33 | 12988 | 122 | 33+0 | 4399 | 7.2 |
| callers unwrap_value | 32 | 4087 | 142 | 32+0 | 4873 | 8.4 |
| callers inexact | 2 | 1063 | 134 | 29+1 | 4572 | 7.6 |
| callers as_byte | 27 | 3717 | 122 | 27+0 | 4570 | 8.0 |
| callers text | 0 | 0 | 151 | 27+0 | 3521 | 7.4 |
| callers seq | 0 | 0 | 160 | 25+0 | 3770 | 7.3 |
| callers bstr | 0 | 0 | 138 | 23+0 | 3641 | 7.9 |

**中央値**: ast-grep 138ms / 1063B vs kenning 7.7ms / 6312B — 構造一致としては同数を拾うが、
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

**faceted** (`kind:method vis:pub test:0`): 477 件 186.042µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| subtitles | 3298 | 9 | 856322 | 3298 | 12 | 746164 | = |
| ignore | 3475 | 9 | 566374 | 3473 | 14 | 585001 | ≠ |
| OpenSubtitles2016 | 2349 | 10 | 637772 | 2349 | 10 | 547110 | = |
| ripgrep | 1886 | 10 | 244360 | 1885 | 11 | 262354 | ≠ |
| benchsuite | 1896 | 8 | 507042 | 1896 | 9 | 440490 | = |
| Sherlock | 2133 | 10 | 421572 | 2139 | 11 | 383183 | ≠ |
| Holmes | 1679 | 8 | 396369 | 1688 | 10 | 349402 | ≠ |
| sample | 1580 | 9 | 406148 | 1580 | 9 | 343953 | = |
| Холмс | 1406 | 8 | 379069 | 1406 | 9 | 339776 | = |
| Шерлок | 1206 | 8 | 330816 | 1206 | 8 | 295258 | = |
| assert_eq | 1296 | 9 | 158656 | 1296 | 10 | 177179 | = |
| unwrap | 1306 | 8 | 165821 | 1306 | 10 | 185555 | = |
| LC_ALL | 1103 | 9 | 281795 | 1103 | 8 | 244529 | = |
| PM_RESUME | 969 | 9 | 175046 | 969 | 8 | 173294 | = |
| matcher | 1200 | 9 | 151245 | 1194 | 10 | 171829 | ≠ |
| github | 954 | 9 | 121749 | 886 | 9 | 116086 | ≠ |
| BurntSushi | 806 | 9 | 102017 | 806 | 8 | 103321 | = |
| matches | 972 | 10 | 123456 | 972 | 11 | 139052 | = |
| return | 1307 | 9 | 164960 | 1308 | 11 | 188021 | ≠ |
| pattern | 1002 | 10 | 143447 | 1002 | 11 | 155061 | = |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 4 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。 多い 3 問 = NUL を含む file (rg は binary 判定で打ち切り、kenning は最後まで読む)。**wall 中央値**: rg 9.3ms vs text 10.2ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 742.917µs
index: 207 files / 3192 symbols / 18055 call-sites
  kind=fn                                              =     705 件  [208ns]
  pub fn                                               =      30 件  [1.417µs]
  pub async fn 非test                                   =       0 件  [125ns]
  def new                                              =      84 件  [166ns]
  callers new (名前一致)                                   =    1102 件  [375ns]
  callers new (確実 逆引き)                                 =       3 件  [125ns]
```


## corpus: enchudb

corpus `~/myapp/enchudb` — 297 files / 44382 call-sites (うち repo 内 20406) / repo 内確定率 80.2% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| get_table | 153 | 131 | 0 | 22 |
| hlc | 139 | 97 | 35 | 7 |
| tie_to | 128 | 127 | 0 | 1 |
| query | 121 | 100 | 1 | 20 |
| entity_count | 75 | 72 | 0 | 3 |
| author_note | 53 | 0 | 46 | 7 |
| tie_ref | 52 | 50 | 0 | 2 |
| pending_sync_ops | 51 | 44 | 0 | 7 |
| from_bytes | 50 | 41 | 3 | 6 |
| create_growable_with_capacity | 48 | 44 | 0 | 4 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 5 行 → 確実 3 + 候補 0 = 検討対象 4 行、ノイズ率 33%

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
| callers open_standalone | 74911 | 42 | 8809 | 1 | 8.5x |
| callers publish_since | 51797 | 25 | 8558 | 1 | 6.1x |
| callers tie_text | 66964 | 36 | 7929 | 1 | 8.4x |
| callers make_eid | 67840 | 34 | 8468 | 1 | 8.0x |
| callers remove_db | 76997 | 46 | 7929 | 1 | 9.7x |
| callers transfer_oplog_to_sync_ops | 70834 | 38 | 7976 | 1 | 8.9x |

**中央値**: 圧縮比 **10.0x**、grep 経路の tool 呼び出し 46 回 → 1 回

**単発 wall-clock 中央値**: rg 10.6ms vs kenning 11.7ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers tie | 382 | 38098 | 330 | 382+3 | 6638 | 12.6 |
| callers define_himo | 350 | 40571 | 328 | 350+0 | 7569 | 13.0 |
| callers entity_in | 230 | 28501 | 286 | 231+0 | 8675 | 10.6 |
| callers create_standalone | 218 | 27457 | 286 | 218+0 | 7843 | 10.4 |
| callers define_himo_in | 181 | 27272 | 301 | 181+0 | 9490 | 12.2 |
| callers eid_local | 142 | 20070 | 313 | 178+1 | 7939 | 11.7 |
| callers define_table | 177 | 21829 | 308 | 177+0 | 8570 | 11.4 |
| callers flush_writes | 166 | 16131 | 309 | 166+0 | 7126 | 12.2 |
| callers number | 128 | 22251 | 322 | 138+0 | 7752 | 9.0 |
| callers oplog_sync | 128 | 13343 | 325 | 128+0 | 7873 | 13.8 |
| callers tie_to | 127 | 14971 | 288 | 127+0 | 8298 | 11.8 |
| callers pull_once | 108 | 11305 | 332 | 122+0 | 8508 | 12.6 |
| callers oplog_commit | 117 | 11398 | 328 | 117+0 | 7483 | 11.1 |
| callers open_concurrent_with_oplog | 114 | 16909 | 291 | 114+0 | 10030 | 12.7 |
| callers open_standalone | 106 | 13866 | 284 | 106+1 | 8809 | 10.6 |
| callers publish_since | 95 | 11458 | 303 | 101+0 | 8558 | 11.3 |
| callers tie_text | 100 | 12269 | 279 | 100+1 | 7929 | 11.1 |
| callers make_eid | 77 | 10302 | 295 | 89+0 | 8468 | 11.7 |
| callers remove_db | 89 | 10597 | 303 | 89+0 | 7929 | 9.3 |
| callers transfer_oplog_to_sync_ops | 84 | 9717 | 266 | 84+0 | 7976 | 11.2 |

**中央値**: ast-grep 303ms / 16131B vs kenning 11.7ms / 7976B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact tie | 354 | 1241910 | 1005 | 22060 | 56x |
| impact define_himo | 370 | 1362998 | 1071 | 20655 | 66x |
| impact entity_in | 593 | 3423834 | 2238 | 40638 | 84x |
| impact create_standalone | 424 | 1978579 | 1395 | 19357 | 102x |
| impact define_himo_in | 318 | 1848957 | 1151 | 21785 | 85x |

中央値: **84x**、tool 呼び出し 1151 回 → 1 回

**impls** (trait→実装型、impl 数上位):

| question | impls | grep bytes | grep calls | 圧縮比 |
|---|---|---|---|---|
| impls Drop | 17 | 24301 | 16 | 17.4x |
| impls Sync | 17 | 29036 | 17 | 20.4x |
| impls Send | 16 | 29130 | 17 | 21.6x |
| impls Default | 12 | 16571 | 13 | 17.0x |
| impls From | 10 | 8057 | 6 | 10.4x |

中央値: **17.4x**

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| engine.rs | 824858 | 11751 | 70x |
| CHANGELOG.md | 371975 | 6491 | 57x |
| lib.rs | 133260 | 10031 | 13x |
| oplog.rs | 106684 | 7519 | 14x |
| sync.rs | 105249 | 10332 | 10x |

中央値: **14x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 2340 B / 2 回 → def 270 B / 1 回 = **8.7x**

**faceted** (`kind:method vis:pub test:0`): 1049 件 221.709µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| unwrap | 4688 | 10 | 595074 | 4688 | 20 | 716422 | = |
| assert_eq | 2131 | 10 | 266035 | 2131 | 15 | 331087 | = |
| Engine | 2814 | 10 | 390748 | 2808 | 16 | 476847 | ≠ |
| format | 1365 | 9 | 180505 | 1365 | 15 | 205444 | = |
| assert | 3594 | 11 | 451775 | 3588 | 18 | 567915 | ≠ |
| entity | 2236 | 9 | 304082 | 2236 | 16 | 372759 | = |
| return | 1041 | 10 | 124715 | 1041 | 13 | 140918 | = |
| cleanup | 876 | 10 | 82901 | 876 | 12 | 111948 | = |
| enchudb | 2251 | 9 | 290266 | 2211 | 16 | 331209 | ≠ |
| ValueType | 846 | 10 | 106174 | 846 | 13 | 123558 | = |
| author | 1160 | 9 | 170149 | 1160 | 12 | 215453 | = |
| engine | 2814 | 11 | 390748 | 2808 | 18 | 476847 | ≠ |
| enchudb_oplog | 793 | 9 | 105879 | 793 | 13 | 121694 | = |
| himo_id | 749 | 10 | 103792 | 749 | 12 | 119952 | = |
| record | 1411 | 10 | 205089 | 1411 | 13 | 253253 | = |
| Ordering | 676 | 9 | 89131 | 676 | 11 | 100478 | = |
| schema | 966 | 9 | 138270 | 953 | 12 | 165650 | ≠ |
| remove_file | 620 | 9 | 76292 | 620 | 13 | 87526 | = |
| String | 952 | 10 | 119812 | 940 | 14 | 135312 | ≠ |
| transport | 743 | 10 | 101927 | 740 | 11 | 127679 | ≠ |

**一致**: 13/20 問でヒット数が同一。差の内訳: 少ない 7 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 10.0ms vs text 13.4ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 562.458µs
index: 297 files / 4110 symbols / 44382 call-sites
  kind=fn                                              =    2093 件  [541ns]
  pub fn                                               =      75 件  [2.916µs]
  pub async fn 非test                                   =       0 件  [208ns]
  def new                                              =      50 件  [166ns]
  callers new (名前一致)                                   =    2053 件  [541ns]
  callers new (確実 逆引き)                                 =      10 件  [125ns]
```


## corpus: kenning

corpus `~/myapp/kenning` — 31 files / 6299 call-sites (うち repo 内 1235) / repo 内確定率 86.9% / **baked (SCIP)**

### quality — grep 相当ヒット vs 精密 callers (n=100, seed=42)

grep 相当 = `\bNAME\s*\(` の全ヒット (def/コメント/文字列/別型の同名も混ざる)。
確実 = callee_sym 逆引き (誤りなし) / 候補 = 未解決の名前一致 (要確認)。

| symbol | grep hits | 確実 | 候補 | grep との差 (≈ノイズ) |
|---|---|---|---|---|
| new | 186 | 5 | 169 | 12 |
| num | 60 | 59 | 0 | 1 |
| txt | 56 | 55 | 0 | 1 |
| next | 53 | 5 | 37 | 11 |
| write_fixture | 30 | 29 | 0 | 1 |
| ref_of | 27 | 26 | 0 | 1 |
| open_ro | 19 | 18 | 0 | 1 |
| parse_opts | 17 | 16 | 0 | 1 |
| median_u | 14 | 13 | 0 | 1 |
| fmt_sym | 12 | 11 | 0 | 1 |
| … | (上位 10 件のみ表示) | | | |

**中央値**: grep 4 行 → 確実 2 + 候補 0 = 検討対象 2 行、ノイズ率 25%

### agent — 「誰が呼ぶ?」20 問の tool 出力バイト比較

質問 = 定義が一意な被呼上位シンボル (grep 側も `\bname\(` で正確に狙える公平条件)。
grep 経路 = rg 出力 + ヒット各ファイル 40 行 Read (楽観モデル=下限)。
kenning 経路 = `callers <name>` の実出力 (別プロセス実行の実測)。

| question | grep bytes | grep calls | cs bytes | cs calls | 圧縮比 |
|---|---|---|---|---|---|
| callers query | 13654 | 3 | 7782 | 1 | 1.8x |
| callers num | 20326 | 8 | 7228 | 1 | 2.8x |
| callers txt | 19430 | 8 | 6773 | 1 | 2.9x |
| callers tmp | 3286 | 2 | 3799 | 1 | 0.9x |
| callers write_fixture | 3631 | 2 | 3973 | 1 | 0.9x |
| callers index | 29533 | 11 | 3339 | 1 | 8.8x |
| callers ref_of | 8735 | 4 | 4052 | 1 | 2.2x |
| callers file_paths | 5252 | 3 | 2115 | 1 | 2.5x |
| callers indexed | 3072 | 2 | 2471 | 1 | 1.2x |
| callers open_ro | 6492 | 4 | 2343 | 1 | 2.8x |
| callers parse_opts | 7154 | 4 | 1881 | 1 | 3.8x |
| callers line_of | 5636 | 3 | 2110 | 1 | 2.7x |
| callers median_f | 4897 | 3 | 1665 | 1 | 2.9x |
| callers median_u | 5027 | 3 | 1688 | 1 | 3.0x |
| callers kenning | 17316 | 8 | 1761 | 1 | 9.8x |
| callers col_of | 3343 | 2 | 1863 | 1 | 1.8x |
| callers fmt_sym | 4702 | 3 | 1512 | 1 | 3.1x |
| callers tmp_tree | 2699 | 2 | 1614 | 1 | 1.7x |
| callers end_line_of | 5151 | 3 | 1820 | 1 | 2.8x |
| callers hit | 5776 | 3 | 1523 | 1 | 3.8x |

**中央値**: 圧縮比 **2.8x**、grep 経路の tool 呼び出し 3 回 → 1 回

**単発 wall-clock 中央値**: rg 7.5ms vs kenning 4.9ms (両者プロセス起動込み。
kenning は鮮度チェック省略時 = デフォルトでは +stat-walk ~10ms。速さは互角 — 差は出力の精密さと bytes)

### agent — vs ast-grep (構造検索アプリ、同じ質問)

ast-grep は tree-sitter の構造一致: def/コメント/文字列のノイズ **0** (grep より 1 段精密)。
ただし呼び出し 3 形 (`name()` / `$R.name()` / `$P::name()`) の列挙をユーザーが背負い、
**名前解決は無い** — `$R.name()` は全ての型の同名 method に一致し、どの定義の caller かは
答えられない (= kenning の「候補」相当の粒度)。walk 型なので repo サイズに比例して遅い。

| question | ast-grep 一致 | bytes | ms (3 パターン計) | cs 確実+候補 | cs bytes | cs ms |
|---|---|---|---|---|---|---|
| callers query | 85 | 8763 | 55 | 90+0 | 7782 | 4.8 |
| callers num | 52 | 5492 | 57 | 59+0 | 7228 | 5.9 |
| callers txt | 52 | 6074 | 53 | 55+0 | 6773 | 5.4 |
| callers tmp | 31 | 2022 | 50 | 31+0 | 3799 | 4.9 |
| callers write_fixture | 29 | 13982 | 49 | 29+0 | 3973 | 4.8 |
| callers index | 27 | 1794 | 54 | 27+0 | 3339 | 5.0 |
| callers ref_of | 26 | 3574 | 59 | 26+0 | 4052 | 5.1 |
| callers file_paths | 18 | 1648 | 53 | 18+0 | 2115 | 5.3 |
| callers indexed | 18 | 1466 | 57 | 18+0 | 2471 | 4.9 |
| callers open_ro | 18 | 1867 | 56 | 18+0 | 2343 | 4.7 |
| callers parse_opts | 16 | 1446 | 55 | 16+0 | 1881 | 4.7 |
| callers line_of | 12 | 1700 | 52 | 13+0 | 2110 | 4.9 |
| callers median_f | 0 | 0 | 49 | 13+0 | 1665 | 5.3 |
| callers median_u | 0 | 0 | 55 | 13+0 | 1688 | 4.7 |
| callers kenning | 12 | 1028 | 54 | 12+0 | 1761 | 4.7 |
| callers col_of | 11 | 1583 | 48 | 11+0 | 1863 | 4.5 |
| callers fmt_sym | 1 | 101 | 56 | 11+0 | 1512 | 4.5 |
| callers tmp_tree | 11 | 961 | 54 | 11+0 | 1614 | 5.1 |
| callers end_line_of | 7 | 1175 | 53 | 10+0 | 1820 | 5.0 |
| callers hit | 3 | 352 | 53 | 10+0 | 1523 | 4.5 |

**中央値**: ast-grep 54ms / 1648B vs kenning 4.9ms / 2115B — 構造一致としては同数を拾うが、
「どの定義か」の確定・impact/path/faceted は ast-grep には無い

### beyond-search — graph/構造クエリ (grep 経路モデル vs 実出力)

grep 経路は agent と同じ楽観モデル (= 下限)。impact の grep 経路 = 訪問シンボルごとに
grep+Read を繰り返す手動 BFS (実際のエージェントの再帰探索を模す)。

**impact** (推移的 callers、上位 5 問):

| question | 影響 syms | grep bytes | grep calls | cs bytes | 圧縮比 |
|---|---|---|---|---|---|
| impact query | 44 | 102369 | 89 | 5390 | 19x |
| impact num | 83 | 323923 | 224 | 7889 | 41x |
| impact txt | 87 | 346459 | 238 | 8225 | 42x |
| impact tmp | 48 | 101571 | 96 | 5848 | 17x |
| impact write_fixture | 48 | 101916 | 96 | 5858 | 17x |

中央値: **19x**、tool 呼び出し 96 回 → 1 回

**outline** (構造把握、最大 5 ファイル — 代替は Read 全文):

| file | Read bytes | outline bytes | 圧縮比 |
|---|---|---|---|
| query.rs | 82431 | 11126 | 7x |
| core.rs | 58574 | 11493 | 5x |
| parse.rs | 51062 | 10848 | 5x |
| tests.rs | 47761 | 10194 | 5x |
| graph.rs | 37262 | 6498 | 6x |

中央値: **5x**

**def** (定義+sig+doc、被呼上位 10 問 — 代替は `rg "fn NAME"` + 前後 Read):

中央値: grep 1688 B / 2 回 → def 152 B / 1 回 = **10.6x**

**faceted** (`kind:method vis:pub test:0`): 6 件 88.75µs — grep では表現不能 (比較なし、能力差)

### text — 全文検索 vs rg (同じ語、同じ repo)

問い = 「この語はどこ?」。rg 経路 = `rg -i -n <term>` (kenning text は大小無視なので -i)。
kenning 経路 = `text <term> --limit 100000` の実出力。**ヒット数の一致**が主指標 —
バイトは kenning が増える (行ごとに関数名 / 見出し階層を付けるため)。それが payload。

| term | rg hits | rg ms | rg bytes | text hits | text ms | text bytes | 一致 |
|---|---|---|---|---|---|---|---|
| kenning | 430 | 7 | 59888 | 429 | 5 | 74605 | ≠ |
| unwrap | 449 | 7 | 53184 | 449 | 5 | 63658 | = |
| String | 432 | 6 | 53397 | 432 | 5 | 60230 | = |
| callers | 336 | 7 | 41639 | 336 | 5 | 64441 | = |
| contains | 212 | 7 | 28916 | 212 | 5 | 37153 | = |
| assert | 397 | 8 | 52928 | 397 | 7 | 70859 | = |
| println | 335 | 7 | 46620 | 335 | 5 | 49187 | = |
| return | 170 | 7 | 15623 | 170 | 4 | 17501 | = |
| container | 175 | 7 | 23424 | 175 | 5 | 26397 | = |
| collect | 144 | 7 | 17128 | 144 | 5 | 19016 | = |
| enchudb | 144 | 7 | 19042 | 123 | 5 | 21896 | ≠ |
| assert_eq | 124 | 6 | 16075 | 124 | 5 | 21317 | = |
| eprintln | 125 | 7 | 18289 | 125 | 5 | 19027 | = |
| format | 115 | 7 | 16412 | 115 | 5 | 18097 | = |
| method | 150 | 7 | 21443 | 150 | 5 | 25779 | = |
| is_empty | 103 | 9 | 10840 | 103 | 5 | 12056 | = |
| to_string | 165 | 6 | 20984 | 165 | 4 | 24052 | = |
| update | 136 | 5 | 19714 | 136 | 4 | 23509 | = |
| call_t | 103 | 10 | 12041 | 103 | 5 | 13283 | = |
| symbol | 156 | 7 | 21355 | 156 | 5 | 25636 | = |

**一致**: 18/20 問でヒット数が同一。差の内訳: 少ない 2 問 = 生成 lock (`Cargo.lock`) / >1MiB / binary の非索引分。**wall 中央値**: rg 7.0ms vs text 4.9ms
(両者プロセス起動込み。text は `#` の件数行と文脈注釈を含んだ上でこの wall)

### micro — warm latency
```
open(readonly): 565.667µs
index: 31 files / 400 symbols / 6299 call-sites
  kind=fn                                              =     300 件  [166ns]
  pub fn                                               =      25 件  [291ns]
  pub async fn 非test                                   =       0 件  [125ns]
  def new                                              =       2 件  [125ns]
  callers new (名前一致)                                   =     174 件  [208ns]
  callers new (確実 逆引き)                                 =       1 件  [125ns]
```

