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
[ -x "$RA" ] || { echo "rust-analyzer not found (set KENNING_RA)" >&2; exit 1; }

measure() { # $@=cmd → "wall_s peak_mb" (time -l の報告は stderr に出る)
    /usr/bin/time -l "$@" > /dev/null 2> /tmp/vsra-time.txt || true
    wall=$(awk '/real/ && /user/ && /sys/ {print $1; exit}' /tmp/vsra-time.txt)
    peak=$(awk '/maximum resident set size/ {printf "%d", $1/1048576; exit}' /tmp/vsra-time.txt)
    echo "${wall:-?} ${peak:-?}"
}

{
    echo "# vs rust-analyzer — from cold to an accurate answer"
    echo
    echo "The RA side is its own benchmark, \`analysis-stats\` (whole-workspace analysis + type inference = what accurate find-refs needs to know)."
    echo "The kenning side is \`index\` (syn layer). **The build cost of precise mode (bake) is the same thing as the RA column** —"
    echo "kenning pays it once as a batch instead of keeping it resident, then answers every later query from the index in µs-ms."
    echo
    echo "| corpus | tool | build wall | peak RSS | queries after build |"
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
    rm -rf "$tmpdb"*  # db は v10 以降 directory
    set -- $(measure kenning index "$repo" "$tmpdb")
    cs_wall=$1; cs_peak=$2
    # 構築後の 1 クエリ (warm CLI)
    t0=$(python3 -c 'import time; print(time.time())')
    KENNING_NO_STALE=1 kenning callers new --db "$tmpdb" >/dev/null 2>&1 || true
    q_ms=$(python3 -c "import time; print(f'{(time.time()-$t0)*1000:.0f}')")
    rm -rf "$tmpdb"*  # db は v10 以降 directory
    {
        echo "| $name | rust-analyzer (resident equivalent) | ${ra_wall}s | ${ra_peak} MB | ms, for as long as the LSP stays resident |"
        echo "| $name | kenning (syn layer) | ${cs_wall}s | ${cs_peak} MB | ${q_ms} ms (incl. CLI startup), nothing resident |"
    } >> "$OUT"
done

{
    echo
    echo "Notes for fairness: (1) kenning (syn layer) resolves less precisely than RA (no type inference; callers come back"
    echo "labelled confirmed ∪ candidates) — when precision is needed, bake pays roughly the RA column once."
    echo "(2) RA analyses only each corpus's default features (tokio's default is minimal, so the RA column looks light —"
    echo "with features=all it is heavier). (3) analysis-stats infers everything in one batch;"
    echo "a real LSP analyses lazily from where it is needed (the first response feels faster, but the total cost of the knowledge is the same)."
    echo
    echo "What each can do (not which is stronger — they have different jobs):"
    echo
    echo "| capability | rust-analyzer | kenning |"
    echo "|---|---|---|"
    echo "| hover type inference / completion / diagnostics | ✅ | ❌ (cargo check is enough for an agent) |"
    echo "| accurate find-refs / who-calls | ✅ (needs to stay resident) | ✅ after bake (= RA's facts joined by position) |"
    echo "| faceted AND (kind×vis×crate×…) | ❌ | ✅ µs |"
    echo "| transitive impact / call path | ❌ (one hop at a time) | ✅ one query |"
    echo "| analysis of inactive cfg branches | ❌ | ✅ (syn sees every branch, cfg-blind) |"
    echo "| across repos (across) | ❌ (single workspace) | ✅ (SCIP symbol join) |"
    echo "| resident memory | GBs | 0 (the index is a file) |"
} >> "$OUT"
sed -i '' "s|$HOME|~|g" "$OUT" 2>/dev/null || sed -i "s|$HOME|~|g" "$OUT"
echo "wrote $OUT" >&2
