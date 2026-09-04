# kenning — Claude 向け操作ガイド

これは Rust コードの **semantic navigation CLI**。顧客は「コード探索する Claude 自身」。
grep+Read の代わりに、精密な少数行 (`path:line<TAB>詳細` = そのまま Read に渡せる) を返す。

## ルール (1 行)

**Rust repo 内の検索は kenning。** シンボル軸の問い (定義 / 呼び元 / 呼び先 / 実装 / 影響範囲 /
faceted) は `kenning <cmd>`、全文検索は `kenning text` — **`.rs` も `.md`/`.toml`/`.yml` も同じ 1 本**で、
文脈注釈が付く分 grep の上位互換。db 管理は考えなくていい (自動)。
grep に落ちるのは対象外だけ: binary / 1MiB 超 / gitignore 済み / 生成 lock ファイル。正規表現は `text -e`、
dir 絞りは `text … path:<dir>`、同名 symbol は `read` の `crate:` / `path:` / `--all`、行の周辺は `read <path>:<line>`、
md の見出し配下は `read <file>#<見出し>`、crate の地図は `outline <dir>`。stderr は要点 1 行だけなので `2>/dev/null` は不要。

## 使い方 (儀式ゼロ: cd して聞くだけ)

```bash
cd <rust-repo>                     # あとは聞くだけ。db は ~/.cache/kenning/ に自動作成・
kenning callers <name>          # 変更があれば自動増分 update (進捗は stderr、stdout はデータのみ)
```

```bash
kenning def     <name>              # 定義位置 + シグネチャ + doc 1 行目 (hover 相当)
kenning read    <name> [container] [crate:X] [path:S] [--all]  # 定義本体 (def + Read の 1 手化。まずこれ)。同名は絞るか --all
kenning read    <path>:<line>       # その行を囲む item の本体 (grep -n → sed の代わり)。非 Rust は見出し配下
kenning read    <file>#<見出し>      # md の見出し / toml の [table] / yaml のキー配下 (CHANGELOG を awk で切る代わり)
kenning find    <substr>            # symbol 名 + ファイル名 (basename) の部分一致 (発見用。`find -name` 相当も兼ねる)
kenning text    <term>... [-e] [path:S]  # 全文検索 + 文脈注釈 (.rs=関数 / .md=見出し階層 / .toml=[table])。複数語は OR、
                                    #   -e で正規表現 ((?-i) で大小区別)、path: で dir 絞り。末尾に `# N 件 / M files`
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
kenning outline <path|dir>          # ファイル構造 (Read せず)。`.` で repo の地図。.md/.toml/.yml は見出し構造 = read <file>#… の目次。
                                    #   dir なら配下 file の地図 (symbol 数 / loc)。`read <path>` も同じ (file 全体は Read)
kenning stats                       # 規模と名前解決率
kenning cache [ls|prune] [--older-than D] [--dry-run]  # 自動 db の棚卸し / 掃除 (repo 消失・旧版を回収)
```

索引対象は **rg と同じ規約** (.gitignore / .ignore / 隠し dir を尊重、`target/` と `node_modules/` は常に除外)。
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
syn 層の索引は repo 全体のままなので、workspace 外は精度控えめで動き続ける。常駐なしのバッチで、実測は
kenning (3 rs) 10s / 1.0GB、enchudb (256 rs、12 crate) 初回 3 分 (依存の build script 込み) → 2 回目 26s / 2.3GB。
features=all → default の順に試す。all は optional dep の build script (bundled C++ / binary DL) で数分かかり、
RA が固まる事故もあるので上限 15 分 (`KENNING_BAKE_TIMEOUT=<秒>`) で group ごと止めて default に退避、
以後その repo は marker (`<db>.bake-default`) で default から焼く。最初から default なら `KENNING_BAKE_DEFAULT_FEATURES=1`。
空きメモリゲート + 直列 lock 付き — 刺さる状況では焚かない。bake しなくても syn 層で全 navigation は
動く (精度控えめ・嘘なし)。bake 後 20 ファイル変更で stderr に再 bake 推奨が出る。

## 出力の読み方 (Claude 向け)

- 各行 `path:line<TAB>詳細` = **そのまま Read に渡せる**。stdout はデータのみ (装飾なし・決定的順序)。
- stderr は自動 index / update の要点 1 行 (初回 2 行) と `⚠` 警告だけ。捨てずに読む (古い結果の警告が載る)。
- `#` 行 = 件数と次の一手 (「確実 N + 候補 M」「絞る: callers X <container>」など)。
- `callers` は **確実 (callee_sym 逆引き、誤りなし) + 候補 (未確定=要 Read で確認) + 別 sym に確定** の3分割。
  「全 caller を掴んだか」はこの3つの合計で判断でき、grep に戻らなくていい。
- 名前が無いと **近い名前を自動提案** (typo 救済)。

## 効いてくる正直な限界

- **鮮度は自動 (query 時に古ければ増分 update してから回答、ms オーダー)。** 判定は既知 dir / file の stat のみ
  (walk は dir に増減があった時だけ)、編集直後の query は編集した file だけ読む。lock が取れない時だけ
  古い結果+stderr 警告に落ちる。full 再 index (heal / 初回) は db ごとの flock で直列化 — 並列に叩いても
  待って成果を再利用するだけで、壊れも二重焼きもしない (作りかけは `<db>.tmp-<pid>` に焼いて rename で差し替え)。
- **精度は食わせた SCIP の feature 網羅に依存 (GIGO)。** 確定 facts は rust-analyzer のもの。
- **hover / 補完 / 診断 / 式の型推論は無い** (人間のエディタ用機能。Claude は Read + `cargo check` で足りる)。
- index は派生物 → **VCS に混ぜない** (gitignore、local に持つ)。
