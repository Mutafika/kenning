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
hundreds of tool calls for a single question (1,151 on enchudb, measured below). kenning's
`impact` answers it from a pre-baked graph in one reply: **13–84× fewer bytes** across the
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
cargo install --git https://github.com/Mutafika/kenning
```

That's the whole install — the [enchudb](https://github.com/Mutafika/enchudb) engine is
pulled in as a pinned git dependency. For precise mode (`bake`), also have rust-analyzer
available (`rustup component add rust-analyzer`).

If your global git config rewrites GitHub HTTPS URLs to SSH — `url."git@github.com:".insteadOf
https://github.com/`, a common setup — cargo's bundled libgit2 cannot authenticate the
rewritten URL and the enchudb fetch fails with *"no authentication methods succeeded"*.
Hand the fetch to the git CLI, which honours the rewrite and your ssh key:

```bash
CARGO_NET_GIT_FETCH_WITH_CLI=true cargo install --git https://github.com/Mutafika/kenning
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
kenning def     <name>              definition + signature + first doc line (hover)
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
kenning callers <name> [container]  who-calls: confirmed ∪ unresolved candidates, with positions
kenning callees <name> [container]  outgoing calls
kenning edges                       all cross-file call edges, aggregated (from\tto\tcount TSV)
kenning refs    <name> [container]  find-all-references (needs bake; includes type refs, read/write)
kenning impls   <trait|type>        go-to-implementation, both directions
kenning impact  <name> [container] [--confirmed-only]
                                    transitive callers = blast radius (reverse BFS). Value
                                    references (map(f)) are followed by default — for a blast
                                    radius, a miss is worse than a maybe
kenning tests   <name> [container]  tests that reach this symbol = impact ∩ is_test
kenning path    <from> <to>         one call path from A to B (forward BFS)
kenning across  <name>              cross-repo precise references over every indexed repo
kenning search  kind:method vis:pub container:Engine path:engine.rs   faceted equality-AND
kenning search  reachable:0         definitions unreachable from live roots (pub / #[test] / trait
                                    impls / main / item-level macro args) = deletion candidates.
                                    Finds dead chains and mutually-recursive dead clusters in one pass
kenning search  attr:<substr>       attribute substring (deprecated / allow(dead_code) / serde / cfg)
kenning search  kind:fn callers:0 namecalls:0 test:0   the one-hop version (in-degree 0). definitions nothing calls (deletion
                                    candidates). callers = confirmed edges, namecalls = name matches.
                                    Add traitimpl:0 for methods — trait impls are called through the trait.
                                    Names that appear textually outside their definition are dropped
                                    automatically (DSL macros etc.); --no-lexical keeps them
kenning outline <path|dir>          file structure without reading the file; a directory maps
                                    the files under it (symbol count / loc), `.` maps the repo
kenning bake                        run rust-analyzer once, ingest SCIP → RA-grade precision
kenning stats   [path:<substr>]     index size + resolution breakdown; path: narrows to a subtree
kenning cache   [ls|prune]          list / prune auto-derived indexes (missing repo, old format,
                                    --older-than D, --dry-run)
kenning --version                   version
```

Output is deterministic `path:line<TAB>detail` rows on stdout (progress goes to stderr) —
each line can be fed straight into a file reader. A `CLAUDE.md` ships with the repo so
Claude-family agents pick the right subcommand without prompting.

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

## Design points

- **Honest completeness.** Calls inside macros that do not parse as Rust (`proptest!` and friends) are
  recovered lexically and labeled `[macro-token]` — candidates only, never promoted to confirmed.
  A function passed *as a value* (`map(f)` / `any(f)`) shows up as a
  `[value-ref]` candidate — the call happens wherever it was handed to, so it is never promoted to
  confirmed, but "is this still used?" is answerable. `callers` returns three labeled sets: *confirmed* (reverse lookup
  of resolved edges — no false positives), *candidates* (same-name call-sites not yet
  resolved — check these), and *resolved-to-other*. The union is grep-complete, the labels
  tell you which rows you can trust blindly. The tool never guesses.
