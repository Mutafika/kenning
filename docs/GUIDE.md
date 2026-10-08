# kenning detailed guide

The short version is `CLAUDE.md` at the repo root (read every turn). This is the detailed version: read only the
section you need with `kenning read docs/GUIDE.md#<heading>`.

kenning is a **semantic navigation CLI** for Rust code. Its customer is Claude itself, exploring code.
Instead of grep + Read, it returns a few precise rows (`path:line<TAB>detail`, which can go straight to Read).

## The rule (one line)

**Search inside a Rust repo with kenning.** Symbol questions (definition / callers / callees / implementations /
impact / faceted) go to `kenning <cmd>`; full-text search goes to `kenning text` — **`.rs`, `.md`, `.toml` and `.yml`
all through the same tool**, with context annotations, so it is a superset of grep. No db management (it is automatic).
Fall back to grep only for files kenning skips: binary / over 1 MiB / gitignored / generated lock files. Regex is
`text -e`, a dir filter is `text … path:<dir>`, same-named symbols are narrowed with `read`'s `crate:` / `path:` /
`--all`, the code around a line is `read <path>:<line>` (a range is `read <path>:<from>-<to>`), a md section is
`read <file>#<heading>`, a crate map is `outline <dir>`. stderr is a single summary line, so `2>/dev/null` is unnecessary.

## Usage (zero ceremony: cd and ask)

```bash
cd <rust-repo>                     # then just ask. The db is created in ~/.cache/kenning/ automatically
kenning callers <name>          # and incrementally updated on changes (progress on stderr, stdout is data only)
```

```bash
kenning def     <name> [path:S]     # location + signature + first doc line (like hover)
kenning read    <name> [container] [crate:X] [path:S] [--all]  # body of the definition (def + Read in one step; start here). Narrow same-named ones or --all
kenning read    <path>:<line>       # body of the item enclosing that line (instead of grep -n → sed). Non-Rust: the section under the heading
kenning read    <path>:<from>-<to>  # a line range (instead of sed -n 'A,Bp'). Lists the definitions / headings it spans first
kenning read    <file>#<heading>    # under a md heading / toml [table] / yaml key (instead of cutting CHANGELOG with awk)
kenning find    <substr>            # substring match on symbol names + file basenames (discovery; doubles as `find -name`)
kenning text    <term>... [-e] [--and] [--files] [path:S]  # full-text search + context (.rs = function / .md = heading path /
                                    #   .toml = [table]). Several terms = OR by default / `--and` = every term (`grep X | grep Y`),
                                    #   -e = regex ((?-i) for case-sensitive), `--files` = per-file counts only (`rg -c`, to
                                    #   triage a wide term), path: = dir filter. Ends with `# N hits / M files`
kenning callers <name> [container] [crate:X] [path:S]  # who-calls: confirmed ∪ unresolved candidates, with positions (narrow same-named free fns with path:)
kenning callees <name> [container] [path:S]  # what X calls (outgoing)
kenning edges                       # TSV of all cross-file call edges (from TAB to TAB count). Raw material for a dependency graph
kenning refs    <name> [container]  # find-all-refs (needs a --scip index; includes type refs and reads/writes)
kenning impls   <trait|type>        # go-to-implementation (trait ↔ type)
kenning across  <name>              # across all repos: definitions / uses in every repo + precise cross-repo refs
kenning impact  <name> [container] [path:S] [--confirmed-only]  # transitive callers = what breaks if it changes. By default also
                                    #   follows pass-by-value refs (map(f), when the name is unique) and via candidates
                                    #   (one name-match-only edge). Confirmed edges only: --confirmed-only
kenning tests   <name> [container] [path:S]  # tests that reach it = impact ∩ is_test (what to run after a change. [d] = confirmed / [c1] = via candidates)
kenning path    <from> <to>         # call chain from → to (shortest). If `to` has several same-named definitions (`park`),
                                    #   continues from the end as a tree of delegations to them (including enum dispatch targets)
