# vs Glean (Meta) — 同じ .scip を食わせた serving 層の頭対頭

**この対戦だけ完全に同じ弾を使える**: Glean の Rust 取込も rust-analyzer の SCIP なので、
本品が bake した `.scip` (enchudb, 10MB) をそのまま Meta のエンジンに食わせて、
「facts の serving 層」だけを比較した。実行環境は OrbStack (macOS) + 公式 demo image
(`ghcr.io/facebookincubator/glean/demo`, 3.1GB, **amd64 のみ → Rosetta 実行**。
公式 docs は image を「現在動かない」と注記しているが、この計測時点の latest は動いた)。

| 段階 | Glean (demo image, Rosetta) | kenning (native) |
|---|---|---|
| SCIP 取込 wall | 8.0s | 0.52s (+syn parse/graph 込み) |
| 取込 peak RSS | 702 MB | 268 MB |
| facts ディスク | **14M** (scip facts のみ) | 87M (syn call graph / 全列 facet / extref 込み) |
| find-refs 1 問 (one-shot CLI) | ~1.0s / 114 MB | **0.011s** |
| 常駐 | server モードが本来の運用形 | 0 (毎回 CLI) |

Rosetta のエミュレーション係数はせいぜい 2-3x — 取込 15x・クエリ ~90x の差は係数では説明できない。
ただし disk は Glean の勝ち (こちらは SCIP 以外の facts も持っている)。

## 回答の突き合わせ (4 本目の相互検証)

「Engine::flush_writes の参照は?」— **Glean 57 vs kenning 58** (同じ scip スナップショット。
差 1 は definition-role occurrence を refs に数えるかの流儀差)。旧 scip でも 57 = 57。
serving 層が違っても facts が同じなら答えは同じ、という当たり前を確認できたのが収穫。

## beyond 系 (impact / callers / impls / faceted) は Glean では測れない — 能力差

scip.angle の全 predicate を確認した (Definition / Reference / SymbolKind / SymbolName 等 20 個):
**call edge も enclosing-symbol も facet も存在しない**。つまり OSS の Rust 経路 (SCIP 取込) では:

- **find-refs / goto-def**: ✅ (上で実測した通り、正確)
- **outline 相当**: △ (DefinitionLocation を file で引けば近いものは出る)
- **who-calls の caller 帰属 / 推移的 impact / impls / faceted AND**: ❌ 表現不能。
  Angle は再帰クエリを書ける言語だが、土台の call edge fact が無いので再帰する対象が無い
  (Meta 社内では Hack/C++ 等のリッチな専用 indexer がこれを供給する。Rust の OSS 経路には無い)

本品が同じ .scip から impact 52x / impls / faceted を出せるのは、**SCIP と並行して syn 層
(call-site + enclosing + facet) を持っている**から — 「syn × SCIP の結婚」の価値が
この対戦で一番はっきり出た。

## この対戦で見つかった設計差 (勝敗より重要)

- **Glean は SCIP を as-is で serve** する (ソース不要) → 弾が古くても壊れない。
  **本品は live ソースに位置 join** する → SCIP が古いとその分の精密 facts が剥がれる
  (実際この対戦中、bake 後のソース編集で refs が 0 になっているのを発見 → fresh bake で回復。
  bake 鮮度は meta の upd_since_bake で追跡・警告される)
- Glean は多言語 schema 基盤 + Angle クエリ言語 + server/シャーディング — 組織スケールの設計。
  本品は「1 開発者 × N repo × agent」に特化して、儀式ゼロ・常駐ゼロ・ms を取った
- Meta 級インフラの構図 (facts を焼いて別層で serve) は単一バイナリでも成立する、が記事の結論

再現: OrbStack/Docker で `./bench/vs-glean.sh` (image pull ~3GB が別途要る)。
