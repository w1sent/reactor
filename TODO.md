# Observed problems
- Inspect how well the current architecture covers cache optimization: which surfaces rebuild the system prompt and when, what each transition costs against the cached prefix, and whether any of it churns per turn. `/cache` now reports reads, writes and cold requests from the log (ADR-0059); read it after a day of work, then decide on `cache.prefixTtl`.

# Deferred
- Child agents (ADR-0063): deliver a background child's report when it settles, on the inbox (ADR-0061), instead of only on `wait`; list children under their parent in the startup session picker too.
- References (ADR-0066): fold the reading tools into `case_get` once it has been watched in use; open a SQLite table or a JSON pointer at a reference; and the ADR for registering programs (a disassembler's plugin and its MCP server) as reference schemes, with Binary Ninja as the first.

