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
tokens: a "what is impacted if I change X?" question costs a chain of greps and file reads
(we measured ~24 KB of tool output for one such question on a mid-size crate). The precise
answer is a handful of `path:line` rows (~1.2 KB — about **21× less**, one call instead of seven).

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

The index is self-maintaining: stale files are detected on every query (a ~10 ms stat-walk)
and re-indexed incrementally (~18 ms for a small edit), so answers are never silently stale.

## Install

```bash
cargo install --git https://github.com/Mutafika/kenning
```

That's the whole install — the [enchudb](https://github.com/Mutafika/enchudb) engine is
pulled in as a pinned git dependency. For precise mode (`bake`), also have rust-analyzer
available (`rustup component add rust-analyzer`).

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
kenning read    <name> [container]  print the definition body itself (def + file-read in one step)
kenning find    <substr>            fuzzy name discovery
kenning text    <term>              full-text search, annotated with enclosing symbol
kenning callers <name> [container]  who-calls: confirmed ∪ unresolved candidates, with positions
kenning callees <name> [container]  outgoing calls
kenning edges                       all cross-file call edges, aggregated (from\tto\tcount TSV)
kenning refs    <name> [container]  find-all-references (needs bake; includes type refs, read/write)
kenning impls   <trait|type>        go-to-implementation, both directions
kenning impact  <name> [container]  transitive callers = blast radius of a change (reverse BFS)
kenning tests   <name> [container]  tests that reach this symbol = impact ∩ is_test
kenning path    <from> <to>         one call path from A to B (forward BFS)
kenning across  <name>              cross-repo precise references over every indexed repo
kenning search  kind:method vis:pub container:Engine   faceted equality-AND
kenning outline <path>              file structure without reading the file
kenning bake                        run rust-analyzer once, ingest SCIP → RA-grade precision
kenning stats                       index size + resolution rate
```

Output is deterministic `path:line<TAB>detail` rows on stdout (progress goes to stderr) —
each line can be fed straight into a file reader. A `CLAUDE.md` ships with the repo so
Claude-family agents pick the right subcommand without prompting.

## Design points

- **Honest completeness.** `callers` returns three labeled sets: *confirmed* (reverse lookup
  of resolved edges — no false positives), *candidates* (same-name call-sites not yet
  resolved — check these), and *resolved-to-other*. The union is grep-complete, the labels
  tell you which rows you can trust blindly. The tool never guesses.
- **cfg-blind recovery.** rust-analyzer only analyzes the active cfg configuration, so SCIP
  is silent inside `#[cfg(...)]` branches that are off. `syn` sees every branch. Where SCIP
  is silent, resolution falls back to a conservative syn resolver — kenning finds impls
  and callers that rust-analyzer itself misses.
- **GIGO is explicit.** Precision equals the SCIP you feed it. `bake` injects
  `features = "all"` via `--config-path` (on tokio this is the difference between 177 and
  6,760 resolved call edges). Resolution rates are printed, not hidden.
- **The index is a derived artifact.** It lives in `~/.cache/kenning/`, never in your
  repo, keyed by repo root. Delete it any time; it rebuilds on the next question.
- **Cross-repo.** SCIP symbols are globally unique (crate + version), so `across` joins
  definition symbols in one repo against external-reference tables of every other indexed
  repo — repo-crossing find-references that a single-workspace language server cannot do.

## Measured (reproducible suite, not anecdotes)

Run it yourself: `./bench/corpus.sh && ./bench/run.sh` — pinned corpora (tokio @ tokio-1.43.0),
fixed random seed, methodology self-described next to every table. Full output:
[bench/RESULTS.md](bench/RESULTS.md).

