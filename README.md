# kenning

**Semantic code search for Rust, built for AI coding agents.**
[日本語 README](README.ja.md)

*A [kenning](https://en.wikipedia.org/wiki/Kenning) is the Old Norse art of compressing a
concept into a compact name — "whale-road" for the sea. This tool does that to a codebase:
whole call graphs compressed into the few lines an agent actually needs. It also contains
"ken" — the range of what one knows.*

`kenning` answers the questions an agent (or a human) actually asks while exploring a
codebase — *who calls this? what breaks if I change it? which types implement this trait?* —
in milliseconds, from a pre-baked fact database. No resident language server, no gigabytes
of RAM, no index ceremony.

```bash
cd your-rust-repo
kenning callers finish_with_oplog     # that's it — the index builds itself on first use
```

## Why

AI coding agents explore code with `grep` + reading whole files. That works, but it burns
tokens: *"what breaks if I change X?"* becomes a recursive chain of greps and reads —
hundreds of tool calls for a single question (1,322 on enchudb, measured below). kenning's
`impact` answers it from a pre-baked graph in one reply: **32–74× fewer bytes** across the
benchmark corpora, and the deeper the question the wider the gap.

The classic precise answer is a language server — but rust-analyzer runs resident at
multi-GB RSS to answer one question at a time, and an agent asks in bursts, from many
repos, often over SSH on machines where that memory doesn't exist.

`kenning` takes a third path, the same shape as Meta's Glean or Google's Kythe, scaled
down to a single local binary:

1. **Parse** every `.rs` with `syn` (fast, cfg-blind, no build needed) → symbols, call-sites,
   impls as rows in an embedded faceted database ([enchudb](https://github.com/Mutafika/enchudb)).
2. **Bake** (optional, one command): run rust-analyzer *once* as a batch compiler
   (`kenning bake`), ingest its SCIP output, and position-join it against the syn facts.
   Call resolution becomes exactly as accurate as rust-analyzer's — then rust-analyzer exits.
3. **Serve** queries from the fact DB in microseconds. Every column is auto-indexed, so
   faceted conjunctions (`kind:method vis:pub container:Engine calls:unwrap`) are bucket
   intersections, not scans.

The index is self-maintaining: stale files are detected on every query (a 0.8–4.4 ms stat-walk)
and re-indexed incrementally (5–21 ms for a one-file edit), so answers are never silently stale.

## Install

```bash
cargo install --git https://github.com/Mutafika/kenning --locked
```

That's the whole install — the [enchudb](https://github.com/Mutafika/enchudb) engine is
pulled in as a pinned git dependency (`--locked` keeps every dependency at the tested version).
For precise mode (`bake`), also have rust-analyzer available (`rustup component add rust-analyzer`).

Requirements: Rust 1.89+. Tested on macOS (arm64) and Linux (Ubuntu, arm64), with CI on both;
Windows is untested.

### Use it from an AI agent

Paste the short [CLAUDE.md](CLAUDE.md) of this repo (~2 KB) into your agent's instructions
(`CLAUDE.md`, `AGENTS.md`, …) for the Rust repos you work on. That is all the setup: the
agent runs `kenning <command>` in the repo, and the index builds and refreshes itself. Keep
it short — in our agent A/B a 1.5 KB guide beat the full 15 KB one (see below).

If your global git config rewrites GitHub HTTPS URLs to SSH — `url."git@github.com:".insteadOf
https://github.com/`, a common setup — cargo's bundled libgit2 cannot authenticate the
rewritten URL and the enchudb fetch fails with *"no authentication methods succeeded"*.
Hand the fetch to the git CLI, which honours the rewrite and your ssh key:

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git https://github.com/Mutafika/kenning --locked
```

Hacking on kenning and enchudb together? Check out both side by side and point the
dependency at your checkout via `.cargo/config.toml`:

```toml
[patch."https://github.com/Mutafika/enchudb"]
enchudb = { path = "../enchudb" }
enchudb-oplog = { path = "../enchudb/crates/enchudb-oplog" }
```

## Commands

```
kenning def     <name> [path:S]     definition + signature + first doc line (hover)
                                    (<name> also takes the qualified `Type::method` form, so a name
                                    kenning prints can be pasted straight back as an argument)
kenning read    <name> [container] [crate:X] [path:S] [--all]
                                    the definition body itself (def + file-read in one step);
                                    narrow same-named symbols, or --all to print every one
kenning read    <path>:<line>       the item enclosing that line (grep -n → sed, in one step)
kenning read    <path>:<from>-<to>  a line range (`sed -n 'A,Bp'`), headed by the definitions /
                                    headings the range spans
kenning read    <file>#<heading>    one .md heading / .toml [table] / .yml key section
kenning find    <substr>            fuzzy discovery over symbol names *and* file names
                                    (the `find -name '*x*'` half, so "where is that file" stays here)
kenning text    <term>... [-e] [--and] [--files] [path:S]
                                    full-text search over every text file, annotated with
                                    context (.rs: enclosing symbol, .md: heading path, .toml:
                                    table). Several terms = OR by default, --and = every term on
                                    the line (`grep X | grep Y`), -e = regex, --files = per-file
                                    hit counts (`rg -c`, for triaging a wide term), path: = dir filter
kenning callers <name> [container] [crate:X] [path:S]  who-calls: confirmed ∪ unresolved candidates,
                                    with positions (container `-` = the free fn; crate: / path: also narrow)
kenning callees <name> [container] [path:S]  outgoing calls
kenning edges                       all cross-file call edges, aggregated (from\tto\tcount TSV)
kenning refs    <name> [container]  find-all-references (needs bake; includes type refs, read/write)
kenning impls   <trait|type>        go-to-implementation, both directions
kenning impact  <name> [container] [--confirmed-only]
                                    transitive callers = blast radius (reverse BFS). Value
                                    references (map(f)) are followed by default — for a blast
                                    radius, a miss is worse than a maybe
kenning tests   <name> [container]  tests that reach this symbol = impact ∩ is_test ([d] confirmed / [c1] via a candidate edge)
kenning path    <from> <to>         one call path from A to B (forward BFS; continues through same-named delegation)
kenning across  <name>              cross-repo precise references over every indexed repo
kenning search  kind:method vis:pub container:Engine path:engine.rs   faceted equality-AND
kenning search  reachable:0         definitions unreachable from live roots (pub / #[test] / trait
                                    impls / main / item-level macro args) = deletion candidates.
                                    Finds dead chains and mutually-recursive dead clusters in one pass
kenning search  attr:<substr>       attribute substring (deprecated / allow(dead_code) / serde / cfg)
kenning search  unsafe:block self:ref   unsafe:(1|fn|block|0) = unsafe fn / safe fn with an unsafe block (the soundness
                                    boundary); self:(ref|mut|owned|none) = receiver kind
kenning search  unsafe:1 reachable-from:Engine::pull_raw   reachability as a facet: reachable-from:X / reaches:X
                                    AND-ed with any other facet
kenning uncovered unsafe:1          fn/methods no test reaches statically (candidate edges counted) — unverified unsafe
kenning search  kind:fn callers:0 namecalls:0 test:0   the one-hop version (in-degree 0). definitions nothing calls (deletion
                                    candidates). callers = confirmed edges, namecalls = name matches.
                                    Add traitimpl:0 for methods — trait impls are called through the trait.
                                    Names that appear textually outside their definition are dropped
                                    automatically (DSL macros etc.); --no-lexical keeps them
kenning outline <path|dir>          file structure without reading the file; a directory maps
                                    the files under it (symbol count / loc), `.` maps the repo
kenning bake                        run rust-analyzer once, ingest SCIP → RA-grade precision
kenning changes --since HEAD        semantic diff of uncommitted work: broken references, signature
                                    changes (+ caller count), newly dead / unwired definitions, callers
                                    that dropped to 0. --since takes any git ref, a sinfo snap
                                    (`snap` / `snap:<id|label>`) or a token; --cursor <name> keeps a
                                    moving baseline; --json for NDJSON. Cheap mid-refactor check —
                                    `cargo check` stays the final gate
kenning stats   [path:<substr>]     index size + resolution breakdown; path: narrows to a subtree
kenning cache   [ls|prune]          list / prune auto-derived indexes (missing repo, old format,
                                    --older-than D, --dry-run)
kenning --version                   version
```

Output is deterministic `path:line<TAB>detail` rows on stdout (progress goes to stderr) —
each line can be fed straight into a file reader. A short `CLAUDE.md` (~2 KB) ships with the repo so
Claude-family agents pick the right subcommand without prompting — copy it into your own agent
instructions; the full reference lives in [docs/GUIDE.md](docs/GUIDE.md) and is read on demand
(short beats full: the full guide costs input tokens on every turn, see the agent A/B below).

Indexing follows ripgrep's rules — `.gitignore` / `.ignore` and hidden directories are respected,
`target/` and `node_modules/` are always excluded. Overrides, for when the automatic behaviour is
wrong:

| env / flag | effect |
|---|---|
| `--db <path>` / `KENNING_DB` | use an explicit index (an explicit db is never auto-indexed) |
| `KENNING_NO_AUTO=1` | no auto index / update at all |
| `KENNING_NO_STALE=1` | keep auto-indexing, skip the per-query freshness check |
| `KENNING_NO_IGNORE=1` | index gitignored `.rs` too (repos whose build generates sources) |
| `KENNING_BAKE_TIMEOUT=<sec>` | cap one rust-analyzer run (default 900). On timeout `bake` kills the process group, falls back to default features, and remembers that choice per repo |
| `KENNING_BAKE_DEFAULT_FEATURES=1` | skip `features = "all"` from the start |
| `KENNING_BAKE_RUSTFLAGS=<flags>` | extra `RUSTFLAGS` for the rust-analyzer run — for repos whose code is behind a custom cfg that never appears in `Cargo.toml` (`--cfg tokio_unstable` moves tokio from 59.2 % to 65.0 %) |
| `KENNING_RA=<path>` | rust-analyzer binary to use for `bake` |
| `KENNING_AUTO_BAKE=0` | turn off auto-bake. By default a repo you have baked once is re-baked in the background (`nice`, detached — the query never waits) when a git commit or sinfo vup lands after the last bake, or 20 files changed since it, and only if the machine is idle enough (load per CPU < 1, free memory, one bake per machine, 30 min apart). `KENNING_NO_AUTO=1` also stops it |

## Design points

- **Honest completeness.** Calls inside macros that do not parse as Rust (`proptest!` and friends) are
  recovered lexically and labeled `[macro-token]` — candidates only, never promoted to confirmed.
  A function passed *as a value* (`map(f)` / `any(f)`) shows up as a
  `[value-ref]` candidate — the call happens wherever it was handed to, so it is never promoted to
  confirmed, but "is this still used?" is answerable. `callers` returns three labeled sets: *confirmed* (reverse lookup
  of resolved edges — no false positives), *candidates* (same-name call-sites not yet
  resolved — check these), and *resolved-to-other*. The union is grep-complete, the labels
  tell you which rows you can trust blindly. The tool never guesses.
- **Receiver types come only from what is written — and are checked against rust-analyzer.**
  `x.f()` is confirmed only when `x`'s type can be read off the source (parameter / `let`
  annotations, struct literals, `T::new()` returning `Self`, field types, return types, `?`, a
  single trait bound / `dyn` / `impl Trait`) *and* Rust's method resolution could not land somewhere
  else (a prelude type like `Vec`, an impl for `&T`, an extension trait with the same method, a
  `self: Pin<&mut Self>` receiver, a re-exported foreign type, a `#[cfg]`-switched alternative, a
  user macro that may rewrite `self`...). Everything else stays a located `[method-name]` candidate —
  otherwise `.next()` / `.len()` show up as "confirmed callers" of your own same-named method.
  `kenning bench infer` re-indexes a baked repo syn-only and compares every confirmed call with the
  `.scip` at the same position: **0 wrong on ripgrep, tokio and kenning** (it started at 43 on ripgrep
  and 286 on tokio — most of them syn-layer mistakes that predate inference and had never been
  measured).
- **Stale bakes degrade per file, never silently.** `bake` records each file's content hash; a
  re-index uses SCIP only for files unchanged since, so answers from shifted lines are never joined
  to today's code. Auto-bake refreshes at the next commit / vup.
- **cfg-blind recovery.** rust-analyzer only analyzes the active cfg configuration, so SCIP
  is silent inside `#[cfg(...)]` branches that are off. `syn` sees every branch. Where SCIP
  is silent, resolution falls back to a conservative syn resolver — kenning finds impls
  and callers that rust-analyzer itself misses.
- **GIGO is explicit.** Precision equals the SCIP you feed it. `bake` asks rust-analyzer for
  `features = "all"` via `--config-path` — a cfg-gated branch RA never sees is a call edge you
  never get. What that buys varies by crate, and we print it rather than assume it: on tokio's
  workspace it is marginal today (8,934 SCIP-confirmed edges with `all` vs 8,853 with default),
  while on enchudb `features = "all"` stalls past the timeout and `bake` falls back to default
  features. Resolution rates are printed, not hidden — but **the denominator excludes external
  calls**: a call into std or a dependency has no definition in the index and is structurally
  unresolvable, so mixing it in reports the corpus's external-dependency ratio as if it were
  precision (47 % of tokio's call-sites are external). `stats` prints the breakdown too
  (external / same-name-ambiguous / value-ref / macro-token).
- **The index is a derived artifact.** It lives in `~/.cache/kenning/`, never in your
  repo, keyed by repo root. Delete it any time; it rebuilds on the next question.
- **Cross-repo.** SCIP symbols are globally unique (crate + version), so `across` joins
  definition symbols in one repo against external-reference tables of every other indexed
  repo — repo-crossing find-references that a single-workspace language server cannot do.

## Measured (reproducible suite, not anecdotes)

Run it yourself: `./bench/corpus.sh && ./bench/run.sh` — pinned corpora (tokio @ tokio-1.43.0),
fixed random seed, methodology self-described next to every table. Full output:
[bench/RESULTS.md](bench/RESULTS.md) (re-measured on v0.5.1, 2026-09-29). Resolution accuracy
has its own suite: `kenning bench infer` (in a freshly baked repo).

**Real agents, not models** ([bench/AGENT-AB.md](bench/AGENT-AB.md), `./bench/agent-ab.sh`): the byte
ratios below model the grep route. To check what an agent actually gains, headless Claude (Opus) solved
six tasks with and without kenning, three runs each, answers graded against ground truth built
independently (rename + `cargo check`, and a probe that records which tests really execute). **Accuracy
was identical** — grep-only Opus also found all 25 `JoinSet::spawn` call sites among 1,140 same-named
`spawn` calls. What kenning bought was the route: **1.7–1.9× lower cost, half the turns, 1.7× faster**,
with the widest gap on "which tests reach this" (grep: $0.65 / 122 s, kenning: $0.23–0.27 / ~50 s).
A 1.5 KB tool guide beat the full 15 KB CLAUDE.md, which costs input tokens on every turn.

| Suite | tokio (770 files) | ripgrep (207 files) | enchudb (354 files) | What it measures |
|---|---|---|---|---|
| **agent** — bytes to answer "who calls X?" | **3.6×** less, 13 calls → 1 | **1.4×** less, 3 calls → 1 | **14.3×** less, 60 calls → 1 | 20 fixed questions, grep-route modeled *optimistically* (lower bound) vs actual `callers` output |
| **beyond** — "what breaks if I change X?" (`impact`) | **72×**, 1,226 calls → 1 | **33×**, 12 calls → 1 | **74×**, 1,372 calls → 1 | transitive-caller BFS: grep route = the manual grep+read recursion an agent actually performs |
| **quality** — grep noise on 100 random symbols | median 33 % | median 33 % | median 29 % | share of `\bname\(` hits that are defs/comments/strings/other symbols — rows an agent reads for nothing |
| **micro** — warm query latency | 166 ns – 4.0 µs | 166 ns – 1.6 µs | 125 ns – 3.7 µs | faceted counts, def lookup, precise reverse-edge callers |

The same suite also measures the other non-search queries: `impls` (go-to-implementation)
10.6–23.4×, `outline` (structure without reading the file) 5–80× on source files (enchudb's 918 KB
`engine.rs` compresses 80×, tokio's 143 KB CHANGELOG 30×; ripgrep's `raw.csv` test data hits
1,000×+, which says more about CSV than about kenning), `def` (hover: location + signature + doc line) 6.1–11.6×. Faceted queries
have no grep equivalent at all — they run in µs and are reported as a capability, not a ratio.
Note the pattern:
**the deeper the question, the bigger the win** — on ripgrep, plain who-calls is only 1.4×
but transitive impact is 33×, because the grep route multiplies per BFS hop.

The spread is the honest story: the advantage scales with how widely symbols are called.
ripgrep — small and famously well-factored — is the floor (1.4×, median symbol called from
3 sites); enchudb's hot symbols (60 sites) show 14.3×. Worst cases are where grep drowns
hardest: `tmp` in enchudb = 560 grep hits spread over dozens of same-named per-file test helpers —
kenning pins 498 of them to the helper each call actually reaches.

**Text search vs `rg`** (the `text` suite, same run): 20 high-frequency terms per corpus, the
same word handed to both engines. Hit counts are **identical on 65 of 80** questions, and every
difference falls under one of two documented rules — kenning does not index generated lock files
or anything over 1 MiB (12 questions where it reports fewer), and `rg` stops at the first NUL byte
while kenning reads the file whole (3 questions on ripgrep's `sherlock-nul.txt`, where kenning
reports more). Wall clock is the same order: rg 6.8–15.2 ms vs text 5.2–22.3 ms across the four
corpora, with every kenning row additionally carrying its enclosing function, heading path or
TOML table. This is the suite behind the claim that you can stop reaching for grep inside a Rust
repo — before it, that was the one claim here with no measurement under it.

**Head-to-head vs rust-analyzer** ([bench/VS-RA.md](bench/VS-RA.md), `./bench/vs-ra.sh`):
time and memory to go from cold to "can answer who-calls" — RA (`analysis-stats`, its own bench
tool): 18.9 s / 3.1 GB on enchudb, vs kenning syn index: 0.30 s / 118 MB, zero resident after.
Precision trade and feature-scope caveats are written next to the table.

**Head-to-head vs CodeQL** ([bench/VS-CODEQL.md](bench/VS-CODEQL.md), `./bench/vs-codeql.sh`):
GitHub's "code as data" engine, whose Rust extractor is also rust-analyzer-based — the same
architecture, built for security analysis instead of navigation. On enchudb: database build
**68 min / 9.9 GB / 301 MB** vs 2.4 s / 174 MB / 67 MB; one who-calls query **47.5 s even
cached** vs 0.1 s. On lib code the answers agree exactly (44 = 44 — a third independent
cross-validation); CodeQL currently drops most `tests/` callers (57 rows vs 117). QL can ask
things kenning never will (taint tracking) — different jobs, same facts idea.

**Head-to-head vs Glean (Meta)** ([bench/VS-GLEAN.md](bench/VS-GLEAN.md), `./bench/vs-glean.sh`):
the purest matchup — Glean's Rust path is also rust-analyzer SCIP, so we fed **the exact same
.scip file** to both engines and compared only the serving layer. Ingest 8.0 s / 702 MB vs
0.52 s / 268 MB; one find-refs query ~1.0 s vs 0.011 s (Rosetta explains at most 2–3× of that);
answers agree (57 vs 58, a def-role counting nuance — fourth independent cross-validation).
Disk is now on par (Glean 14 MB vs 14.2 MB, measured 2026-10-05 on the current, larger enchudb tree — the 87 MB in the
original run was an almost-empty vocab hash index (#15) plus column padding that APFS materializes (enchudb#400, fixed in
enchudb 0.30). The 11.7 MB we published on 2026-10-02 was a mis-measurement) and Glean serves stale SCIP gracefully, since it never joins against live source.

The CodeQL and Glean matchups were measured on an earlier enchudb snapshot (175 files) and are
not re-run every release — a 68-minute database build is not a per-release cost anyone should pay
twice. The order of magnitude is the claim, not the third decimal.

**Head-to-head vs ast-grep** (structural search; same questions, inside the agent suite):
its structural matches equal kenning's confirmed ∪ candidate sets almost exactly
(tokio `registration` 89 → 91+21, `unbounded_channel` 30 → 66 — kenning is *ahead* by the calls written
inside macro arguments, which tree-sitter's three patterns cannot reach; the confirmed side is
SCIP/rust-analyzer-backed) — an independent cross-validation that
call-site detection is complete. The differences: median 58–290 ms per question (repo walk, three
call-shape patterns the user must enumerate) vs 5–12 ms (indexed), and no name resolution —
it cannot say *which* definition a call belongs to, and has no impact/path/faceted/cross-repo.

- Index build (syn layer, no SCIP, measured 2026-10-05, median of 3): enchudb 322 files / 5,573 symbols / 65,719
  call-sites in **0.45 s**; tokio 722 files / 7,723 symbols / 38,428 call-sites in **0.42 s**.
- Incremental update after a one-file edit: **5–21 ms** (median: 4.8 on kenning's 31 files and
  enchudb's 297, 12.6 on ripgrep's 207, 20.8 on tokio's 770). The per-query freshness check (a
  dir-gated stat-walk) costs 0.8–4.4 ms; a whole `callers` query, process start included, is
  5–10 ms (bench medians 2026-10-05: kenning 5.3, ripgrep 6.2, tokio 7.8, enchudb 9.6 ms — `rg` answering
  the same questions takes 8.3–15.2 ms, so the speed is a tie and the difference is what comes back).
- `bake`: one rust-analyzer batch run, then **zero** resident memory. Measured: ripgrep 7 s /
  1.1 GB, tokio 28 s / 2.0 GB, enchudb 46 s / 2.3 GB. **In-repo confirmation rate** (calls into
  std / dependency crates are excluded from the denominator — see below) before → after:
  tokio 33.3 % → 70.8 %, ripgrep 62.0 % → 92.7 % (both `features = "all"`), enchudb
  34.9 % → 89.5 % — enchudb bakes with *default* features because `features = "all"` stalls
  past the timeout there, exactly the fallback the cap exists for. Without a bake, the syn
  layer's written-type inference and Rust's scoping rules now reach (bench infer, v0.6.2):
  **ripgrep 62.0 %** (bake 92.7 %), **enchudb 34.9 %** (89.5 %), **tokio 33.3 %** (70.8 %) — with
  zero wrong confirmations on all of them (and on kenning itself). tokio stays low because much of it is generic, macro-generated or
  cfg-switched, exactly where a syn layer that must not guess has to stay silent. What it
  drops stays as a located candidate.

## Deliberate trade-offs — what we don't do, and what it cost

Every number above was bought by *not* doing something. The full ledger:

| We don't do | What it bought | What it costs (measured / observed) |
|---|---|---|
| Full type inference (trait solving, generic inference, std's return types) — only *written* types are used | 0.3 s builds, 5–21 ms incremental updates, cfg-blind coverage | syn-only resolution 22–81 % depending on style (tokio 22 %, ripgrep 56 %); RA-grade answers need `bake` (one 7–46 s / 1.1–2.3 GB RA run, refreshed automatically) |
| Hover / completion / diagnostics | zero-resident, no LSP protocol | not a human editor; agents use `cargo check` for types |
| Macro expansion | per-file parse speed | calls and impls born **of expansion** stay invisible (calls/refs written as macro arguments — `println!("{}", f())`, `criterion_group!(g, f)` — are recorded, inside a function body or at item level) |
| Resident server / file watcher | 0 RAM, zero ops, works over SSH | a 0.8–4.4 ms stat-walk on every query (5–12 ms for the whole CLI round trip); warm-µs numbers only apply in-process |
| Serving SCIP as-is (we position-join against live source instead) | answers always point at today's code | files changed since the bake fall back to syn (per-file content hashes) until auto-bake refreshes at the next commit / vup |
| Guessing (no fabricated resolution) | zero false positives in the confirmed set | agents still eyeball the *candidates* bucket |
| A general query language (Angle/QL) | zero learning curve, µs answers | arbitrary relational questions (taint tracking) stay CodeQL's territory |
| Languages other than Rust (for now) | depth (cfg recovery, trait containers) | useless in a TS/Python repo; the fact schema itself is language-neutral |
| Disk thrift | every column auto-indexed + syn graph alongside SCIP | 14.2 MB vs Glean's 14 MB (was 87 MB before #15 / enchudb#400) |

**Are ~15 commands enough?** They are not a closed set — they are the vocabulary of questions
agents actually ask (definition / users / callees / blast radius / implementations / path /
structure), grown by dogfooding. In practice the escape hatch (grep + reading files) has been
needed for the long tail, not for navigation. When a gap shows up, a new command is an
afternoon, not a project: the facts are already in the store — `tests <name>` (which tests
exercise this symbol) is literally `impact ∩ is_test`, composed from existing facts in a
single sitting. The asset is the schema, not the command list.

## License

MIT. The storage engine ([enchudb](https://github.com/Mutafika/enchudb)) is licensed
separately (FSL-1.1-Apache-2.0).
