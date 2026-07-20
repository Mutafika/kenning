# kenning

enchudb を **コード探索エンジン** として使う PoC。Rust ソースを `syn` でパースして
symbol / call の fact を enchudb に積み、**faceted 等値 AND + who-calls を µs** で引く。



## 使い方

**顧客は「コード探索する AI」**。価値は grep 全マッチ + ファイル通読を、精密な少数行に圧縮すること。
出力は `path:line<TAB>詳細` = そのまま Read に渡せる (装飾/計測なし・決定的順序)。

```bash
cargo install --git https://github.com/Mutafika/kenning   # これだけ (enchudb は pin 済み git dep)
# ↑ checkout 済みなら cargo install --path . でも可

# 儀式ゼロ: repo 内で聞くだけ。db は ~/.cache/kenning/ に自動作成・古ければ自動増分 update。
cd <rust-repo> && kenning callers <name>

# 精密化 (rust-analyzer を窯として一発。常駐なし・空きゲート・直列 lock):
kenning bake                                   # RA scip (features=all 注入) → 精密 index

# 手動制御したい時だけ (db は末尾引数 / --db / env KENNING_DB):
./target/release/kenning index  [dir] [db]     # full index (root と時刻を db に焼く=自己記述)
./target/release/kenning update [dir] [db]     # 変更/追加/削除だけ増分 re-index (full と同一結果)

# 探索 (共通 flag: --db <path> / --limit <n>)
./target/release/kenning def <name>            # 定義位置 + シグネチャ (hover 相当)
./target/release/kenning read <name> [container]     # 定義本体をそのまま出す (def + Read の 1 手化)
./target/release/kenning find <substr>         # 名前の部分一致 (発見用、大小無視)
./target/release/kenning text <term>            # 全文検索 + enclosing symbol 注釈 (grep superset)
./target/release/kenning callers <name> [container]  # 精密 who-calls (確実 ∪ 未確定候補を位置付き)
./target/release/kenning callees <name> [container]  # X が呼ぶ先 (outgoing calls、callers の鏡)
./target/release/kenning edges                       # 全 cross-file call edge の集計 TSV (依存グラフの素材)
./target/release/kenning refs <name> [container]     # 正確 find-all-refs (要 --scip、型/読み書きも)
./target/release/kenning impact <name> [container]   # 推移的 callers = 変えると壊れる範囲 (逆 BFS)
./target/release/kenning tests <name> [container]    # これに届くテスト = impact ∩ is_test
./target/release/kenning impls <trait|type>          # go-to-implementation (trait↔型)
./target/release/kenning across <name>              # 全 repo 横断 (repo 跨ぎ精密参照、要 bake)
./target/release/kenning path <from> <to>            # from→to の呼び出し経路 1 本 (前方 BFS)
./target/release/kenning search kind:method vis:pub container:Engine  # faceted 等値 AND
./target/release/kenning outline <path>        # ファイルの symbol 一覧 (Read せず構造把握)
./target/release/kenning stats                 # 規模 + 名前解決率
```

`update` は内容 hash で変わったファイルだけ再 parse し、未変更ファイルからの参照 (incoming edge) も
再解決するので **full 再 index と同一結果**。小ファイル編集で ~18ms（full 再 index ~206ms の ~11x）。

index は **自己記述**（root と build 時刻を db に焼く）。query 時に root を stat-walk して、index 後に
更新されたファイルがあれば **stderr に「index が古い→update しろ」と警告**（stdout の path:line は汚さない）。
編集後に黙って古い結果を返さないための信頼担保。速度重視ループは `KENNING_NO_STALE=1` で無効化。

## 実測 (再現可能スイート — `./bench/corpus.sh && ./bench/run.sh`)

corpus は tag 固定・乱数は固定 seed・手法は表の直上に自己記述。全文は [bench/RESULTS.md](bench/RESULTS.md)。

