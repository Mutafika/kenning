# kenning — Rust repo のコード探索はこれ (grep の代わり)

`cd` して聞くだけ (index は自動作成・自動更新)。出力の各行は `path:line<TAB>詳細` = そのまま Read に渡せる。
出力の修飾名 (`Engine::open`) はそのまま次のコマンドの引数に渡せる。

```
kenning callers <name> [container] [path:S]  # 誰が呼ぶか: 確実 (誤りなし) + 候補 (要確認) + 別の同名に確定
kenning impact  <name>              # 推移的な呼び元 = 変えると壊れる範囲
kenning tests   <name>              # 届くテスト (変更後に回す物)
kenning read    <name> | <path>:<line>  # 定義本体 / その行を囲む item
kenning def     <name>              # 位置 + シグネチャ + doc 1 行
kenning callees <name> / path <from> <to> / impls <trait|type>
kenning text    <term>... [-e] [--and] [path:<dir>]  # 全文検索 (.rs / .md / .toml)、囲む関数・見出し付き
kenning outline <path|dir>          # file / crate の構造 (読まずに)
kenning search  reachable:0         # どこからも届かない定義 (消せる候補)
kenning changes --since HEAD        # 未 commit の作業の意味的な差分 (壊れた呼び出し / sig 変更 / dead)
```

- 同名は container か `path:` / `crate:` で絞る。`#` 行 = 件数と次の一手。stderr の 1 行 (古い結果の ⚠ 等) も読む。
- 候補の `[method-name]` = 受け手の型が分からない呼び。確定させないだけで、全 caller は確実 + 候補で揃う。
- リファクタの途中は `changes --since HEAD` で確認し、`cargo check` はターンの最後に 1 回。
- 精度を上げるのは `kenning bake` (rust-analyzer 1 回、常駐なし)。一度焼いた repo は裏で自動で焼き直す
  (止めるなら `KENNING_AUTO_BAKE=0`)。
- grep に落ちるのは対象外 (binary / 1MiB 超 / gitignore 済み / 生成 lock) だけ。

詳細 (出力の読み方・各コマンドの全 option・bake・限界) は `docs/GUIDE.md` — 必要な節だけ
`kenning read docs/GUIDE.md#<見出し>` で読む (目次は `kenning outline docs/GUIDE.md`)。
