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
  its own graph or as rows. The query panel only reads: see
  [Read-only queries](#read-only-queries).
- **Canton event graph view.** This view is for a SurrealDB copy of a Canton
  ledger: one column per transaction, with the event tree indented under it. It
  draws `consumes` and `target` arrows, fades archived contracts, and can show
  parties in their own column. You can filter by node kind, party (each party's
  projection), template, active or archived, offset range and text search. The
  view can page through the latest *n* transactions and follow new ones as they
  arrive.
- **Connects to a SurrealDB server** (`ws://…`) as a root, namespace or
  database user, as one client among others.
- **Auto refresh.** The graph redraws when another program changes the database.
- **MCP server for Claude Code.** It lets Claude describe the schema, run
  queries, draw results in the window, and read, set or scroll to the current
  selection. Queries that would change data run only after you click Allow in
  the window; the others run as the read-only viewer user, if one is set.
- **Analytics dashboard** (optional, see [Dashboard](#dashboard)). Charts
  and tables over the Delta Lake tables that the canton-analytics daemon
  writes, one dashboard per plugin.
- **Sample data.** Settings → Data → *Load sample data* or *Load Canton sample*.
  There is a larger Canton demo in [`demo/canton_demo.surql`](demo/canton_demo.surql).

## Build and run

You need a recent stable Rust toolchain (edition 2024) and a running SurrealDB
server. To try the app, [install SurrealDB](https://surrealdb.com/install) and
start an in-memory server:

```sh
surreal db start --user root --pass secret memory
```

Then build and run the app:

```sh
cargo run --release
```

The first run creates `config.toml` in the current folder and connects to
`ws://127.0.0.1:8000` as `root`. Set the password (`secret` above) in
**Settings** (Settings → Data has the sample data). Every setting can be
changed there or in the file. Logs go to the terminal and to `logs/`.

To build a macOS `.app` with the icon, use
[cargo-bundle](https://github.com/burtonageo/cargo-bundle):

```sh
cargo bundle --release
```

## Configuration

`config.toml` has these sections:

| Section      | What it sets |
|--------------|--------------|
| `[database]` | `server`, `username`, `user_level` (`root`, `namespace` or `database`), `password`, `namespace`, `database`, and the read-only `viewer_username`, `viewer_user_level`, `viewer_password` |
| `[graph]`    | Record limits, auto refresh, the visualization (`standard` or `canton`) and Canton view options |
| `[logging]`  | Log level and folder |
| `[mcp]`      | `enabled` and `port` (default 8765) |
| `[analytics]` | The Dashboard: `location` of the tables, `storage_options` and the `selected` table |

`password` and `viewer_password` can be a 1Password reference (`op://vault/item/field`). The app then
reads it with the 1Password CLI (`op`), so the secret never has to be in the
file. `config.toml` and `logs/` are in `.gitignore`.

## Dashboard

The [canton-analytics daemon](https://github.com/gyorgybalazsi/canton-event-graph-syncer/tree/main/analytics)
runs WebAssembly analytics plugins over the syncer's SurrealDB and writes
Delta Lake tables. The Dashboard reads them; it never writes, and the
daemon doesn't need the explorer to be open.

It is an opt-in feature, because it pulls in DataFusion:

```sh
cargo run --release --features dashboard            # tables in a local folder
cargo run --release --features dashboard-s3         # …or in S3 (also -azure, -gcs)
```

Click **Dashboard** next to Settings. Type where the tables are (the
daemon's `--tables`: a folder, or `s3://`, `az://` or `gs://`) and press
Enter, then pick a table. Each table shows its plugin's panels: number
cards, time series, bar charts and sortable tables. A table without panels
gets some made from its columns. A line under the header shows the daemon's
heartbeat: up to date, catching up, stopped, not heard from, or failing.
The dashboard checks for new commits every two seconds while it is open;
**Refresh now** looks again at once, and also finds new tables.

Cloud credentials come from the usual environment variables
(`AWS_REGION`, `AWS_ACCESS_KEY_ID`, `AZURE_STORAGE_ACCOUNT_NAME`,
`GOOGLE_APPLICATION_CREDENTIALS`…). Other object store options go in
`config.toml`:

```toml
[analytics]
location = "s3://my-bucket/canton-analytics"
storage_options = ["aws_endpoint=http://localhost:9000", "aws_allow_http=true"]
```

## Queries

**▸ Query** under the graph opens the query panel. Type SurrealQL and press
**Run** (⌘↩). The result shows under the query: rows of fields as a table (long
Canton party ids shortened; hover a value to see it whole), paths as paths, and
anything else as JSON. Records in the result are also drawn in the graph above.
The editor grows to fit the query, and dragging the line between the graph and
the panel resizes the panel.

### Saved queries

The panel keeps named queries in `queries.toml` in the current folder. Pick one
from **Choose a saved query…** to put it in the editor, then **Run** it. Type a name
and press **Save query** to save the editor's query (an existing name is
replaced), and **Delete** removes the picked one.

A new `queries.toml` starts with the *Ticket purchase demo query*, which lists
each ticket bought in the
[canton-event-graph-demo](https://github.com/gyorgybalazsi/canton-event-graph-demo)
ticket purchase flow with its buyer and price. It can be deleted like any other
query and doesn't come back.

### Read-only queries

The queries you type in the query panel, and the ones Claude Code sends, only
read. The app keeps them from changing data in one of two ways, depending on
whether you set a read-only viewer user.

**Without a viewer user (the default): the app checks.** Queries run as the main
user, which can write. Before sending a query, the app looks for words that
change data (`CREATE`, `UPDATE`, `DELETE`, `RELATE`, `DEFINE`, `REMOVE`, a custom
`fn::…`, …) and doesn't run a query that has one. This needs no setup, but it
reads words, not meaning: it is a best-effort guard, and it can also stop a
harmless read, for example one that compares a field to `'create'`.

**With a viewer user: the server checks.** Define a user with the `VIEWER` role
and set it under **Settings → Read-only queries** (or `viewer_username` and
`viewer_password` in `config.toml`):

```sql
DEFINE USER viewer ON DATABASE PASSWORD 'viewer-secret' ROLES VIEWER;
```

Those queries then run as that user, on a second connection, and SurrealDB
itself refuses to change anything, however the query is written. It refuses
`DEFINE` and `REMOVE` with an error, but skips `CREATE`, `UPDATE`, `DELETE` and
`RELATE` silently (they return no records), so the query panel adds a note
that nothing was changed. If the viewer can't sign in, those queries don't run
at all. They never fall back to the main user.

Either way, the main user (`username`) still draws the graph, runs Settings →
Data (*Load sample data*, *Erase database*), and runs a write Claude Code asks
for after you click Allow.

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

The tests start temporary in-memory SurrealDB servers, so they need the
`surreal` program, on your `PATH` or in `~/.surrealdb`.

## License

This project is licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

The `surrealdb` client crate this app uses is under its own license (Business
Source License 1.1). See [surrealdb/surrealdb](https://github.com/surrealdb/surrealdb).

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in this work by you, as defined in the Apache-2.0 license, shall
be dual licensed as above, without any additional terms or conditions.