- **agent** (「誰が呼ぶ?」20 問の tool 出力バイト): 中央値 **2.3x** (ripgrep) / **5.3x** (tokio) /
  **10.2x** (enchudb) 圧縮、呼び出し 3-30 回 → 1 回。grep 経路は楽観モデル (= 下限)。
  優位はシンボルの被呼の広さに比例 — 綺麗に設計された ripgrep が正直な床
- **beyond** (サーチ以外): `impact` (壊れる範囲) = **13-52x**・tool 呼び出し 75-808 回 → 1 回
  (grep 経路 = agent が実際にやる手動 BFS を模す)。`impls` 10-23x / `outline` 7-12x /
  `def` 6-10x。**深い問いほど差が開く** — ripgrep でも who-calls 2.3x に対し impact は 13x
- **quality** (ランダム 100 シンボル): grep `\bname\(` ヒットのノイズ率 中央値 33-43%
  (最悪例: enchudb `len` = 988 ヒット中、確実 caller は 46)
- **micro**: faceted AND / def / 確実 callers = 125ns–4µs (warm)
- **vs rust-analyzer** ([bench/VS-RA.md](bench/VS-RA.md)): cold→who-calls 可能まで、
  RA `analysis-stats` = 39s / 6.1GB (enchudb) vs kenning index = 0.45s / 175MB・常駐 0。
  精度差と features の注記は表の直下に明記
- **vs ast-grep** (agent スイート内、同じ質問): 構造一致は 確実∪候補 とほぼ完全一致
  (`tie` 321=321、`clone` 621=224+397) = **独立実装による検出完全性の相互検証**。
  差は速度 (walk 564-878ms vs index 13-19ms) と、名前解決・impact/faceted の有無
- **vs CodeQL** ([bench/VS-CODEQL.md](bench/VS-CODEQL.md)): 同じ RA ベース facts 構図の本家。
  構築 68 分/9.9GB/301MB vs 2.4s/174MB/67MB、who-calls 1 問 cache 済でも 47.5s vs 0.1s。
  lib コードの回答は 44=44 で完全一致 (3 本目の相互検証)、CodeQL は tests/ をほぼ欠落
- **vs Glean (Meta)** ([bench/VS-GLEAN.md](bench/VS-GLEAN.md)): **同じ .scip を両者に食わせた**
  serving 層だけの純粋対決 (OrbStack/Rosetta)。取込 8.0s/702MB vs 0.52s/268MB、
  find-refs 1 問 ~1.0s vs 0.011s、回答は 57 vs 58 で実質一致 (4 本目の相互検証)。
  disk は Glean 勝ち (14MB vs 87MB)、古い SCIP への耐性も Glean (as-is serve) の設計が上

## 設計の要点

- **index は derived artifact** → VCS の中には混ぜない。local で持ち gitignore。
- **完全 standalone** — 外部 VCS (sf 等) とは統合しない。freshness は自前の増分 `update` で持つ
  （将来は daemon + file-watcher で scan ゼロに）。
- **enchudb backend**: 全列自動 index の faceted 等値 AND が設計点。schema は Kythe/Glean/SCIP/CodeQL 参考。

## 名前解決 (Phase 1 実装済)

2-pass で callee を解決: `Type::f()`/`Self::f()`/`mod::f()` の修飾一致 or 同名ユニークで確定し、
解決先 sym の eid を `call.callee_sym` に持つ。**推測はしない**(曖昧・外部型は未解決のまま)。
→ who-calls が「名前一致(混ざる)」から「特定定義への呼び出しだけ」に絞れる。

実測 (enchudb self-index, 24850 call-sites): 解決率 **26.6%** (unique 5150 / qualified 1466)、
ambiguous 7093 (`x.f()` の型推論待ち) / external 11141 (std/外部 crate)。

### SCIP による正確化 (`index --scip <f>`)

rust-analyzer の SCIP 出力 (`rust-analyzer scip .`) を食い、occurrence を **(rel_path, line, col) で
位置 join** して callee_sym を正確化する (syn=facet+call 検出、SCIP=解決 の結婚)。

