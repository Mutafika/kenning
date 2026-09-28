#!/bin/sh
# 実 agent の A/B: 同じ課題を headless の claude に「kenning あり / なし」で解かせ、token・費用・正答を比べる。
# 使い方: ./bench/agent-ab.sh [task-id...] (既定は全課題)。env: AB_REPS=回数 (既定 1) / AB_MODEL / AB_EFFORT
# 課題 = bench/agent-ab/tasks/<repo>-<name>.txt、正解 = truth/<同名>.txt (path:line、cargo check で検証済み)。
# 結果 = $AB_OUT (既定 ~/.cache/kenning-bench/agent-ab/<日時>)/<task>.<cond>.<rep>.json → agent-ab-score.py で集計。
# 課金される (1 run = 数十万 token 規模)。corpus は ./bench/corpus.sh + 各 corpus で kenning bake。
set -e
cd "$(dirname "$0")/.."
KB=$(pwd)
D="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}"
OUT="${AB_OUT:-$D/agent-ab/$(date +%Y%m%d-%H%M%S)}"
REPS="${AB_REPS:-1}"
mkdir -p "$OUT"

# 両条件とも読むだけの道具。A は kenning の実行と、利用者が実際に置く CLAUDE.md (運用そのまま) を足す
BASE_TOOLS="Read Grep Glob Bash(rg:*) Bash(grep:*) Bash(sed:*) Bash(cat:*) Bash(head:*) Bash(tail:*) Bash(find:*) Bash(ls:*) Bash(wc:*) Bash(awk:*)"

run() { # task cond rep
    t=$1; c=$2; r=$3
    repo=$D/${t%%-*}
    tools="$BASE_TOOLS"; extra=""
    if [ "$c" = A ]; then tools="$tools Bash(kenning:*)"; extra="$(cat "$KB/CLAUDE.md")"; fi
    (cd "$repo" && claude -p "$(cat "$KB/bench/agent-ab/tasks/$t.txt")" \
        ${AB_MODEL:+--model "$AB_MODEL"} ${AB_EFFORT:+--effort "$AB_EFFORT"} \
        --tools Bash,Read,Grep,Glob --allowedTools $tools --permission-mode dontAsk \
        --strict-mcp-config --no-session-persistence --output-format json \
        ${extra:+--append-system-prompt "$extra"} \
        > "$OUT/$t.$c.$r.json" 2> "$OUT/$t.$c.$r.err") || echo "fail: $t.$c.$r" >&2
}

tasks="$*"
[ -n "$tasks" ] || tasks=$(ls bench/agent-ab/tasks | sed 's/\.txt$//')
for r in $(seq 1 "$REPS"); do
    for t in $tasks; do
        # A と B を同時に走らせて負荷条件を揃える
        run "$t" A "$r" & run "$t" B "$r" & wait
    done
done
echo "$OUT"
python3 bench/agent-ab-score.py "$OUT"
