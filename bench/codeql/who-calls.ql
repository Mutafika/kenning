/**
 * @name who-calls
 * @description 「flush_writes を呼ぶのは誰?」 — codesearch `callers flush_writes` と同じ問い。
 *              解決は CodeQL の static target (rust-analyzer ベース extractor)。
 * @kind table
 */

import rust

from Call c, Function f
where c.getStaticTarget() = f and f.getName().getText() = "flush_writes"
select c.getLocation().getFile().getRelativePath(), c.getLocation().getStartLine()