- SCIP 列は **UTF-8 バイトオフセット**、syn (proc-macro2) は char 単位なので、非 ASCII 行では
  `Scip::load` で byte→char 変換して join を合わせる。
- **SCIP が沈黙する call-site**(主に `#[cfg(feature=…)]`/`#[cfg(test)]` の非活性コード。syn は
  cfg-blind に全ブランチを見るが rust-analyzer は活性 cfg しか解析しない)は、保守的 syn resolver に
  **フォールバック**して best-effort 回収する。
- 二層: **`refs` は SCIP-pure**(occurrence のみ) / **`callers` は SCIP + syn 回収**(cfg 域も拾う)。

実測 (enchudb, 25796 call-sites, async-blob 無効で SCIP 生成): 解決 **35.4%** = SCIP 確定 7113 +
syn 回収 2030。診断は `KENNING_DIAG_NOOCC=1 kenning index …` で no-occ の内訳(doc欠落/cfg位置ズレ)。

## grep / rust-analyzer との違い (実測)

- **vs grep**: grep はテキスト一致だけ → who-calls は def/コメント/別型の同名 method が混ざる
  (`block_on(` = tokio で 184 生ヒット、うち確実な `Runtime::block_on` 呼びは 82)。`refs Engine`
  も grep 996 vs SCIP 確定 623。さらに **faceted AND / impact / path** は grep 原理的に不可。
- **vs rust-analyzer**: 確定 edge は RA の SCIP を位置 join したもの = **RA と同じ正確さ**。RA は
  6GB 常駐 LSP で 1 問ずつ返すが、kenning は offline に焼いて faceted 合成 + 推移的 graph + µs
  CLI で返す serving 層。精度は食わせた SCIP の feature 網羅に依存(GIGO): 同じ tokio でも
  `default=[]` の SCIP は 177 確定、`--features full` なら 6760 確定。
- **速さ**: 単発は rg と互角(~5ms)。勝ちは grep 不可の問い + 長命プロセスでの反復(warm 1.58µs)。

## 設計の取引 — やらないこと、とその代償

上の数字は全部「何かをやらない」ことで買っている。台帳:

| やらないこと | 買ったもの | 代償 (実測・実感) |
|---|---|---|
| 型推論 (`x.f()` の受け手) | 構築 0.5s / 増分 18ms / cfg 全ブランチ | syn 層 13-26% 止まり。精密は bake (40s/5GB 一発) |
| hover / 補完 / 診断 | 常駐ゼロ、LSP 実装不要 | 人間のエディタにはならない。型は `cargo check` |
| マクロ展開 | per-file parse の速さ | マクロ生成の call/impl はどの層からも見えない |
| 常駐 / watcher | RAM 0、運用ゼロ、SSH 先 OK | 毎クエリ stat-walk 10-20ms が床 |
| SCIP の as-is serve (live ソースに位置 join) | 答えが常に今のコードを指す | bake が古びると精密層が剥がれる (Glean 戦で refs=0 を実地で踏んだ。`upd_since_bake` が警告) |
| 推測 (解決の偽装) | 確実層に誤りゼロ | 「候補」の目視はエージェントに残る |
| 汎用クエリ言語 (Angle/QL) | 学習コスト 0、µs | taint tracking 等は CodeQL の領分のまま |
| 多言語 (今は) | Rust に深く (cfg 回収・trait 帰属) | TS/Python repo では無力 (schema 自体は言語中立) |
| disk 節約 | 全列自動 index + syn 層並走 | 87MB vs Glean 14MB |

**15 コマンドで足りるのか？** 閉集合ではなく「エージェントが実際に聞く質問の語彙」を
dogfood で育てたもの。実運用でナビゲーション質問が聞けず grep に戻った例は今のところ無く、
長尾は grep+Read への fallback で受ける。欠けが見つかった時の追加コストが安いのが本体 —
facts は既に store にあるので、新コマンド = 合成半日 (実例: `tests <name>` = impact ∩ is_test
を既存 facts の組み合わせだけで即日追加)。**資産はコマンド一覧ではなく schema。**
