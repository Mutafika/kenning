# vs CodeQL — 「code as data」本家との頭対頭 (corpus: enchudb)

CodeQL の Rust extractor は rust-analyzer ベース = 構図 (解析を facts に焼いて別層で引く) は
本品と同じ。違いは規模と目的: CodeQL はセキュリティ解析向けの汎用リレーショナル QL、
本品は agent のナビゲーション専用に薄く速く。(CodeQL 2.26.1 / rust-all 0.2.17)

| 段階 | CodeQL | kenning (syn 層) |
|---|---|---|
| facts 構築 wall | **4073s (68 分)** | 2.4s |
| 構築 peak RSS | 9930 MB | 174 MB |
| facts ディスク | 301M | 67M |
| 「who calls flush_writes?」初回 | 1418s (QL コンパイル込み) | 101 ms (CLI 起動込み) |
| 同、2 回目 (cache 済) | 47.5s / 1764 MB | 101 ms (毎回) |

## 回答の突き合わせ (相互検証)

同じ問いに CodeQL は 57 行、kenning は確実 117 行。内訳を突き合わせると:

- **lib コード (crates/enchudb-engine): 44 = 44 で完全一致** — RA ベースの解決同士、
  数字が合う。本品の確実 edge の正確さの独立検証がまた 1 本
- 差分はほぼ全部 **tests/**: CodeQL の extractor は integration test の caller 60+ 件を
  ほぼ落とした (57 行中 tests/ は 1 行のみ)。examples/ と benches/ は両者拾えている

## 注記 (公平のため)

- QL は本品に書けない任意リレーショナル質問 (taint tracking、データフロー等) が書ける。
  役割が違う — 「セキュリティ解析の副産物として navigation もできる」のが CodeQL、
  navigation 専用に構築 2.4s・クエリ 0.1s に削ったのが本品
- 本品の bake (精密モード) の構築コストは VS-RA.md の RA 列 (~40s/6GB) — CodeQL 構築と
  同系統だが 100x 軽い (extractor の目的が違うので当然ではある)
- CodeQL の Rust 対応はまだ若い (rust-all 0.2.x)。tests/ の欠落は将来直る可能性がある

再現: `codeql` CLI を PATH に置いて `KENNING_CODEQL=<path> ./bench/vs-codeql.sh`
(db が既にあれば構築は skip される)。
