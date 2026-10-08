#!/bin/sh
# ベンチ用コーパスを固定 tag で取得 (再現性のため tag を動かさないこと)。
# 置き場は $KENNING_BENCH_DIR (default: ~/.cache/kenning-bench)。
set -e
D="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}"
mkdir -p "$D"

TOKIO_TAG=tokio-1.43.0
RIPGREP_TAG=15.2.0
# 実 agent の A/B (agent-ab.sh) 用。bench/agent-ab/truth の enchudb 分はこの commit の cargo check で作った
ENCHUDB_REV=eeed941
if [ ! -d "$D/tokio" ]; then
    git clone --quiet --depth 1 --branch "$TOKIO_TAG" https://github.com/tokio-rs/tokio "$D/tokio"
fi
if [ ! -d "$D/ripgrep" ]; then
    git clone --quiet --depth 1 --branch "$RIPGREP_TAG" https://github.com/BurntSushi/ripgrep "$D/ripgrep"
fi
if [ ! -d "$D/enchudb" ]; then
    git clone --quiet https://github.com/Mutafika/enchudb "$D/enchudb"
    git -C "$D/enchudb" checkout --quiet "$ENCHUDB_REV"
fi
echo "corpus dir: $D"
echo "  tokio @ $TOKIO_TAG"
echo "  ripgrep @ $RIPGREP_TAG"
echo "  enchudb @ $ENCHUDB_REV (for agent-ab; bake with KENNING_BAKE_DEFAULT_FEATURES=1)"
echo
echo "For precision, in each corpus: (cd $D/<repo> && kenning bake)"
