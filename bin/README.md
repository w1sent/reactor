# bin/

**Deprecated — the `reactor` CLI is Rust now**, in [`crates/reactor-cli`](../crates/reactor-cli/)
over [`crates/reactor-core`](../crates/reactor-core/)
([ADR-0034](../docs/adr/0034-reactor-cli-becomes-a-rust-library-with-a-binary.md)).

`bin/reactor` is the Python original, kept for one reason: it is the oracle
`scripts/parity.py` diffs the Rust binary against, byte for byte. It is **not**
installed by anything and the extensions' test harness no longer runs it.

It is deleted — together with `tests/test_reactor.py`, `scripts/install.py` and
`scripts/parity.py` — once the extension suite is green against the Rust binary
(MIGRATE.md, phase 1). Python may stay in this repo for development tooling and
inside skills; it may not be part of what `reactor` is
([ADR-0039](../docs/adr/0039-the-executables-contain-no-python.md)).
