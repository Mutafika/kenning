# kenning — Claude 向け操作ガイド

これは Rust コードの **semantic navigation CLI**。顧客は「コード探索する Claude 自身」。
grep+Read の代わりに、精密な少数行 (`path:line<TAB>詳細` = そのまま Read に渡せる) を返す。

## ルール (1 行)

**Rust repo 内の検索は kenning。** シンボル軸の問い (定義 / 呼び元 / 呼び先 / 実装 / 影響範囲 /
faceted) は `kenning <cmd>`、全文検索は `kenning text` — **`.rs` も `.md`/`.toml`/`.yml` も同じ 1 本**で、
文脈注釈が付く分 grep の上位互換。db 管理は考えなくていい (自動)。
grep に落ちるのは対象外だけ: binary / 1MiB 超 / gitignore 済み / 生成 lock ファイル。正規表現は `text -e`、
dir 絞りは `text … path:<dir>`、同名 symbol は `read` の `crate:` / `path:` / `--all`、行の周辺は `read <path>:<line>` (範囲は `read <path>:<from>-<to>`)、
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
kenning read    <path>:<from>-<to>  # 行範囲 (sed -n 'A,Bp' の代わり)。範囲が跨ぐ定義 / 見出しを頭に列挙
kenning read    <file>#<見出し>      # md の見出し / toml の [table] / yaml のキー配下 (CHANGELOG を awk で切る代わり)
kenning find    <substr>            # symbol 名 + ファイル名 (basename) の部分一致 (発見用。`find -name` 相当も兼ねる)
kenning text    <term>... [-e] [--and] [--files] [path:S]  # 全文検索 + 文脈注釈 (.rs=関数 / .md=見出し階層 /
                                    #   .toml=[table])。複数語は既定 OR / `--and` で全語 AND (`grep X | grep Y`)、
                                    #   -e で正規表現 ((?-i) で大小区別)、`--files` で file 別件数だけ (`rg -c` = 広い語の
                                    #   triage)、path: で dir 絞り。末尾に `# N 件 / M files`
kenning callers <name> [container]  # who-calls: 確実 ∪ 未確定候補を位置付き
kenning callees <name> [container]  # X が呼ぶ先 (outgoing)
kenning edges                       # 全 cross-file call edge の集計 TSV (from TAB to TAB count)。依存グラフの素材
kenning refs    <name> [container]  # find-all-refs (要 --scip index、型/読み書きも)
kenning impls   <trait|type>        # go-to-implementation (trait↔型)
kenning across  <name>              # 全 repo 横断: 全 repo の定義/利用 + repo 跨ぎ精密参照
kenning impact  <name> [container] [--confirmed-only]  # 変えると壊れる推移的 callers。既定は値渡し参照
                                    #   (map(f)、名前が一意な時) も辿る。確定 edge だけなら --confirmed-only
kenning tests   <name> [container]  # これに届くテスト = impact ∩ is_test (変更後に何を回すか)
kenning path    <from> <to>         # from→to の呼び出し経路
kenning search  kind:method vis:pub container:Engine calls:unwrap path:engine.rs  # faceted AND
kenning search  reachable:0         # **消せる候補はこれ**: live root (pub / #[test] / trait 実装 / main /
                                    #   item 直下マクロ) から到達しない定義。鎖や相互再帰で繋がった dead な塊も
                                    #   1 パスで出る。定義以外に名前が字句として出る物は自動除外 (--no-lexical で切れる)。
                                    #   除外の判定は「候補と dead の**定義本体**の外での、コード上の出現」。
                                    #   コメント / 文字列 / 束縛 (field 宣言・局所変数・`.field` アクセス) は
                                    #   使用に数えない。cfg で分岐した同名定義も候補から外す (索引は cfg-blind
                                    #   なので非活性側に流入が付かないため)
kenning search  attr:deprecated     # 属性の部分一致 (attr:allow(dead_code) / attr:serde / attr:cfg(...))
kenning search  kind:fn callers:0 namecalls:0 test:0  # 1 段だけの版 (入次数 0)。定義以外に
                                    #   名前が字句として出る物は自動で除外 (DSL マクロ等。`--no-lexical` で切れる)。(callers=確実 /
                                    #   namecalls=名前一致。確実だけ 0 なら「未解決の呼び出しかも」)。method は
                                    #   `traitimpl:0` も付ける (trait 実装は trait 経由で呼ばれ、構造的に 0 になる)
kenning outline <path|dir>          # ファイル構造 (Read せず)。`.` で repo の地図。.md/.toml/.yml は見出し構造 = read <file>#… の目次。
                                    #   dir なら配下 file の地図 (symbol 数 / loc)。`read <path>` も同じ (file 全体は Read)
