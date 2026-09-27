#!/bin/sh
# Claude Code の hook から `kenning changes` を呼ぶ薄い接続 (kenning 本体は Claude を知らない)。
#
#   pre  (PreToolUse)  : この session の cursor がまだ無ければ baseline を作る (編集前の状態を起点に)
#   post (PostToolUse) : cursor からの差分を取り、何かあれば Claude の context に載せる
#
# 対象は .rs の編集だけ。cursor は session ごと (cc-<session_id>) なので、並列 session は互いの
# 起点を動かさない — そして差分には**他 session / 人間の編集も含まれる** (起点以降の repo 全体の差)。
# 依存: kenning (PATH) と jq。
#
# 設定例 (~/.claude/settings.json か repo の .claude/settings.json):
#   "hooks": {
#     "PreToolUse":  [{ "matcher": "Edit|Write|MultiEdit",
#                       "hooks": [{ "type": "command", "command": "/path/to/kenning-changes-hook.sh pre" }] }],
#     "PostToolUse": [{ "matcher": "Edit|Write|MultiEdit",
#                       "hooks": [{ "type": "command", "command": "/path/to/kenning-changes-hook.sh post" }] }]
#   }
set -u
mode="${1:-post}"
in=$(cat)
file=$(printf '%s' "$in" | jq -r '.tool_input.file_path // empty')
sid=$(printf '%s' "$in" | jq -r '.session_id // empty' | tr -cd 'A-Za-z0-9_.-')
case "$file" in *.rs) ;; *) exit 0 ;; esac
[ -n "$sid" ] || exit 0
dir=$(dirname "$file")
[ -d "$dir" ] || exit 0
cd "$dir" || exit 0
cursor="cc-$sid"

state="${TMPDIR:-/tmp}/kenning-hook"
mkdir -p "$state"
stamp="$state/$cursor"

if [ "$mode" = pre ]; then
    [ -e "$stamp" ] && exit 0
    kenning changes --cursor "$cursor" >/dev/null 2>&1 && : > "$stamp"
    exit 0
fi

out=$(kenning changes --cursor "$cursor" --limit 20 2>/dev/null) || exit 0
: > "$stamp"
# 差分 0 は黙る (毎編集で「変化なし」を読ませない)
printf '%s\n' "$out" | grep -q '^# changes since .*broken 0 / sig 0 / dead 0 / revived 0 / callers 0' && exit 0
printf '%s\n' "$out" | grep -q '^# changes since' || exit 0
body=$(printf '%s\n' "$out" | grep -v '^# token:')
jq -n --arg ctx "kenning changes (前回の確認からの repo 全体の意味的な差分。他 session や人間の編集も含む):
$body" '{hookSpecificOutput: {hookEventName: "PostToolUse", additionalContext: $ctx}}'
