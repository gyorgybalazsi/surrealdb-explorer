# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- A memory watch: the app samples its own memory use every few seconds,
  logs it every five minutes, and warns in the log and the window when it
  grows by 1 GB within a minute or passes 2 GB. Above 8 GB it stops, so a
  leak can't take the computer down. Set in `[memory]` in `config.toml`.

### Fixed
- The Dashboard's x axis could make hundreds of thousands of ticks (one row
  on a day- or week-wide axis, or zooming out) and run out of memory (#44).

## [0.1.0] - 2026-11-23

The first release of SurrealDB Explorer: a desktop app (Rust, iced) for exploring
a SurrealDB database as an interactive graph, with a dedicated view for Canton
ledger event graphs and a built-in MCP server for Claude Code.

### Added

#### Graph and table views
- Graph view: records are nodes and relations are edges, in a force-directed
  layout, with hover, select, hide, focus and neighbourhood exploration.
- Table view, and a table overview that large databases open on first, so big
  graphs stay fast and readable.
- Query panel: run SurrealQL and see each query's result as its own graph or as
  rows. Path results show as rows. The result sits under the query, the panel is
  resizable, and a *Clear* button empties it.
- Saved queries in the query panel, kept in `queries.toml`. An unreadable
  `queries.toml` is kept rather than overwritten.
- Auto refresh: the graph redraws when another program changes the database,
  including a change made while the graph was loading.
- Loading the graph, the table overview, Canton paging, auto refresh and
  switching databases run in the background, so the window stays responsive
  while the database answers. Only the latest load's result is shown.

#### Canton event graph view
- A view for a SurrealDB copy of a Canton ledger: one column per transaction,
  with the event tree indented under it, `consumes` and `target` arrows, faded
  archived contracts, and an optional party column.
- Each relation has its own port, so arrows never overlap.
- Filters by node kind, party (on each event's witness parties), template,
  active or archived, offset range and text search. Filtering zooms to fit.
- Load the latest *n* transactions, show the offsets found, and follow the
  newest transactions as they arrive.
- Choose how many transactions sit side by side, and scroll.
- Created cards are titled with the full template name.
- An event links to its transaction when its parent is filtered out.

#### Connections and security
- Connects to a SurrealDB server (`ws://…`) as a root, namespace or database
  user.
- The query panel only reads. With a read-only viewer user set, queries run as
  that user and the server enforces it; without one, the app checks each query
  before running it.
- *Erase database*, also available to a database user.
- Settings apply straight away.

#### MCP server for Claude Code
- A built-in MCP server lets Claude Code (or any MCP client) describe the
  schema, run queries, draw results in the window, and read, set or scroll to
  the current selection.
- Queries that would change data run only after you click *Allow* in the
  window; the others run as the read-only viewer user, if one is set.
- The window says so when the MCP server can't start.

#### Sample data and packaging
- Settings → Data → *Load sample data* or *Load Canton sample*, plus a larger
  Canton demo in `demo/canton_demo.surql`.
- App icon (SurrealDB / Explorer in magenta) and `cargo bundle` metadata for a
  macOS `.app`.
- README, MIT OR Apache-2.0 dual license, security policy.

#### Development
- CI builds and tests every pull request, against the `surreal` 4.0.1 server
  that matches the `surrealdb` crate; Dependabot updates crates and GitHub
  Actions.
- Integration tests against throwaway in-memory SurrealDB servers, built as one
  test binary, and the first UI tests with `iced_test` (including one that runs
  the app headless).

[Unreleased]: https://github.com/gyorgybalazsi/surrealdb-explorer/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/gyorgybalazsi/surrealdb-explorer/releases/tag/v0.1.0
