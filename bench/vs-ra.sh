#!/bin/sh
# 対 rust-analyzer 頭対頭 → bench/VS-RA.md を生成。
# 問い: 「cold の状態から who-calls / find-refs に正確に答えられるようになるまで」のコスト。
#   - RA 側   = `rust-analyzer analysis-stats .` (本家ベンチツール。全 workspace の解析+推論
#               = resident LSP が正確な find-all-refs を返すために持つ知識の構築コスト)
#   - 本品側  = `kenning index` (syn 層、bake なし)。精密モードの構築コスト = bake は
#               RA の scip 実行そのものなので RA 列とほぼ同じ (それを「一発だけ」払うのが設計)。
# 重い (GB 級 RSS) ので run.sh には入れない。手動: ./bench/vs-ra.sh
set -e
cd "$(dirname "$0")/.."
D="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}"
RA="${KENNING_RA:-$(ls "$HOME"/.rustup/toolchains/*/bin/rust-analyzer 2>/dev/null | head -1)}"
OUT=bench/VS-RA.md
[ -x "$RA" ] || { echo "rust-analyzer が見つからない (KENNING_RA で指定)" >&2; exit 1; }

measure() { # $@=cmd → "wall_s peak_mb" (time -l の報告は stderr に出る)
    /usr/bin/time -l "$@" > /dev/null 2> /tmp/vsra-time.txt || true
    wall=$(awk '/real/ && /user/ && /sys/ {print $1; exit}' /tmp/vsra-time.txt)
    peak=$(awk '/maximum resident set size/ {printf "%d", $1/1048576; exit}' /tmp/vsra-time.txt)
    echo "${wall:-?} ${peak:-?}"
}

{
    echo "# vs rust-analyzer — cold から正確に答えられるまで"
    echo
    echo "RA 側は本家ベンチ \`analysis-stats\` (全 workspace 解析+型推論 = 正確な find-refs の前提知識)。"
    echo "kenning 側は \`index\` (syn 層)。**精密モード (bake) の構築コストは RA 列と同じもの** —"
    echo "それを常駐でなく一発のバッチとして払い、以後の全クエリを index から µs-ms で返すのが本品の設計。"
    echo
    echo "| corpus | 対象 | 構築 wall | peak RSS | 構築後のクエリ |"
    echo "|---|---|---|---|---|"
} > "$OUT"

for repo in ../enchudb "$D/tokio"; do
    [ -d "$repo" ] || continue
    name=$(basename "$(cd "$repo" && pwd)")
    echo "== $name: rust-analyzer analysis-stats ==" >&2
    set -- $(cd "$repo" && measure "$RA" analysis-stats .)
    ra_wall=$1; ra_peak=$2
    echo "== $name: kenning index (syn) ==" >&2
    tmpdb="$D/vsra-$name.db"
    rm -f "$tmpdb"*
    set -- $(measure kenning index "$repo" "$tmpdb")
    cs_wall=$1; cs_peak=$2
    # 構築後の 1 クエリ (warm CLI)
    t0=$(python3 -c 'import time; print(time.time())')
    KENNING_NO_STALE=1 kenning callers new --db "$tmpdb" >/dev/null 2>&1 || true
    q_ms=$(python3 -c "import time; print(f'{(time.time()-$t0)*1000:.0f}')")
    rm -f "$tmpdb"*
    {
        echo "| $name | rust-analyzer (resident 相当) | ${ra_wall}s | ${ra_peak} MB | LSP 常駐が続く限り ms |"
        echo "| $name | kenning (syn 層) | ${cs_wall}s | ${cs_peak} MB | ${q_ms} ms (CLI 起動込み)、常駐 0 |"
    } >> "$OUT"
done

{
    echo
    echo "公平のための注記: ①kenning (syn 層) は RA より解決精度が低い (型推論なし。callers は"
    echo "確実∪候補のラベル付きで返す) — 精密が要る時の bake コスト ≈ RA 列を一発だけ払う。"
    echo "②RA は各 corpus の default features 分しか解析しない (tokio の default は最小構成なので"
    echo "RA 列が軽く見える — features=all なら更に重い)。③analysis-stats は全域推論の一括実行で、"
    echo "実際の LSP は必要箇所から lazy に解析する (体感の初回応答はこれより早いが、知識の総コストは同じ)。"
    echo
    echo "できることの差 (どちらが強い、でなく役割が違う):"
    echo
    echo "| 能力 | rust-analyzer | kenning |"
    echo "|---|---|---|"
    echo "| hover 型推論 / 補完 / 診断 | ✅ | ❌ (agent は cargo check で足りる) |"
    echo "| 正確 find-refs / who-calls | ✅ (常駐前提) | ✅ bake 後 (= RA の facts を位置 join) |"
    echo "| faceted AND (kind×vis×crate×…) | ❌ | ✅ µs |"
    echo "| 推移的 impact / call path | ❌ (1 hop ずつ) | ✅ 1 クエリ |"
    echo "| 非活性 cfg 側の解析 | ❌ | ✅ (syn が cfg-blind に全ブランチ) |"
    echo "| repo 横断 (across) | ❌ (単一 workspace) | ✅ (SCIP symbol join) |"
    echo "| 常駐メモリ | GB 級 | 0 (index はファイル) |"
} >> "$OUT"
sed -i '' "s|$HOME|~|g" "$OUT" 2>/dev/null || sed -i "s|$HOME|~|g" "$OUT"
echo "wrote $OUT" >&2
