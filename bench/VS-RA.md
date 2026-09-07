# vs rust-analyzer — cold から正確に答えられるまで

RA 側は本家ベンチ `analysis-stats` (全 workspace 解析+型推論 = 正確な find-refs の前提知識)。
kenning 側は `index` (syn 層)。**精密モード (bake) の構築コストは RA 列と同じもの** —
それを常駐でなく一発のバッチとして払い、以後の全クエリを index から µs-ms で返すのが本品の設計。

| corpus | 対象 | 構築 wall | peak RSS | 構築後のクエリ |
|---|---|---|---|---|
| enchudb | rust-analyzer (resident 相当) | 16.30s | 3179 MB | LSP 常駐が続く限り ms |
| enchudb | kenning (syn 層) | 0.30s | 136 MB | 35 ms (CLI 起動込み)、常駐 0 |
| tokio | rust-analyzer (resident 相当) | 17.04s | 2477 MB | LSP 常駐が続く限り ms |
| tokio | kenning (syn 層) | 0.34s | 156 MB | 48 ms (CLI 起動込み)、常駐 0 |

公平のための注記: ①kenning (syn 層) は RA より解決精度が低い (型推論なし。callers は
確実∪候補のラベル付きで返す) — 精密が要る時の bake コスト ≈ RA 列を一発だけ払う。
②RA は各 corpus の default features 分しか解析しない (tokio の default は最小構成なので
RA 列が軽く見える — features=all なら更に重い)。③analysis-stats は全域推論の一括実行で、
実際の LSP は必要箇所から lazy に解析する (体感の初回応答はこれより早いが、知識の総コストは同じ)。

できることの差 (どちらが強い、でなく役割が違う):

| 能力 | rust-analyzer | kenning |
|---|---|---|
| hover 型推論 / 補完 / 診断 | ✅ | ❌ (agent は cargo check で足りる) |
| 正確 find-refs / who-calls | ✅ (常駐前提) | ✅ bake 後 (= RA の facts を位置 join) |
| faceted AND (kind×vis×crate×…) | ❌ | ✅ µs |
| 推移的 impact / call path | ❌ (1 hop ずつ) | ✅ 1 クエリ |
| 非活性 cfg 側の解析 | ❌ | ✅ (syn が cfg-blind に全ブランチ) |
| repo 横断 (across) | ❌ (単一 workspace) | ✅ (SCIP symbol join) |
| 常駐メモリ | GB 級 | 0 (index はファイル) |

## 精度: RA と「どれだけ同じことを言うか」 (bake 後)

kenning の**確定**は RA の facts そのもの (SCIP occurrence を位置 join したもの) なので、
「RA と精度が違うか」ではなく **「RA が答えた所をどれだけ拾えているか」+「RA が黙る所をどうするか」**
が実際の比較軸。`kenning index --scip` の内訳 (2026-09-06 実測、features=all / enchudb は default):

| corpus | repo 内 call-site | 確定 | うち RA(SCIP) 由来 | syn 回収 (RA 沈黙域) | 未確定 |
|---|---|---|---|---|---|
| tokio | 20,390 | 12,073 (59.2%) | 11,235 (93.1%) | 838 | 8,317 |
| ripgrep | 10,508 | 9,611 (91.5%) | 9,589 (99.8%) | 22 | 897 |
| enchudb | 20,296 | 16,265 (80.1%) | 16,230 (99.8%) | 35 | 4,031 |

分母は **repo 内呼び出し** (std / 依存 crate への呼び出しは index に定義が無く解決不能なので除く)。

- **確定は RA と同じか、それより多い。** RA に無い分 (syn 回収) は RA が沈黙した領域を
  保守的な syn resolver が拾ったもの。RA の答えを上書きすることはない。
- **未確定 = RA も occurrence を出していない所。** tokio の源は SCIP occurrence が無い 9,134 箇所で、
  内訳は「document 自体が SCIP に無い」4,095 + 「document はあるが該当位置に occurrence 無し」5,039。
  **RA なら "no references" で終わる所**を、kenning は `[method-name]` / `[value-ref]` /
  `[macro-token]` の候補として位置付きで見せる。
- **位置 join の取りこぼしではない。** 「同じ行に別の occurrence はある」のが 995 箇所あるが、
  中身を見ると `assert_eq!(b.next()…)` の行で RA が出しているのは `assert_eq` (col 4) だけで、
  `.next` (col 17) には何も無い — RA が path / マクロ名は解決し、その行の method 呼びは
  解決していない形。列のズレで取り落としているのではない。
- **tokio が低いのは repo 側の事情。** `--cfg tokio_unstable` を渡して焼き直すと
  確定 11,235 → 12,332、**59.2% → 65.0%**。RUSTFLAGS 依存の cfg は Cargo.toml に現れないので
  自動では当てられない → `KENNING_BAKE_RUSTFLAGS='--cfg tokio_unstable' kenning bake` で渡す。
  target 種別で切っても src 60.2% / tests 63.5% / benches 74.0% / examples 66.0% と差は小さく、
  「test だけ解析されていない」といった単純な話ではない (`kenning stats path:<dir>` で確認できる)。
- **未 bake (syn 層のみ) は 15〜24%** と大きく落ちる。受け手の型が分からない method 呼びを
  確定させないため — 嘘はつかないが、精度が要るなら bake は必須。
