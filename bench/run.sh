#!/bin/sh
# bench/RESULTS.md を再生成する。事前に ./bench/corpus.sh (+ 各 corpus で kenning bake 推奨)。
# 使い方: ./bench/run.sh   (kenning は cargo install 済みの想定)
set -e
cd "$(dirname "$0")/.."
D="${KENNING_BENCH_DIR:-$HOME/.cache/kenning-bench}"
OUT=bench/RESULTS.md

{
    echo "# bench results"
    echo
    echo "生成: \`./bench/run.sh\` ($(uname -s) $(uname -m)) / 手法とモデルの定義は各表の直上に自己記述。"
    echo "corpus は tag 固定 (bench/corpus.sh)。乱数は固定 seed — 同じ環境なら同じ数字が出る。"
} > "$OUT"

for repo in "$D/tokio" "$D/ripgrep" ../enchudb .; do
    [ -d "$repo" ] || { echo "skip: $repo (無い — ./bench/corpus.sh を先に)" >&2; continue; }
    name=$(basename "$(cd "$repo" && pwd)")
    echo >> "$OUT"
    echo "## corpus: $name" >> "$OUT"
    echo >> "$OUT"
    (cd "$repo" && kenning bench all) >> "$OUT"
done
# 環境の絶対パスを落とす (公開 repo に home dir を混ぜない)
sed -i '' "s|$HOME|~|g" "$OUT" 2>/dev/null || sed -i "s|$HOME|~|g" "$OUT"
echo "wrote $OUT" >&2
