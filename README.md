# SurrealDB Explorer

A desktop app for exploring a [SurrealDB](https://surrealdb.com) database as an
interactive graph. It's written in Rust with [iced](https://iced.rs).

It also has a dedicated view for **Canton ledger event graphs**, and a built-in
**MCP server** that lets Claude Code (or any MCP client) query the database and
draw in the app's window.

![Canton event graph view](docs/screenshots/canton-cols3-top.png)

## Features

- **Graph view.** Records are nodes and relations are edges, in a force-directed
  layout. You can hover, select, hide, focus on a record and see its
  neighbourhood. Large databases open on a table overview first.
- **Table view and query panel.** Run SurrealQL and see each query's result as
  its own graph or as rows.
- **Canton event graph view.** This view is for a SurrealDB copy of a Canton
  ledger: one column per transaction, with the event tree indented under it. It
  draws `consumes` and `target` arrows, fades archived contracts, and can show
  parties in their own column. You can filter by node kind, party (each party's
  projection), template, active or archived, offset range and text search. The
  view can page through the latest *n* transactions and follow new ones as they
  arrive.
- **Two ways to connect.** Open a local SurrealKV database folder, or connect to
  a SurrealDB server (`ws://…`) as a root, namespace or database user.
- **Auto refresh.** The graph redraws when another program changes the database.
- **MCP server for Claude Code.** It lets Claude describe the schema, run
  queries, draw results in the window, and read, set or scroll to the current
  selection. Queries that would change data run only after you click Allow in
  the window.
- **Sample data.** Settings → Data → *Load sample data* or *Load Canton sample*.
  There is a larger Canton demo in [`demo/canton_demo.surql`](demo/canton_demo.surql).

## Build and run

You need a recent stable Rust toolchain (edition 2024).

```sh
cargo run --release
```

The first run creates `config.toml` in the current folder. Logs go to the
terminal and to `logs/`. Every setting can also be changed in the app under
**Settings**.

To build a macOS `.app` with the icon, use
[cargo-bundle](https://github.com/burtonageo/cargo-bundle):

```sh
cargo bundle --release
```

## Configuration

`config.toml` has these sections:

| Section      | What it sets |
|--------------|--------------|
| `[database]` | `connection` (`folder` or `server`), `path`, `server`, `username`, `user_level` (`root`, `namespace` or `database`), `password`, `namespace`, `database` |
| `[graph]`    | Record limits, auto refresh, the visualization (`standard` or `canton`) and Canton view options |
| `[logging]`  | Log level and folder |
| `[mcp]`      | `enabled` and `port` (default 8765) |

`password` can be a 1Password reference (`op://vault/item/field`). The app then
reads it with the 1Password CLI (`op`), so the secret never has to be in the
file. `config.toml`, `data/` and `logs/` are in `.gitignore`.

In folder mode the app holds the database open, so only one copy can use a
folder at a time. Use a server connection to share the database with other
programs.

## Using it from Claude Code

While the app is running, its MCP server listens on `127.0.0.1` only:

```sh
claude mcp add --transport http --scope user surrealdb-explorer http://127.0.0.1:8765/mcp
```

The tools are `get_schema`, `query`, `show`, `get_selection`, `select_record`,
`scroll_graph` and `overview`.

## Canton schema

The Canton view reads the node tables `transaction`, `created`, `exercised` and
`party`. It also reads `fetched`, `reassignment` and `topology_transaction` when
they are present. The relation tables are `action`, `consequence`, `target`,
`consumes` and `requested`. See the top of [`src/canton.rs`](src/canton.rs)
for the fields it uses. Any indexer that writes this shape will work.

## Tests

```sh
cargo test
```

`tests/server.rs` runs against a real SurrealDB server. It is skipped when the
`surreal` binary isn't on your `PATH`.

## License

This project is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

SurrealDB, which this app embeds, is under its own license (BSL 1.1 for the
database engine). See [surrealdb/surrealdb](https://github.com/surrealdb/surrealdb).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
