# vs CodeQL — head to head with the original "code as data" (corpus: enchudb)

CodeQL's Rust extractor is built on rust-analyzer = the same architecture as kenning (bake the analysis into facts, query them from a separate layer).
The difference is scale and purpose: CodeQL is a general relational QL for security analysis;
kenning is thin and fast, purely for agent navigation. (CodeQL 2.26.1 / rust-all 0.2.17)

| stage | CodeQL | kenning (syn layer) |
|---|---|---|
| facts build wall | **4073s (68 min)** | 2.4s |
| build peak RSS | 9930 MB | 174 MB |
| facts on disk | 301M | 67M |
| "who calls flush_writes?" first run | 1418s (incl. QL compilation) | 101 ms (incl. CLI startup) |
| same, second run (cached) | 47.5s / 1764 MB | 101 ms (every time) |

## Comparing the answers (cross-check)

For the same question CodeQL returns 57 rows, kenning 117 confirmed rows. Lining up the breakdown:

- **Library code (crates/enchudb-engine): 44 = 44, an exact match** — two RA-based resolvers,
  the numbers agree. One more independent check that kenning's confirmed edges are accurate
- The difference is almost entirely **tests/**: CodeQL's extractor dropped nearly all 60+ callers in integration tests
  (only 1 of the 57 rows is in tests/). Both pick up examples/ and benches/

## Notes (for fairness)

- QL can ask arbitrary relational questions kenning cannot (taint tracking, data flow, etc.).
  Different jobs — CodeQL "can also navigate, as a by-product of security analysis";
  kenning was cut down to navigation only, with a 2.4s build and 0.1s queries
- kenning's bake (precise mode) build cost is the RA column in VS-RA.md (~40s/6GB) — the same family as a CodeQL build,
  but 100x lighter (expected, since the extractors have different purposes)
- CodeQL's Rust support is still young (rust-all 0.2.x). The missing tests/ may be fixed in the future

Reproduce: put the `codeql` CLI on PATH and run `KENNING_CODEQL=<path> ./bench/vs-codeql.sh`
(the build is skipped if the db already exists).