kenning stats [path:<substr>]        # 規模と解決の内訳 (repo 内確定率 + 外部/同名複数/値渡し/マクロ)
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
repo が RUSTFLAGS 前提の custom cfg を要る場合 (tokio の `--cfg tokio_unstable` 等。Cargo.toml に
現れないので自動では当てられない) は `KENNING_BAKE_RUSTFLAGS='--cfg tokio_unstable' kenning bake`
— 実測で tokio の確定率 59.2% → 65.0%。
空きメモリゲート + 直列 lock 付き — 刺さる状況では焚かない。bake しなくても syn 層で全 navigation は
動く (精度控えめ・嘘なし)。bake 後 20 ファイル変更で stderr に再 bake 推奨が出る。

## 出力の読み方 (Claude 向け)

- 各行 `path:line<TAB>詳細` = **そのまま Read に渡せる**。stdout はデータのみ (装飾なし・決定的順序)。
- 出力に出る修飾名 (`Engine::open_readonly` / `IndexLock::acquire`) は **そのまま次のコマンドの引数に渡せる**
  (`def` / `read` / `callers` / `callees` / `refs` / `impact` / `tests` / `path` / `across`)。同名の絞り込みが 1 手で済む。
- stderr は自動 index / update の要点 1 行 (初回 2 行) と `⚠` 警告だけ。捨てずに読む (古い結果の警告が載る)。
- `#` 行 = 件数と次の一手 (「確実 N + 候補 M」「絞る: callers X <container>」など)。
- `in (item 直下)` = 関数の外 (item 直下のマクロ引数、`criterion_group!(benches, bench_x, …)` など) からの参照。
- 候補の `[macro-token]` = parse できない DSL マクロ (proptest! 等) の中で字句として見つけた呼び出し。
  当て推量なので確定はしない (「確実 = 誤りなし」を保つ) が、「使われているか」には答えられる。
- 候補の `[method-name]` = 受け手の型が分からない method 呼び (`x.f()`)。同名 method が repo に
  1 つしか無くても確定しない (std/dep の `.next()` / `.len()` を自前の定義に誤確定しないため)。
  確定するのは `self.f()` (受け手 = 今いる impl の型) と SCIP が答えた分。bake すればここは減る。
- 候補の `[external]` = SCIP または名前照合が「呼び先は repo の外 (std / 依存 crate)」と判定した物。
  同名の定義が repo にあっても、その呼び出しは別物という意味。
- 候補の `[value-ref]` = 関数を値として渡した参照 (`map(f)` / `&f` / `Some(f)` / `S { f: g }` / `vec![f]`)。
  同じボディで束縛された名前 (局所変数) は除外済み。呼ぶのは渡した先なので確定しないが、
  「まだ使われているか」の判断には使える (`search callers:0 namecalls:0` の偽 "未使用" を防ぐのはこれ)。
- trait 実装 method の `callers` が 0 件なのは **trait 経由で呼ばれる**から (未使用ではない)。その旨と
  `impls <Trait>` への導線が `#` 行に出る。未使用判定は `search … traitimpl:0` で外す。
- `callers` は **確実 (callee_sym 逆引き、誤りなし) + 候補 (未確定=要 Read で確認) + 別 sym に確定** の3分割。
  「全 caller を掴んだか」はこの3つの合計で判断でき、grep に戻らなくていい。
- 名前が無いと **近い名前を自動提案** (typo 救済)。

## 効いてくる正直な限界

- **鮮度は自動 (query 時に古ければ増分 update してから回答、ms オーダー)。** 判定は既知 dir / file の stat のみ
  (walk は dir に増減があった時だけ)、編集直後の query は編集した file だけ読む。lock が取れない時だけ
  古い結果+stderr 警告に落ちる。full 再 index (heal / 初回) は db ごとの flock で直列化 — 並列に叩いても
  待って成果を再利用するだけで、壊れも二重焼きもしない (作りかけは `<db>.tmp-<pid>` に焼いて rename で差し替え)。
- **精度は食わせた SCIP の feature 網羅に依存 (GIGO)。** 確定 facts は rust-analyzer のもの。
- **`stats` の率は「repo 内呼び出しのうち確定できた割合」。** std / 依存 crate への呼び出しは
  index に定義が無く構造的に解決不能なので分母から外す (混ぜると corpus の外部依存率になる —
  tokio は call-site の 47% が外部)。実測: bake 済みで tokio 59.1% / ripgrep 90.4% / enchudb 80.2%、
  syn 層のみだと 15〜24% (受け手不明の method を確定させない分)。`stats path:<substr>` で
  repo の一部だけの率も出る (どこなら確定を信じてよいかが分かる)。
- **hover / 補完 / 診断 / 式の型推論は無い** (人間のエディタ用機能。Claude は Read + `cargo check` で足りる)。
- **full 再 index (INDEX_VER 更新 / heal / 明示 index) は SCIP facts を落とす。** 精度が黙って syn 層まで
  下がるので、その時は stderr に焼き直し推奨が出る。今の状態は `kenning stats` の `bake:` 行で分かる。
- index は派生物 → **VCS に混ぜない** (gitignore、local に持つ)。
