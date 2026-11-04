# Patched copy of `surrealdb-kvs-surrealkv` 4.0.0

This is SurrealDB's SurrealKV storage adapter, copied from crates.io and used in
place of the original through `[patch.crates-io]` in the project's `Cargo.toml`.

SurrealDB 4.0 never passes an encryption key to SurrealKV, so a database cannot
be encrypted at rest. The patch adds `set_key_manager()`, which the application
calls before opening the database; `options()` then enables SurrealKV's
encryption with that key.

Every change is marked `PATCH (surrealdb-explore)` in `src/lib.rs`.

To update SurrealDB: copy the new version of this crate from
`~/.cargo/registry/src/*/surrealdb-kvs-surrealkv-<version>/`, re-apply the
marked changes, and update the version in `[patch.crates-io]` if needed.
