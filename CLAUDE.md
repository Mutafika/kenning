# kenning — Claude 向け操作ガイド

これは Rust コードの **semantic navigation CLI**。顧客は「コード探索する Claude 自身」。
grep+Read の代わりに、精密な少数行 (`path:line<TAB>詳細` = そのまま Read に渡せる) を返す。

## ルール (1 行)

**Rust repo 内の検索は kenning。** シンボル軸の問い (定義 / 呼び元 / 呼び先 / 実装 / 影響範囲 /
faceted) は `kenning <cmd>`、全文検索は `kenning text` — **`.rs` も `.md`/`.toml`/`.yml` も同じ 1 本**で、
文脈注釈が付く分 grep の上位互換。db 管理は考えなくていい (自動)。
grep に落ちるのは対象外だけ: binary / 1MiB 超 / gitignore 済み / 生成 lock ファイル、あとは正規表現。

## 使い方 (儀式ゼロ: cd して聞くだけ)

```bash
cd <rust-repo>                     # あとは聞くだけ。db は ~/.cache/kenning/ に自動作成・
kenning callers <name>          # 変更があれば自動増分 update (進捗は stderr、stdout はデータのみ)
```

```bash
kenning def     <name>              # 定義位置 + シグネチャ + doc 1 行目 (hover 相当)
kenning read    <name> [container]  # 定義本体をそのまま出す (def + Read の 1 手化。まずこれ)
kenning find    <substr>            # 名前の部分一致 (発見用)
kenning text    <term>              # 全文検索 + 文脈注釈 (.rs=関数 / .md=見出し階層 / .toml=[table])
kenning callers <name> [container]  # who-calls: 確実 ∪ 未確定候補を位置付き
kenning callees <name> [container]  # X が呼ぶ先 (outgoing)
kenning edges                       # 全 cross-file call edge の集計 TSV (from TAB to TAB count)。依存グラフの素材
kenning refs    <name> [container]  # find-all-refs (要 --scip index、型/読み書きも)
kenning impls   <trait|type>        # go-to-implementation (trait↔型)
kenning across  <name>              # 全 repo 横断: 全 repo の定義/利用 + repo 跨ぎ精密参照
kenning impact  <name> [container]  # 変えると壊れる推移的 callers
kenning tests   <name> [container]  # これに届くテスト = impact ∩ is_test (変更後に何を回すか)
kenning path    <from> <to>         # from→to の呼び出し経路
kenning search  kind:method vis:pub container:Engine calls:unwrap  # faceted AND
kenning outline <path>              # ファイル構造 (Read せず)
kenning stats                       # 規模と名前解決率
kenning cache [ls|prune] [--older-than D] [--dry-run]  # 自動 db の棚卸し / 掃除 (repo 消失・旧版を回収)
```

索引対象は **rg と同じ規約** (.gitignore / .ignore / 隠し dir を尊重、`target/` は常に除外)。
gitignore 済みだが実際に compile される生成 `.rs` を持つ repo だけ `KENNING_NO_IGNORE=1`。

手動制御が要る時だけ: `--db <path>` / env `KENNING_DB` (明示 db は自動 index しない)、
`KENNING_NO_AUTO=1` (魔法全停止)、`KENNING_NO_STALE=1` (鮮度チェックのみ停止)。
binary は `~/.cargo/bin/kenning` (cargo install --path . 済み)。

## 精度を上げる (bake = 一発)

```bash
kenning bake        # repo 内で。RA scip (features=all 注入) → 精密 index まで自動
```
who-calls/refs が **rust-analyzer と同じ正確さ**になる。焼くのは **cwd を含む cargo workspace**
(repo root 全体ではない。RA は 1 project しか読めないため。曖昧なら焼かずに選択を促す)。
syn 層の索引は repo 全体のままなので、workspace 外は精度控えめで動き続ける。peak ~5GB × 数十秒のバッチ (常駐なし)。
空きメモリゲート + 直列 lock 付き — 刺さる状況では焚かない。bake しなくても syn 層で全 navigation は
動く (精度控えめ・嘘なし)。bake 後 20 ファイル変更で stderr に再 bake 推奨が出る。

## 出力の読み方 (Claude 向け)

- 各行 `path:line<TAB>詳細` = **そのまま Read に渡せる**。stdout はデータのみ (装飾なし・決定的順序)。
- `#` 行 = 件数と次の一手 (「確実 N + 候補 M」「絞る: callers X <container>」など)。
- `callers` は **確実 (callee_sym 逆引き、誤りなし) + 候補 (未確定=要 Read で確認) + 別 sym に確定** の3分割。
  「全 caller を掴んだか」はこの3つの合計で判断でき、grep に戻らなくていい。
- 名前が無いと **近い名前を自動提案** (typo 救済)。

## 効いてくる正直な限界

- **鮮度は自動 (query 時に古ければ増分 update してから回答、ms オーダー)。** lock が取れない時だけ
  古い結果+stderr 警告に落ちる。full 再 index (heal / 初回) は db ごとの flock で直列化 — 並列に叩いても
  待って成果を再利用するだけで、壊れも二重焼きもしない (作りかけは `<db>.tmp-<pid>` に焼いて rename で差し替え)。
- **精度は食わせた SCIP の feature 網羅に依存 (GIGO)。** 確定 facts は rust-analyzer のもの。
- **hover / 補完 / 診断 / 式の型推論は無い** (人間のエディタ用機能。Claude は Read + `cargo check` で足りる)。
- index は派生物 → **VCS に混ぜない** (gitignore、local に持つ)。
