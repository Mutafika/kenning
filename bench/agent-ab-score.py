#!/usr/bin/env python3
"""agent-ab.sh の結果を集計: 条件ごとの token・費用・turn・時間・道具の呼び出し・正答 (recall/precision)。
使い方: python3 bench/agent-ab-score.py <結果 dir>

正解 truth/<task>.txt の行形式で採点が変わる:
  `path:line`      — 回答の `ANSWER:` 以降の path:line と集合比較 (呼び出し箇所など)
  `path<TAB>name`  — 回答の 1 行に path と name が両方あれば一致 (テスト関数など、行番号の流儀に依らない)"""
import json, os, re, sys, statistics
from collections import Counter, defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
TRUTH = os.path.join(HERE, "agent-ab", "truth")
CORPUS = os.environ.get("KENNING_BENCH_DIR", os.path.expanduser("~/.cache/kenning-bench"))
LOC = re.compile(r"([\w./-]+\.rs):(\d+)")
ROOT = re.compile(r"^.*?/kenning-bench/[^/]+/")


def rel(p):
    return ROOT.sub("", p).lstrip("./")


def answer_tail(text):
    return text.rsplit("ANSWER:", 1)[-1] if "ANSWER:" in text else ""


def answer_locs(text):
    """`ANSWER:` 以降の path:line を repo 相対に正規化して集合で返す。"""
    return {f"{rel(p)}:{ln}" for p, ln in LOC.findall(answer_tail(text))}


def answer_names(text, truth):
    """name 形式の正解のうち、回答の同じ行に path と name が揃って出た物。回答の行数も返す。"""
    lines = [rel(l.strip()) for l in answer_tail(text).splitlines() if l.strip()]
    hit = set()
    for t in truth:
        path, name = t.split("\t")
        if any(path in l and re.search(rf"\b{re.escape(name)}\b", l) for l in lines):
            hit.add(t)
    return hit, len(lines)


def in_doc(task, loc):
    """doc コメント (doctest の例) の中の行か。正誤どちらにも数えない (cargo check の正解に載らないため)。"""
    path, ln = loc.rsplit(":", 1)
    try:
        line = open(os.path.join(CORPUS, task.split("-", 1)[0], path)).read().split("\n")[int(ln) - 1]
    except (OSError, IndexError):
        return False
    return line.lstrip().startswith(("///", "//!"))


def truth_of(task):
    f = os.path.join(TRUTH, task + ".txt")
    if not os.path.exists(f):
        return None
    return {l.rstrip("\n") for l in open(f) if l.strip()}


# kenning の代わりに打たれた検索・行切り出し (Read は kenning の後でも普通に使うので数えない)。
GREP_LIKE = ("Grep", "rg", "grep", "sed", "awk")


def load(path):
    """stream-json (.jsonl) か json (.json) から (最終 result, 道具の呼び出し Counter)。"""
    tools = Counter()
    if path.endswith(".json"):
        return json.load(open(path)), tools
    result = None
    for line in open(path):
        try:
            ev = json.loads(line)
        except json.JSONDecodeError:
            continue
        if ev.get("type") == "result":
            result = ev
        elif ev.get("type") == "assistant":
            for c in ev.get("message", {}).get("content", []):
                if c.get("type") != "tool_use":
                    continue
                name = c.get("name", "?")
                if name == "Bash":
                    name = (c.get("input", {}).get("command", "").split() or ["?"])[0]
                tools[name] += 1
    if result is None:
        raise ValueError("no result")
    return result, tools


def main(d):
    rows = defaultdict(list)  # (task, cond) -> [metrics]
    for name in sorted(os.listdir(d)):
        m = re.match(r"(.+)\.([A-Z])\.(\d+)\.jsonl?$", name)
        if not m:
            continue
        task, cond = m.group(1), m.group(2)
        try:
            j, tools = load(os.path.join(d, name))
        except (ValueError, OSError, json.JSONDecodeError):
            print(f"# broken result: {name}", file=sys.stderr)
            continue
        u = j.get("usage", {})
        tok_in = u.get("input_tokens", 0) + u.get("cache_creation_input_tokens", 0) + u.get("cache_read_input_tokens", 0)
        text = j.get("result", "")
        truth = truth_of(task)
        rec = prec = None
        if truth and all("\t" in t for t in truth):
            hit, n = answer_names(text, truth)
            rec = len(hit) / len(truth)
            prec = len(hit) / n if n else 0.0
        elif truth:
            got = {l for l in answer_locs(text) if l in truth or not in_doc(task, l)}
            hit = len(got & truth)
            rec = hit / len(truth)
            prec = hit / len(got) if got else 0.0
        rows[(task, cond)].append(dict(
            cost=j.get("total_cost_usd", 0.0), tin=tok_in, tout=u.get("output_tokens", 0),
            turns=j.get("num_turns", 0), sec=j.get("duration_ms", 0) / 1000, rec=rec, prec=prec,
            tools=tools))

    med = lambda xs: statistics.median(xs) if xs else float("nan")
    print("| task | cond | n | input tok | output tok | cost $ | turns | sec | recall | precision | grep-like (mean/run) | tools (mean/run) |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    tot = defaultdict(lambda: defaultdict(float))
    for (task, cond), ms in sorted(rows.items()):
        g = lambda k: med([m[k] for m in ms if m[k] is not None])
        rp = lambda k: f"{g(k):.0%}" if any(m[k] is not None for m in ms) else "-"
        tc = sum((m["tools"] for m in ms), Counter())
        tl = " ".join(f"{k} {v / len(ms):.1f}" for k, v in tc.most_common(4)) or "-"
        gr = sum(tc[k] for k in GREP_LIKE) / len(ms)
        print(f"| {task} | {cond} | {len(ms)} | {g('tin'):,.0f} | {g('tout'):,.0f} | {g('cost'):.2f} | "
              f"{g('turns'):.0f} | {g('sec'):.0f} | {rp('rec')} | {rp('prec')} | {gr:.1f} | {tl} |")
        tot[cond]["grep"] += gr
        for k in ("cost", "tin", "tout", "turns", "sec"):
            tot[cond][k] += g(k)
    if "B" in tot:
        b = tot["B"]
        for c in sorted(tot):
            if c == "B":
                continue
            a = tot[c]
            print(f"\n{c} vs B (sum of per-task medians): cost ${a['cost']:.2f} vs ${b['cost']:.2f} "
                  f"(B/{c} {b['cost'] / a['cost']:.2f}x), input token B/{c} {b['tin'] / a['tin']:.2f}x, "
                  f"output token B/{c} {b['tout'] / a['tout']:.2f}x, turn B/{c} {b['turns'] / a['turns']:.2f}x, "
                  f"time B/{c} {b['sec'] / a['sec']:.2f}x")
    for c in sorted(tot):
        print(f"\n{c}: grep-like calls, total per task set {tot[c]['grep']:.1f} (sum of per-run means)")


if __name__ == "__main__":
    main(sys.argv[1])