- **A method call with an unknown receiver is never confirmed.** syn cannot know the type of
  `x` in `x.f()`, so even a repo-unique method name stays a located `[method-name]` candidate.
  Only `self.f()` (receiver = the enclosing impl type) and whatever SCIP answers get confirmed.
  Without that line, `.next()` / `.len()` show up as "confirmed callers" of your same-named method.
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
[bench/RESULTS.md](bench/RESULTS.md).

| Suite | tokio (722 files) | ripgrep (100 files) | enchudb (258 files) | What it measures |
|---|---|---|---|---|
| **agent** — bytes to answer "who calls X?" | **3.6×** less, 17 calls → 1 | **1.4×** less, 3 calls → 1 | **10.0×** less, 46 calls → 1 | 20 fixed questions, grep-route modeled *optimistically* (lower bound) vs actual `callers` output |
| **beyond** — "what breaks if I change X?" (`impact`) | **43×**, 327 calls → 1 | **33×**, 12 calls → 1 | **84×**, 1,151 calls → 1 | transitive-caller BFS: grep route = the manual grep+read recursion an agent actually performs |
| **quality** — grep noise on 100 random symbols | median 33 % | median 33 % | median 33 % | share of `\bname\(` hits that are defs/comments/strings/other symbols — rows an agent reads for nothing |
| **micro** — warm query latency | 83 ns – 3.9 µs | 125 ns – 1.4 µs | 125 ns – 2.9 µs | faceted counts, def lookup, precise reverse-edge callers |

The same suite also measures the other non-search queries: `impls` (go-to-implementation)
10.6–23.6×, `outline` (structure without reading the file) 5–30× on source files (tokio's 143 KB
CHANGELOG compresses 30×; ripgrep's `raw.csv` test data hits 1,000×+, which says more about CSV
than about kenning), `def` (hover: location + signature + doc line) 6.2–9.2×. Faceted queries
have no grep equivalent at all — they run in µs and are reported as a capability, not a ratio.
Note the pattern:
**the deeper the question, the bigger the win** — on ripgrep, plain who-calls is only 1.7×
but transitive impact is 13×, because the grep route multiplies per BFS hop.

The spread is the honest story: the advantage scales with how widely symbols are called.
ripgrep — small and famously well-factored — is the floor (1.4×, median symbol called from
3 sites); enchudb's hot symbols (46 sites) show 10.0×. Worst cases are where grep drowns
hardest: `len` in enchudb = 1,185 grep hits → 1,223 name-matching call-sites, of which 122 are
confirmed callers.

**Text search vs `rg`** (the `text` suite, same run): 20 high-frequency terms per corpus, the
same word handed to both engines. Hit counts are **identical on 64 of 80** questions, and every
difference falls under one of two documented rules — kenning does not index generated lock files
or anything over 1 MiB (13 questions where it reports fewer), and `rg` stops at the first NUL byte
while kenning reads the file whole (3 questions on ripgrep's `sherlock-nul.txt`, where kenning
reports more). Wall clock is the same order: rg 7.0–15.7 ms vs text 4.9–22.0 ms across the four
corpora, with every kenning row additionally carrying its enclosing function, heading path or
TOML table. This is the suite behind the claim that you can stop reaching for grep inside a Rust
repo — before it, that was the one claim here with no measurement under it.

**Head-to-head vs rust-analyzer** ([bench/VS-RA.md](bench/VS-RA.md), `./bench/vs-ra.sh`):
time and memory to go from cold to "can answer who-calls" — RA (`analysis-stats`, its own bench
tool): 18.9 s / 3.1 GB on enchudb, vs kenning syn index: 0.30 s / 136 MB, zero resident after.
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
Glean wins on facts disk (14 MB vs 87 MB — ours also carries the syn call graph and facets)
and serves stale SCIP gracefully, since it never joins against live source.

The CodeQL and Glean matchups were measured on an earlier enchudb snapshot (175 files) and are
not re-run every release — a 68-minute database build is not a per-release cost anyone should pay
twice. The order of magnitude is the claim, not the third decimal.