kenning search  kind:method vis:pub container:Engine calls:unwrap path:engine.rs  # faceted AND
kenning search  reachable:0         # **removal candidates**: definitions no live root (pub / #[test] / trait impl / main /
                                    #   item-level macros) reaches. Dead clusters linked by chains or mutual recursion come out
                                    #   in one pass. Names that appear lexically outside their definition are dropped
                                    #   automatically (--no-lexical turns that off). The check is "an occurrence in code outside
                                    #   the **bodies** of the candidate and the dead definitions". Comments / strings / bindings
                                    #   (field declarations, locals, `.field` access) do not count as uses. Same-named
                                    #   definitions split by cfg are also dropped (the index is cfg-blind, so the inactive
                                    #   side gets no incoming edges)
kenning search  attr:deprecated     # substring match on attributes (attr:allow(dead_code) / attr:serde / attr:cfg(...))
kenning search  unsafe:block self:ref  # unsafe:(1|fn|block|0) = unsafe fn / safe fn with an unsafe block in its body (the soundness
                                    #   boundary), self:(ref|mut|owned|none) = receiver (&self / &mut self / self / none)
kenning search  unsafe:1 reachable-from:Engine::pull_raw  # reachability facets: reachable-from:X (reached from X) /
                                    #   reaches:X (reaches X = the impact set). Confirmed + pass-by-value edges
kenning uncovered [facet...]        # fn/methods no test reaches statically (#8). Unreached even counting candidate edges =
                                    #   a strong claim. Reached only via candidates and trait impls (not judged) are counted
                                    #   separately. The main use is unsafe:1
kenning search  kind:fn callers:0 namecalls:0 test:0  # the one-level version (in-degree 0). Names that appear lexically
                                    #   outside their definition are dropped automatically (DSL macros etc.; `--no-lexical`
                                    #   turns it off). (callers = confirmed / namecalls = name matches. Only confirmed at 0 =
                                    #   "maybe an unresolved call".) For methods add `traitimpl:0` (trait impls are called
                                    #   through the trait, so they are structurally 0)
kenning outline <path|dir>          # file structure without reading it. `.` = map of the repo. .md/.toml/.yml = heading
                                    #   structure = the table of contents for read <file>#…. For a dir, a map of its files
                                    #   (symbol count / loc). `read <path>` does the same (whole file: Read)
kenning changes --since HEAD        # **semantic diff** of uncommitted work (a meaning-level git diff, stateless).
                                    #   broken (definition removed but calls remain; position = the remaining call = where to fix) /
                                    #   sig (signature changed + caller count) / dead (no longer reached, or added but not wired up) /
                                    #   revived / callers → 0 (e.g. a duplicate definition stole the resolution). --since takes any
                                    #   git ref (`main` / `HEAD~3`). Caller-count changes are counts only (`--all` for rows),
                                    #   --json for NDJSON. For continuous monitoring use `--cursor <name>` (kenning advances the
                                    #   baseline, independently per caller) or `--since <token>`. Snapshots live in `<db>.changes/`
                                    #   (pruned automatically)
