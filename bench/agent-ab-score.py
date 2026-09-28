#!/usr/bin/env python3
"""agent-ab.sh の結果を集計: 条件 (A=kenning あり / B=なし) ごとの token・費用・turn・時間・正答 (recall/precision)。
使い方: python3 bench/agent-ab-score.py <結果 dir>"""
import json, os, re, sys, statistics
from collections import defaultdict

HERE = os.path.dirname(os.path.abspath(__file__))
TRUTH = os.path.join(HERE, "agent-ab", "truth")
CORPUS = os.environ.get("KENNING_BENCH_DIR", os.path.expanduser("~/.cache/kenning-bench"))
LOC = re.compile(r"([\w./-]+\.rs):(\d+)")


def answer_locs(text):
    """`ANSWER:` 以降の path:line を repo 相対に正規化して集合で返す。"""
    tail = text.rsplit("ANSWER:", 1)[-1] if "ANSWER:" in text else ""
    out = set()
    for p, ln in LOC.findall(tail):
        p = re.sub(r"^.*?/kenning-bench/[^/]+/", "", p).lstrip("./")
        out.add(f"{p}:{ln}")
    return out


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
    return {l.strip() for l in open(f) if l.strip()}


def main(d):
    rows = defaultdict(list)  # (task, cond) -> [metrics]
    for name in sorted(os.listdir(d)):
        m = re.match(r"(.+)\.([AB])\.(\d+)\.json$", name)
        if not m:
            continue
        task, cond = m.group(1), m.group(2)
        try:
            j = json.load(open(os.path.join(d, name)))
        except (json.JSONDecodeError, OSError):
            print(f"# 壊れた結果: {name}", file=sys.stderr)
            continue
        u = j.get("usage", {})
        tok_in = u.get("input_tokens", 0) + u.get("cache_creation_input_tokens", 0) + u.get("cache_read_input_tokens", 0)
        got = answer_locs(j.get("result", ""))
        truth = truth_of(task)
        rec = prec = None
        if truth:
            got = {l for l in got if l in truth or not in_doc(task, l)}
            hit = len(got & truth)
            rec = hit / len(truth)
            prec = hit / len(got) if got else 0.0
        rows[(task, cond)].append(dict(
            cost=j.get("total_cost_usd", 0.0), tin=tok_in, tout=u.get("output_tokens", 0),
            turns=j.get("num_turns", 0), sec=j.get("duration_ms", 0) / 1000, rec=rec, prec=prec,
            err=j.get("is_error", False)))

    med = lambda xs: statistics.median(xs) if xs else float("nan")
    print("| task | cond | n | input tok | output tok | cost $ | turns | sec | recall | precision |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    tot = defaultdict(lambda: defaultdict(float))
    for (task, cond), ms in sorted(rows.items()):
        g = lambda k: med([m[k] for m in ms if m[k] is not None])
        rp = lambda k: f"{g(k):.0%}" if any(m[k] is not None for m in ms) else "-"
        print(f"| {task} | {cond} | {len(ms)} | {g('tin'):,.0f} | {g('tout'):,.0f} | {g('cost'):.2f} | "
              f"{g('turns'):.0f} | {g('sec'):.0f} | {rp('rec')} | {rp('prec')} |")
        for k in ("cost", "tin", "turns", "sec"):
            tot[cond][k] += g(k)
    if "A" in tot and "B" in tot:
        a, b = tot["A"], tot["B"]
        print(f"\n合計 (課題ごとの中央値の和): 費用 A ${a['cost']:.2f} vs B ${b['cost']:.2f} "
              f"(B/A {b['cost'] / a['cost']:.1f}x)、input token B/A {b['tin'] / a['tin']:.1f}x、"
              f"turn B/A {b['turns'] / a['turns']:.1f}x、時間 B/A {b['sec'] / a['sec']:.1f}x")


if __name__ == "__main__":
    main(sys.argv[1])
