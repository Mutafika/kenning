#!/bin/sh
# 課題の正解 (呼び出し箇所) を kenning と独立に作る: 対象の定義を worktree で `<name>_zz` に rename してから
# `cargo check --all-targets` を回し、出た error の位置を集める。lib の error で下流の crate が check されないので、
# error の位置の名前も `_zz` に直して、error が 0 になるまで繰り返す。
# 使い方: truth-rename.sh <rename 済み worktree> <CARGO_TARGET_DIR> <名前の正規表現 (例: 'open|insert')> <出力>
# 出力は `path:line:col: error...` の生の行。同じ位置が複数 target で重複するので path:line で束ねて truth/ に置く。
# cfg で外れた code (tokio の cfg(loom) 等) は check に現れない — kenning の答えと突き合わせて目で足す。
#
# テストの課題 (tokio-sleep-reset-tests) の正解は別の方法: 対象の本体の先頭に `println!("ZZPROBE");` を入れ、
# `cargo test --workspace --all-features --tests -- --test-threads=1 --nocapture` の出力で ZZPROBE を挟んだ
# `test <name> ...` を拾う (実行時に通ったテスト = 動的な正解。静的に届くだけのテストは含まない)。
set -e
W=$1; T=$2; NAMES=$3; OUT=$4
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
cd "$W"
: > "$OUT"
for i in $(seq 1 15); do
    CARGO_TARGET_DIR=$T nice cargo check --workspace --all-targets --message-format short --keep-going 2>&1 \
        | grep -E '^[^ ]+:[0-9]+:[0-9]+: error' | awk -F: '!seen[$1":"$2":"$3]++' > "$TMP/iter.txt" || true
    n=$(wc -l < "$TMP/iter.txt" | tr -d ' ')
    echo "iter $i: $n" >&2
    [ "$n" = 0 ] && break
    cat "$TMP/iter.txt" >> "$OUT"
    python3 - "$TMP/iter.txt" "$NAMES" <<'PY'
import re, sys
for l in open(sys.argv[1]):
    m = re.match(r'([^:]+):(\d+):(\d+): error.*?`(' + sys.argv[2] + r')`', l)
    if not m:
        print("?? (名前の error ではない — 連鎖した型エラー等)", l.strip(), file=sys.stderr)
        continue
    f, ln, col, name = m.group(1), int(m.group(2)), int(m.group(3)), m.group(4)
    lines = open(f).read().split('\n')
    s, c = lines[ln - 1], col - 1
    if s[c:c + len(name)] != name:
        print("?? 列がずれている", l.strip(), file=sys.stderr)
        continue
    lines[ln - 1] = s[:c] + name + "_zz" + s[c + len(name):]
    open(f, 'w').write('\n'.join(lines))
PY
done