kenning stats [path:<substr>]       # size and resolution breakdown (in-repo confirmed rate + external / same-named / by value / macro)
kenning cache [ls|prune] [--older-than D] [--dry-run]  # inventory / cleanup of the automatic dbs (reclaims missing repos and old versions)
```

What gets indexed follows **the same rules as rg** (respects .gitignore / .ignore / hidden dirs; `target/` and
`node_modules/` are always excluded). Only repos with gitignored generated `.rs` that is actually compiled need
`KENNING_NO_IGNORE=1`.

Manual control, only when needed: `--db <path>` / env `KENNING_DB` (an explicit db is not auto-indexed),
`KENNING_NO_AUTO=1` (turns off all the magic), `KENNING_NO_STALE=1` (turns off only the freshness check).
The binary is `~/.cargo/bin/kenning` (installed with cargo install --path .).

## Checking mid-refactor (don't run cargo check every time)

**During** a change that spans many files, check with `kenning changes --since HEAD`, and for anything whose signature
changed, list the places to fix with `kenning callers <name>`. **Run `cargo check` once at the end of the turn** (the
final gate for correctness). On machines with many parallel sessions, build CPU and waiting on the `target/` lock add up.
Measured (enchudb, 13 crates, incremental):

| Edit | kenning changes | cargo check |
|---|---|---|
| body-only change | 0.5 CPU s / 0.5 s | 32 CPU s / 7–11 s |
| pub fn signature change (113 callers) | 0.2 CPU s / 0.3 s | 3.7 CPU s / 2.6–3 s (35 errors) |

changes is an approximation at the syn layer and does not see type errors — a mid-way check, not a replacement for
`cargo check`. Conversely, things `cargo check` stays silent about (a duplicate definition stealing call resolution =
callers → 0, a pub item nobody calls) show up only in changes.

## Raising precision (bake = one step)

```bash
kenning bake        # inside the repo. RA scip (injects features=all) → precise index, all automatic
```
who-calls / refs become **as precise as rust-analyzer**. What gets baked is **the cargo workspace containing the cwd**
(not the whole repo root: RA reads only one project. If it is ambiguous, kenning asks you to choose instead of baking).
The syn-layer index still covers the whole repo, so code outside the workspace keeps working at lower precision. It is a
batch run with nothing resident. Measured: kenning (3 rs) 10 s / 1.0 GB; enchudb (256 rs, 12 crates) 3 min the first time
(including dependency build scripts) → 26 s / 2.3 GB the second time.
It tries features=all, then default. all can take minutes on optional dependencies' build scripts (bundled C++ / binary
downloads) and RA has been seen to hang, so it is capped at 15 min (`KENNING_BAKE_TIMEOUT=<seconds>`): the whole process
group is stopped, it falls back to default, and from then on that repo bakes from default via a marker
(`<db>.bake-default`). To start with default, set `KENNING_BAKE_DEFAULT_FEATURES=1`.
If the repo needs custom cfgs from RUSTFLAGS (e.g. tokio's `--cfg tokio_unstable`; they don't appear in Cargo.toml, so
they can't be applied automatically), use `KENNING_BAKE_RUSTFLAGS='--cfg tokio_unstable' kenning bake`
— measured: tokio's confirmed rate 59.2% → 65.0%.
It has a free-memory gate and a serializing lock — it won't fire when it would get stuck. Without a bake, all navigation
still works on the syn layer (lower precision, never wrong).

**Auto-bake (on by default, only for repos you baked by hand once):** once 20 files have changed since the bake, a
query's incremental update starts a detached `bake` in the background (nice, separate process group, the query doesn't
wait, log in `<db>.auto-bake.log`). On an active repo the benefit otherwise fades within days (enchudb measured: confirmed
rate 80.2% → 18.5% after 193 files changed).
It does not start: within 30 min of the previous start / when load ≥ CPU count / when free memory is short (the bake's
own gate) / while another bake runs.
**Disable with `KENNING_AUTO_BAKE=0`** (`KENNING_NO_AUTO=1` also stops it). The one stderr line tells you the state.

## Reading the output (for Claude)

- Each row is `path:line<TAB>detail` = **can go straight to Read**. stdout is data only (no decoration, deterministic order).
- Qualified names in the output (`Engine::open_readonly` / `IndexLock::acquire`) **can be passed straight to the next
  command** (`def` / `read` / `callers` / `callees` / `refs` / `impact` / `tests` / `path` / `across`). Narrowing
  same-named symbols takes one step.
- stderr is only the one-line summary of auto index / update (two lines the first time) and `⚠` warnings. Read it, don't
  discard it (stale-result warnings appear there).
- `#` lines = counts and the next step ("N confirmed callers", "N unresolved candidates", "narrow with container /
  crate:X / path:S", etc.).
- `in (item level)` = a reference from outside any function (arguments of an item-level macro such as
  `criterion_group!(benches, bench_x, …)`).
