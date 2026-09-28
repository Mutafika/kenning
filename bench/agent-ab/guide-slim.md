# kenning (Rust code navigation CLI — use it instead of grep for Rust repos)

`cd` into the repo and ask; the index is built and refreshed automatically. Every output row is
`path:line<TAB>detail`. Qualified names in the output (`Engine::open`) can be passed straight back.

```
kenning callers <name> [container]   # who calls it: confirmed (never wrong) + unresolved candidates
kenning impact  <name>               # transitive callers = what breaks if it changes
kenning tests   <name>               # tests that reach it (what to run after a change)
kenning read    <name> | <path>:<line>   # body of the definition / enclosing item
kenning def     <name>               # location + signature + doc line
kenning callees <name>               # what it calls
kenning path    <from> <to>          # call chain between two functions
kenning impls   <trait|type>         # implementations
kenning text    <term>... [-e] [path:<dir>]  # full-text search with enclosing fn / heading
kenning outline <path|dir>           # structure of a file / crate without reading it
kenning search  reachable:0          # unreachable (dead) definitions
```

`callers` splits results into confirmed / candidates / "resolved to another same-named symbol";
candidates need a look. Same-named symbols: narrow with a container (`callers new Engine`).
