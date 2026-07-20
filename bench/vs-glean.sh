#!/bin/sh
# 対 Glean (Meta) — 同じ .scip を両エンジンに食わせて serving 層を比較 → bench/VS-GLEAN.md 参照。
# 前提: Docker (OrbStack 等) + bake 済み repo (enchudb)。image は amd64 のみ (Apple Silicon は Rosetta)。
# 使い方: ./bench/vs-glean.sh <scip-file> <scip-symbol>
#   例: ./bench/vs-glean.sh ~/.cache/kenning/enchudb-<hash>.scip \
#       'rust-analyzer cargo enchudb-engine 0.13.1 engine/impl#[Engine]flush_writes().'
set -e
SCIP="${1:?usage: vs-glean.sh <scip-file> <scip-symbol>}"
SYM="${2:?usage: vs-glean.sh <scip-file> <scip-symbol>}"
IMG=ghcr.io/facebookincubator/glean/demo:latest
GDB="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}/gleandb"
mkdir -p "$GDB"

docker image inspect "$IMG" >/dev/null 2>&1 || docker pull --platform linux/amd64 "$IMG"

echo "== Glean: SCIP 取込 (wall / cgroup peak) ==" >&2
docker run --rm --platform linux/amd64 --entrypoint /bin/bash \
    -v "$SCIP":/work/in.scip:ro -v "$GDB":/gdb "$IMG" -c '
    time glean --db-root /tmp/g --schema dir:/glean-demo/schema/source index scip /work/in.scip --db bench/0 >/dev/null 2>&1
    echo "peak_mb=$(( $(cat /sys/fs/cgroup/memory.peak) / 1048576 ))"
    rm -rf /gdb/bench-persist; cp -r /tmp/g /gdb/bench-persist'

echo "== Glean: find-refs 1 問 (one-shot CLI) ==" >&2
docker run --rm --platform linux/amd64 --entrypoint /bin/bash -v "$GDB":/gdb "$IMG" -c "
    Q='scip.Reference { symbol = scip.Symbol \"$SYM\" }'
    time glean --db-root /gdb/bench-persist --schema dir:/glean-demo/schema/source query --db bench/0 --limit 1000 \"\$Q\" 2>/dev/null | wc -l
    echo \"peak_mb=\$(( \$(cat /sys/fs/cgroup/memory.peak) / 1048576 ))\""

echo "== kenning: 同じ scip の取込 + refs (比較側) ==" >&2
echo "  index: /usr/bin/time -l kenning index <repo> <db> --scip $SCIP"
echo "  query: time kenning refs <name>  (bake 済み repo 内で)"