- `[macro-token]` on a candidate = a call found lexically inside a DSL macro that can't be parsed (proptest! etc.).
  It is a guess, so it is never confirmed (keeping "confirmed = never wrong"), but it does answer "is this used?".
- `[method-name]` on a candidate = a method call whose receiver type is unknown (`x.f()`). Even if the repo has only one
  method of that name, it is not confirmed (so std/dep `.next()` / `.len()` aren't wrongly confirmed to your own
  definition). Confirmed are the ones whose receiver type is written in the source (`self` / argument and `let` types /
  return values of `T::new()` and free functions / through `Box::pin(x)`) and whatever SCIP answered. Baking shrinks this.
- `[external]` on a candidate = SCIP or name matching decided the callee is outside the repo (std / a dependency crate).
  Even if the repo has a same-named definition, that call is something else.
- `[value-ref]` on a candidate = a reference passing the function as a value (`map(f)` / `&f` / `Some(f)` / `S { f: g }` /
  `vec![f]`). Names bound in the same body (locals) are already excluded. The call happens wherever it was passed, so it
  is not confirmed, but it tells you whether something is still used (this is what prevents false "unused" from
  `search callers:0 namecalls:0`).
- 0 `callers` for a trait-impl method means it is **called through the trait** (not unused). A `#` line says so and
  points to `impls <Trait>`. Exclude them from unused checks with `search … traitimpl:0`.
- `callers` is split three ways: **confirmed (never wrong) + unresolved candidates (check them with Read) + resolved to
  another same-named symbol**. Whether you have every caller is decided by the sum of the three; no need to go back to grep.
- `tests` / `impact` have two layers: **confirmed `[dN]` (N = call depth) + via candidates `[c1]`**. `[c1]` = not reached
  by confirmed edges, but reached by crossing one name-match-only edge (a method call with an unknown receiver type /
  a call through a trait = a generic wrapper calling `poll_next` of `impl Stream for X`). Together they are everything
  statically reachable, so don't add grep results. Running both sets of tests is the safe side; to narrow, read `[c1]`
  and decide. Names with too many same-named definitions to follow candidates appear on a `#` line (check with the ⚠ of
  `callers <that qualified name>`). `Drop::drop` can't be called by name, so it is never a candidate.
- `g()` from `use a::f as g` counts as a call to `f` (it shows up in `callers f`). Same for `use a::{self as aa}` /
  `use a::T as U`.
- An unknown name **suggests similar names automatically** (typo rescue).

## Honest limits that matter

- **Freshness is automatic (a query on a stale index updates incrementally first, in milliseconds).** The check only
  stats known dirs / files (a walk happens only when a dir gained or lost entries), and a query right after an edit reads
  only the edited files. Only when the lock can't be taken does it fall back to the old result + a stderr warning. Full
  re-indexes (heal / first time) are serialized by a per-db flock — hitting it in parallel just waits and reuses the
  result, never corrupting or building twice (the work in progress is built in `<db>.tmp-<pid>` and swapped in by rename).
- **Precision depends on the feature coverage of the SCIP you feed it (GIGO).** Confirmed facts come from rust-analyzer.
- **The rate in `stats` is "the share of in-repo calls that could be confirmed".** Calls into std / dependency crates
  have no definition in the index and are structurally unresolvable, so they are left out of the denominator (mixing
  them in turns it into the corpus's external-dependency rate — 47% of tokio's call sites are external). Measured: baked
  tokio 73.7% / ripgrep 92.7% / enchudb 89.5%; syn layer alone 29–61% (because methods whose receiver can't be read from
  written types are not confirmed). `stats path:<substr>` gives the rate for part of the repo (tells you where confirmed
  results can be trusted).
- **No hover / completion / diagnostics / expression type inference** (editor features for humans; Claude gets by with
  Read + `cargo check`).
- **A full re-index (INDEX_VER bump / heal / explicit index) drops the SCIP facts.** Precision silently falls to the syn
  layer, so stderr recommends re-baking when it happens. The current state is on the `bake:` line of `kenning stats`.
- The index is derived data → **keep it out of VCS** (gitignore it, keep it local).
