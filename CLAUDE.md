# kenning (Rust code navigation CLI — use it instead of grep for Rust repos)

`cd` into the repo and ask; the index is built and refreshed automatically. Every output row is
`path:line<TAB>detail`. Qualified names in the output (`Engine::open`) can be passed straight back.

```
kenning callers <name> [container] [path:S]  # who calls it: confirmed (never wrong) + unresolved candidates
kenning impact  <name>               # transitive callers = what breaks if it changes
kenning tests   <name>               # tests that reach it (what to run after a change)
kenning read    <name> [path:S] | <path>:<line>  # body of the definition / enclosing item
kenning def     <name>               # location + signature + doc line
kenning callees <name>               # what it calls
kenning path    <from> <to>          # call chain between two functions
kenning impls   <trait|type>         # implementations
kenning text    <term>... [-e] [path:<dir>]  # full-text search (.rs/.md/.toml) with enclosing fn / heading
kenning outline <path|dir>           # structure of a file / crate without reading it
kenning search  reachable:0          # unreachable (dead) definitions
kenning changes --since HEAD         # semantic diff of uncommitted work (broken calls / sig changes / dead)
```

`callers` accounts for every call site of the name: confirmed / candidates (need a look) /
resolved to another same-named symbol (listed per target) — no need to re-count with grep.
Same-named symbols: narrow with a container (`callers new Engine`) or `path:`.
Mid-refactor, check with `changes --since HEAD`; run `cargo check` once at the end of the turn.
`kenning bake` (one rust-analyzer run, nothing resident) raises precision; baked repos re-bake in the
background (`KENNING_AUTO_BAKE=0` to stop). Details: `kenning read docs/GUIDE.md#<heading>`.