| Suite | tokio (722 files) | ripgrep (100 files) | enchudb (174 files) | What it measures |
|---|---|---|---|---|
| **agent** — bytes to answer "who calls X?" | **5.3×** less, 15 calls → 1 | **2.3×** less, 3 calls → 1 | **10.2×** less, 30 calls → 1 | 20 fixed questions, grep-route modeled *optimistically* (lower bound) vs actual `callers` output |
| **beyond** — "what breaks if I change X?" (`impact`) | **46×**, 321 calls → 1 | **13×**, 75 calls → 1 | **52×**, 808 calls → 1 | transitive-caller BFS: grep route = the manual grep+read recursion an agent actually performs |
| **quality** — grep noise on 100 random symbols | median 43 % | median 33 % | median 33 % | share of `\bname\(` hits that are defs/comments/strings/other symbols — rows an agent reads for nothing |
| **micro** — warm query latency | 125 ns – 4 µs | similar | 125 ns – 2.3 µs | faceted counts, def lookup, precise reverse-edge callers |

The same suite also measures the other non-search queries: `impls` (go-to-implementation)
10–23×, `outline` (structure without reading the file) 7–12× (a 442 KB file compresses 42×),
`def` (hover: location + signature + doc line) 6–10×. Faceted queries have no grep equivalent
at all — they run in µs and are reported as a capability, not a ratio. Note the pattern:
**the deeper the question, the bigger the win** — on ripgrep, plain who-calls is only 2.3×
but transitive impact is 13×, because the grep route multiplies per BFS hop.

The spread is the honest story: the advantage scales with how widely symbols are called.
ripgrep — small and famously well-factored — is the floor (2.3×, median symbol called from
3 sites); enchudb's hot symbols (30 sites) show 10.2×. Worst cases are where grep drowns
hardest: `len` in enchudb = 988 grep hits, of which 46 are confirmed callers of the local `len`.

**Head-to-head vs rust-analyzer** ([bench/VS-RA.md](bench/VS-RA.md), `./bench/vs-ra.sh`):
time and memory to go from cold to "can answer who-calls" — RA (`analysis-stats`, its own bench
tool): 39 s / 6.1 GB on enchudb, vs kenning syn index: 0.45 s / 175 MB, zero resident after.
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

**Head-to-head vs ast-grep** (structural search; same questions, inside the agent suite):
its structural matches equal kenning's confirmed ∪ candidate sets almost exactly
(`tie` 321 = 321, `clone` 621 = 224+397) — an independent cross-validation that call-site
detection is complete. The differences: median 564–878 ms per question (repo walk, three
call-shape patterns the user must enumerate) vs 13–19 ms (indexed), and no name resolution —
it cannot say *which* definition a call belongs to, and has no impact/path/faceted/cross-repo.

- Index build: ~1k LOC/ms (enchudb: 174 files / 2,880 symbols / 25,895 call-sites in ~360 ms).
- Incremental update after a small edit: ~18 ms. Staleness check per query: ~10 ms.
- `bake`: one rust-analyzer batch run (peak ≈ 5 GB for ~30–50 s), then **zero** resident memory.
  Resolution on enchudb: 26.6 % (syn only) → 39.7 % (baked, features=all). The unresolved
  remainder is dominated by std/external-crate calls, which are still listed as labeled candidates.

## Deliberate trade-offs — what we don't do, and what it cost

Every number above was bought by *not* doing something. The full ledger:

| We don't do | What it bought | What it costs (measured / observed) |
|---|---|---|
| Type inference (`x.f()` receivers) | 0.5 s builds, 18 ms incremental updates, cfg-blind coverage | syn-only resolution stays at 13–26 %; precision requires `bake` (one 40 s / 5 GB RA run) |
| Hover / completion / diagnostics | zero-resident, no LSP protocol | not a human editor; agents use `cargo check` for types |
| Macro expansion | per-file parse speed | calls and impls born inside macros are invisible to every layer |
| Resident server / file watcher | 0 RAM, zero ops, works over SSH | a ~10–20 ms stat-walk floor on every query; warm-µs numbers only apply in-process |
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
