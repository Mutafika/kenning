#!/bin/sh
# 対 CodeQL 頭対頭 → bench/VS-CODEQL.md を生成。
# CodeQL = GitHub の「code as data」本家 (Rust 対応は rust-analyzer ベースの extractor)。
# 問い: ①facts 構築のコスト (db create) ②「who calls X?」1 問のコスト (query run)。
# 前提: codeql CLI が PATH に居る (KENNING_CODEQL で上書き可) + codeql/rust-all pack。
set -e
cd "$(dirname "$0")/.."
D="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}"
CODEQL="${KENNING_CODEQL:-codeql}"
OUT=bench/VS-CODEQL.md
"$CODEQL" version >/dev/null 2>&1 || { echo "codeql CLI not found (set KENNING_CODEQL)" >&2; exit 1; }

measure() { # $1=出力ファイル, $2...=cmd → "wall_s peak_mb" (stage ごとに別ファイル = 上書き事故防止)
    out="$1"; shift
    /usr/bin/time -l "$@" > "$out" 2> "$out.time" || true
    wall=$(awk '/real/ && /user/ && /sys/ {print $1; exit}' "$out.time")
    peak=$(awk '/maximum resident set size/ {printf "%d", $1/1048576; exit}' "$out.time")
    echo "${wall:-?} ${peak:-?}"
}

REPO=../enchudb
QLDB="$D/codeql-enchudb-db"

if [ -d "$QLDB" ]; then
    echo "== db exists → skipping build (rm -rf $QLDB to rebuild) ==" >&2
    ql_wall="(existing)"; ql_peak="-"
else
    echo "== codeql database create (enchudb) — RA-class weight, takes tens of minutes ==" >&2
    set -- $(measure /tmp/vsql-create.txt "$CODEQL" database create "$QLDB" --language=rust --source-root "$REPO" --overwrite)
    ql_wall=$1; ql_peak=$2
fi
ql_disk=$(du -sh "$QLDB" 2>/dev/null | awk '{print $1}')

echo "== codeql query run (who-calls) first run ==" >&2
set -- $(measure /tmp/vsql-q1.txt "$CODEQL" query run --database="$QLDB" bench/codeql/who-calls.ql)
q_wall=$1; q_peak=$2
echo "== same, second run (cached) ==" >&2
set -- $(measure /tmp/vsql-q2.txt "$CODEQL" query run --database="$QLDB" bench/codeql/who-calls.ql)
q2_wall=$1; q2_peak=$2
n_rows=$(grep -c '^|' /tmp/vsql-q2.txt 2>/dev/null || echo "?")

echo "== kenning side (same conditions) ==" >&2
tmpdb="$D/vsql-cs.db"
rm -rf "$tmpdb"*  # db は v10 以降 directory
set -- $(measure /tmp/vsql-cs.txt kenning index "$REPO" "$tmpdb")
cs_wall=$1; cs_peak=$2
cs_disk=$(du -sh "$tmpdb" 2>/dev/null | awk '{print $1}')
t0=$(python3 -c 'import time; print(time.time())')
KENNING_NO_STALE=1 kenning callers flush_writes --db "$tmpdb" >/dev/null 2>&1 || true
cs_q_ms=$(python3 -c "import time; print(f'{(time.time()-$t0)*1000:.0f}')")
rm -rf "$tmpdb"*  # db は v10 以降 directory

{
    echo "# vs CodeQL — head to head with the original \"code as data\" (corpus: enchudb)"
    echo
    echo "CodeQL's Rust extractor is built on rust-analyzer = the same architecture as kenning (bake the analysis into facts, query them from a separate layer)."
    echo "The difference is scale and purpose: CodeQL is a general relational QL for security analysis;"
    echo "kenning is thin and fast, purely for agent navigation."
    echo
    echo "| stage | CodeQL | kenning (syn layer) |"
    echo "|---|---|---|"
    echo "| facts build wall | ${ql_wall}s | ${cs_wall}s |"
    echo "| build peak RSS | ${ql_peak} MB | ${cs_peak} MB |"
    echo "| facts on disk | ${ql_disk} | ${cs_disk} |"
    echo "| \"who calls flush_writes?\" first run | ${q_wall}s (incl. QL compilation, ${n_rows} rows) | ${cs_q_ms} ms (incl. CLI startup) |"
    echo "| same, second run (cached) | ${q2_wall}s / ${q2_peak} MB | ${cs_q_ms} ms (every time) |"
    echo
    echo "Note: a CodeQL query run is a one-off cost including compilation + evaluation (later runs are faster thanks to the cache)."
    echo "QL can ask arbitrary relational questions kenning cannot (taint tracking, etc.) — different jobs."
    echo "For the build cost of kenning's bake (precise mode), see the RA column in VS-RA.md (the same family of cost as a CodeQL build)."
} > "$OUT"
sed -i '' "s|$HOME|~|g" "$OUT" 2>/dev/null || sed -i "s|$HOME|~|g" "$OUT"
echo "wrote $OUT" >&2