**Head-to-head vs ast-grep** (structural search; same questions, inside the agent suite):
its structural matches equal kenning's confirmed ∪ candidate sets almost exactly
(tokio `sleep` 152 → 106+89, `registration` 89 → 79+19 — kenning is *ahead* by the calls written
inside macro arguments, which tree-sitter's three patterns cannot reach; the confirmed side is
SCIP/rust-analyzer-backed) — an independent cross-validation that
call-site detection is complete. The differences: median 54–303 ms per question (repo walk, three
call-shape patterns the user must enumerate) vs 5–12 ms (indexed), and no name resolution —
it cannot say *which* definition a call belongs to, and has no impact/path/faceted/cross-repo.

- Index build (syn layer, cold, measured 2026-09-08): enchudb 258 files / 4,110 symbols / 44,382 call-sites in
  **0.29 s**; tokio 722 files / 7,156 symbols / 38,216 call-sites in **0.33 s**.
- Incremental update after a one-file edit: **5–21 ms** (median: 4.8 on kenning's 31 files and
  enchudb's 297, 12.6 on ripgrep's 207, 20.8 on tokio's 770). The per-query freshness check (a
  dir-gated stat-walk) costs 0.8–4.4 ms; a whole `callers` query, process start included, is
  5–12 ms (bench medians: kenning 4.9, ripgrep 7.7, tokio 10.2, enchudb 11.7 ms — `rg` answering
  the same questions takes 7.5–15.6 ms, so the speed is a tie and the difference is what comes back).
- `bake`: one rust-analyzer batch run, then **zero** resident memory. Measured: ripgrep 7 s /
  1.1 GB, tokio 28 s / 2.0 GB, enchudb 46 s / 2.3 GB. **In-repo confirmation rate** (calls into
  std / dependency crates are excluded from the denominator — see below) before → after:
  tokio 15.5 % → 59.1 %, ripgrep 23.9 % → 90.4 % (both `features = "all"`), enchudb
  18.1 % → 80.2 % — enchudb bakes with *default* features because `features = "all"` stalls
  past the timeout there, exactly the fallback the cap exists for. The syn layer starts low
  because it refuses to confirm a method call whose receiver type it cannot know (`x.f()`);
  name-only confirmation would list `.next()` as a caller of your own same-named method.
  What it drops stays as a located candidate. kenning itself, mostly free functions, reads
  81.9 % syn-only and 86.9 % baked — only five points of headroom, because a non-inflating syn
  layer already resolves most of what this repo is made of (`self.f()` and free functions).

## Deliberate trade-offs — what we don't do, and what it cost

Every number above was bought by *not* doing something. The full ledger:

| We don't do | What it bought | What it costs (measured / observed) |
|---|---|---|
| Type inference (`x.f()` receivers) | 0.3 s builds, 5–21 ms incremental updates, cfg-blind coverage | syn-only resolution stays at 15–28 %; precision requires `bake` (one 7–46 s / 1.1–2.3 GB RA run) |
| Hover / completion / diagnostics | zero-resident, no LSP protocol | not a human editor; agents use `cargo check` for types |
| Macro expansion | per-file parse speed | calls and impls born **of expansion** stay invisible (calls/refs written as macro arguments — `println!("{}", f())`, `criterion_group!(g, f)` — are recorded, inside a function body or at item level) |
| Resident server / file watcher | 0 RAM, zero ops, works over SSH | a 0.8–4.4 ms stat-walk on every query (5–12 ms for the whole CLI round trip); warm-µs numbers only apply in-process |
| Serving SCIP as-is (we position-join against live source instead) | answers always point at today's code | stale bakes shed precise facts (we hit `refs → 0` live in the Glean matchup; `upd_since_bake` warns) |
| Guessing (no fabricated resolution) | zero false positives in the confirmed set | agents still eyeball the *candidates* bucket |
| A general query language (Angle/QL) | zero learning curve, µs answers | arbitrary relational questions (taint tracking) stay CodeQL's territory |
| Languages other than Rust (for now) | depth (cfg recovery, trait containers) | useless in a TS/Python repo; the fact schema itself is language-neutral |
| Disk thrift | every column auto-indexed + syn graph alongside SCIP | 87 MB vs Glean's 14 MB for the same corpus |

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
