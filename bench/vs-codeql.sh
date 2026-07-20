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
"$CODEQL" version >/dev/null 2>&1 || { echo "codeql CLI が無い (KENNING_CODEQL で指定)" >&2; exit 1; }

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
    echo "== db 既存 → 構築 skip (作り直しは rm -rf $QLDB) ==" >&2
    ql_wall="(既存)"; ql_peak="-"
else
    echo "== codeql database create (enchudb) — RA 系の重さ、数十分かかる ==" >&2
    set -- $(measure /tmp/vsql-create.txt "$CODEQL" database create "$QLDB" --language=rust --source-root "$REPO" --overwrite)
    ql_wall=$1; ql_peak=$2
fi
ql_disk=$(du -sh "$QLDB" 2>/dev/null | awk '{print $1}')

echo "== codeql query run (who-calls) 初回 ==" >&2
set -- $(measure /tmp/vsql-q1.txt "$CODEQL" query run --database="$QLDB" bench/codeql/who-calls.ql)
q_wall=$1; q_peak=$2
echo "== 同 2 回目 (cache 済) ==" >&2
set -- $(measure /tmp/vsql-q2.txt "$CODEQL" query run --database="$QLDB" bench/codeql/who-calls.ql)
q2_wall=$1; q2_peak=$2
n_rows=$(grep -c '^|' /tmp/vsql-q2.txt 2>/dev/null || echo "?")

echo "== kenning 側 (同条件) ==" >&2
tmpdb="$D/vsql-cs.db"
rm -f "$tmpdb"*
set -- $(measure /tmp/vsql-cs.txt kenning index "$REPO" "$tmpdb")
cs_wall=$1; cs_peak=$2
cs_disk=$(du -sh "$tmpdb" 2>/dev/null | awk '{print $1}')
t0=$(python3 -c 'import time; print(time.time())')
KENNING_NO_STALE=1 kenning callers flush_writes --db "$tmpdb" >/dev/null 2>&1 || true
cs_q_ms=$(python3 -c "import time; print(f'{(time.time()-$t0)*1000:.0f}')")
rm -f "$tmpdb"*

{
    echo "# vs CodeQL — 「code as data」本家との頭対頭 (corpus: enchudb)"
    echo
    echo "CodeQL の Rust extractor は rust-analyzer ベース = 構図 (解析を facts に焼いて別層で引く) は"
    echo "本品と同じ。違いは規模と目的: CodeQL はセキュリティ解析向けの汎用リレーショナル QL、"
    echo "本品は agent のナビゲーション専用に薄く速く。"
    echo
    echo "| 段階 | CodeQL | kenning (syn 層) |"
    echo "|---|---|---|"
    echo "| facts 構築 wall | ${ql_wall}s | ${cs_wall}s |"
    echo "| 構築 peak RSS | ${ql_peak} MB | ${cs_peak} MB |"
    echo "| facts ディスク | ${ql_disk} | ${cs_disk} |"
    echo "| 「who calls flush_writes?」初回 | ${q_wall}s (QL コンパイル込み, ${n_rows} rows) | ${cs_q_ms} ms (CLI 起動込み) |"
    echo "| 同、2 回目 (cache 済) | ${q2_wall}s / ${q2_peak} MB | ${cs_q_ms} ms (毎回) |"
    echo
    echo "注記: CodeQL の query run はコンパイル+評価込みの単発コスト (キャッシュで 2 回目以降は速くなる)。"
    echo "QL は本品に書けない任意リレーショナル質問 (taint tracking 等) が書ける — 役割が違う。"
    echo "本品の bake (精密モード) の構築コストは VS-RA.md の RA 列を参照 (CodeQL 構築と同系統のコスト)。"
} > "$OUT"
sed -i '' "s|$HOME|~|g" "$OUT" 2>/dev/null || sed -i "s|$HOME|~|g" "$OUT"
echo "wrote $OUT" >&2
