# vs Glean (Meta) — head to head on the serving layer, fed the same .scip

**This is the one comparison that can use exactly the same input**: Glean's Rust ingestion is also rust-analyzer's SCIP, so
the `.scip` kenning baked (enchudb, 10MB) was fed straight into Meta's engine, comparing only
"the serving layer for the facts". Environment: OrbStack (macOS) + the official demo image
(`ghcr.io/facebookincubator/glean/demo`, 3.1GB, **amd64 only → run under Rosetta**.
The official docs note the image "currently does not work", but the latest at the time of measurement did).

| stage | Glean (demo image, Rosetta) | kenning (native) |
|---|---|---|
| SCIP ingest wall | 8.0s | 0.52s (incl. syn parse/graph) |
| ingest peak RSS | 702 MB | 268 MB |
| facts on disk | **14M** (scip facts only) | 87M (incl. syn call graph / every column facet / extref) |
| one find-refs (one-shot CLI) | ~1.0s / 114 MB | **0.011s** |
| resident | server mode is the intended deployment | 0 (a CLI each time) |

Rosetta's emulation factor is 2-3x at most — it cannot explain a 15x gap on ingest and ~90x on queries.
Disk, however, goes to Glean (kenning also holds facts beyond SCIP).
→ **Added 2026-10-02:** most of the 87M was a nearly empty vocab hash index (reserved at entity count × 16, 0.3% full). #15
made it reserve from an estimate of the term count.
→ **Correction 2026-10-05:** the "11.7 MB" written on 10-02 was a mismeasurement; it was really ~50 MB. The rest came from column (himo) arrays
indexed by the DB-wide eid, so later tables' columns carried leading zeros, and APFS materialised the holes of files
under 16 MB (enchudb#400). Fixed in enchudb 0.30; today's enchudb (larger than at that time, 366 files) is **14.2 MB** = on par with Glean.

## Comparing the answers (the 4th cross-check)

"References to Engine::flush_writes?" — **Glean 57 vs kenning 58** (same scip snapshot;
the difference of 1 is a convention: whether the definition-role occurrence counts as a ref). With the older scip it was 57 = 57.
Different serving layers give the same answer when the facts are the same — confirming the obvious was the takeaway.

## The beyond queries (impact / callers / impls / faceted) cannot be measured in Glean — a capability gap

Checked every predicate in scip.angle (Definition / Reference / SymbolKind / SymbolName etc., 20 in all):
**there is no call edge, no enclosing symbol, no facet**. So on the OSS Rust path (SCIP ingestion):

- **find-refs / goto-def**: ✅ (accurate, as measured above)
- **outline equivalent**: △ (looking up DefinitionLocation by file gives something close)
- **caller attribution for who-calls / transitive impact / impls / faceted AND**: ❌ cannot be expressed.
  Angle can express recursive queries, but with no call-edge facts underneath there is nothing to recurse over
  (inside Meta, rich dedicated indexers for Hack/C++ etc. supply these. The OSS Rust path has none)

kenning can produce impact 52x / impls / faceted from the same .scip because **it keeps a syn layer
(call sites + enclosing + facets) alongside SCIP** — the value of "marrying syn × SCIP" showed most clearly
in this comparison.

## Design differences this comparison surfaced (more important than who wins)

- **Glean serves SCIP as-is** (no source needed) → a stale input does not break it.
  **kenning joins against the live source by position** → when SCIP is stale, the precise facts for that part fall off
  (during this comparison, refs dropped to 0 after editing the source post-bake → recovered with a fresh bake.
  Bake freshness is tracked and warned about via upd_since_bake in meta)
- Glean is a multi-language schema platform + the Angle query language + server/sharding — designed for organisation scale.
  kenning is specialised for "one developer × N repos × an agent", trading for zero ceremony, zero residency, and ms
- The conclusion for the write-up: the Meta-scale architecture (bake facts, serve them from a separate layer) also works in a single binary

Reproduce: `./bench/vs-glean.sh` with OrbStack/Docker (the ~3GB image pull is extra).
