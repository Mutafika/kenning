# vs rust-analyzer — from cold to an accurate answer

The RA side is its own benchmark, `analysis-stats` (whole-workspace analysis + type inference = what accurate find-refs needs to know).
The kenning side is `index` (syn layer). **The build cost of precise mode (bake) is the same thing as the RA column** —
kenning pays it once as a batch instead of keeping it resident, then answers every later query from the index in µs-ms.

| corpus | tool | build wall | peak RSS | queries after build |
|---|---|---|---|---|
| enchudb | rust-analyzer (resident equivalent) | 16.30s | 3179 MB | ms, for as long as the LSP stays resident |
| enchudb | kenning (syn layer) | 0.30s | 118 MB | 35 ms (incl. CLI startup), nothing resident |
| tokio | rust-analyzer (resident equivalent) | 17.04s | 2477 MB | ms, for as long as the LSP stays resident |
| tokio | kenning (syn layer) | 0.34s | 133 MB | 48 ms (incl. CLI startup), nothing resident |

kenning's peak RSS was re-measured with enchudb 0.26.11 (#270: bulk load stopped growing a reverse index nobody reads)
on 2026-09-15 (enchudb 136→118 MB / tokio 156→133 MB). Wall times are still from the earlier run
(2026-09-06): on the re-measure day the load average was 75 and both 0.26.8 and 0.26.11 stretched to ~1s
(a machine-state difference, not a version difference), so they were not replaced.

Notes for fairness: (1) kenning (syn layer) resolves less precisely than RA (no type inference; callers come back
labelled confirmed ∪ candidates) — when precision is needed, bake pays roughly the RA column once.
(2) RA analyses only each corpus's default features (tokio's default is minimal, so the RA column looks light —
with features=all it is heavier). (3) analysis-stats infers everything in one batch;
a real LSP analyses lazily from where it is needed (the first response feels faster, but the total cost of the knowledge is the same).

What each can do (not which is stronger — they have different jobs):

| capability | rust-analyzer | kenning |
|---|---|---|
| hover type inference / completion / diagnostics | ✅ | ❌ (cargo check is enough for an agent) |
| accurate find-refs / who-calls | ✅ (needs to stay resident) | ✅ after bake (= RA's facts joined by position) |
| faceted AND (kind×vis×crate×…) | ❌ | ✅ µs |
| transitive impact / call path | ❌ (one hop at a time) | ✅ one query |
| analysis of inactive cfg branches | ❌ | ✅ (syn sees every branch, cfg-blind) |
| across repos (across) | ❌ (single workspace) | ✅ (SCIP symbol join) |
| resident memory | GBs | 0 (the index is a file) |

## Precision: how often kenning says the same thing as RA (after bake)

kenning's **confirmed** set is RA's facts themselves (SCIP occurrences joined by position), so the real question is not
"is it as precise as RA" but **"how much of what RA answered does it pick up" + "what does it do where RA is silent"**.
Breakdown from `kenning index --scip` (measured 2026-09-06, features=all / enchudb with default):

| corpus | in-repo call sites | confirmed | from RA (SCIP) | syn recovered (RA silent) | unconfirmed |
|---|---|---|---|---|---|
| tokio | 20,390 | 12,073 (59.2%) | 11,235 (93.1%) | 838 | 8,317 |
| ripgrep | 10,508 | 9,611 (91.5%) | 9,589 (99.8%) | 22 | 897 |
| enchudb | 20,296 | 16,265 (80.1%) | 16,230 (99.8%) | 35 | 4,031 |

The denominator is **in-repo calls** (calls into std / dependency crates have no definition in the index and cannot be resolved, so they are excluded).

- **Confirmed is the same as RA or more.** The extra (syn recovered) is where RA was silent and the
  conservative syn resolver picked it up. It never overrides an RA answer.
- **Unconfirmed = places where RA emitted no occurrence either.** In tokio the source is 9,134 sites with no SCIP occurrence:
  4,095 where "the document itself is missing from SCIP" + 5,039 where "the document exists but has no occurrence at that position".
  **Where RA would end with "no references"**, kenning shows them as `[method-name]` / `[value-ref]` /
  `[macro-token]` candidates with positions.
- **Not a position-join miss.** 995 sites have "another occurrence on the same line", but looking at them,
  on lines like `assert_eq!(b.next()…)` RA emits only `assert_eq` (col 4) and nothing at
  `.next` (col 17) — RA resolves the path / macro name and leaves the method call on that line
  unresolved. It is not dropped because of a column offset.
- **tokio is low because of the repo itself.** Re-baking with `--cfg tokio_unstable` takes
  confirmed 11,235 → 12,332, **59.2% → 65.0%**. cfgs that depend on RUSTFLAGS do not appear in Cargo.toml, so they
  cannot be applied automatically → pass them with `KENNING_BAKE_RUSTFLAGS='--cfg tokio_unstable' kenning bake`.
  Split by target kind it is src 60.2% / tests 63.5% / benches 74.0% / examples 66.0% — small differences,
  so it is not as simple as "tests are not analysed" (check with `kenning stats path:<dir>`).
- **Without bake (syn layer only) it drops to 15–24%.** Method calls whose receiver type is unknown
  are not confirmed — it does not lie, but if you need precision, bake is required.
