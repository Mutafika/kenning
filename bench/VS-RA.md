# vs rust-analyzer — cold から正確に答えられるまで

RA 側は本家ベンチ `analysis-stats` (全 workspace 解析+型推論 = 正確な find-refs の前提知識)。
kenning 側は `index` (syn 層)。**精密モード (bake) の構築コストは RA 列と同じもの** —
それを常駐でなく一発のバッチとして払い、以後の全クエリを index から µs-ms で返すのが本品の設計。

| corpus | 対象 | 構築 wall | peak RSS | 構築後のクエリ |
|---|---|---|---|---|
| enchudb | rust-analyzer (resident 相当) | 38.94s | 6058 MB | LSP 常駐が続く限り ms |
| enchudb | kenning (syn 層) | 0.45s | 175 MB | 44 ms (CLI 起動込み)、常駐 0 |
| tokio | rust-analyzer (resident 相当) | 20.67s | 2416 MB | LSP 常駐が続く限り ms |
| tokio | kenning (syn 層) | 0.60s | 232 MB | 37 ms (CLI 起動込み)、常駐 0 |

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
