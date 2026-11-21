# Canton analytics (spike)

Analytics plugins written as WebAssembly components. A runner feeds them data
from SurrealDB, for example the tables the
[Canton syncer](https://github.com/gyorgybalazsi/canton-event-graph-syncer)
writes. It appends their output to Delta Lake tables, which Polars reads.

```
SurrealDB ──query($from, $to)──▶ runner ──JSON──▶ plugin.wasm ──Arrow IPC──▶ runner ──commit──▶ Delta table ──▶ Polars
                                   ▲                                                 (rows + cursor + state)
                                   └──────────── cursor and state, read back from the last commit ────────────┘
```

- **`wit/analytics.wit`**: the plugin interface. A plugin exports
  `info`, `schema`, `source` and `transform(results, state)`.
- **`plugins/contract-lifetimes`**: an example plugin. It writes one row per
  archived contract, with its template, its create and archive offsets and
  times, its lifetime, and the choice that archived it. Contracts that are
  still open are kept in the plugin's state from one step to the next.
- **`reader`**: reads a plugin's Delta table with Polars. Both the runner's
  `show` command and the explorer's **Dashboard** tab use it, so the explorer
  doesn't depend on wasmtime.
- **`runner`**: the `canton-analytics` program. It loads a plugin in
  wasmtime, steps through the database in offset ranges, and commits each
  step to Delta Lake. It also has a `show` command that reads a table with
  Polars.

## How a step works

1. The plugin's high-water query says how far the database has got. It uses
   `sync_state:stream.last_offset` from the syncer, or the highest
   transaction offset.
2. The plugin's query runs with `$from` (exclusive) and `$to` (inclusive),
   covering at most `--batch` offsets.
3. `transform` gets one JSON document per statement, plus its state from the
   last step. It returns Arrow rows and its new state.
4. The rows, the new cursor (as a Delta application transaction) and the
   state (in the commit info) go into **one** Delta commit. If the runner
   stops, it continues from the last commit, without losing or repeating a
   batch. Readers only ever see whole commits.

The plugin runs in a sandbox. It gets no folders, no environment variables
and no network, it has a 1 GB memory limit, and each call has a fuel budget,
so an endless loop stops with an error.

## Try it

```bash
rustup target add wasm32-wasip2
(cd plugins/contract-lifetimes && cargo build --release --target wasm32-wasip2)

# Against the syncer's server, as a read-only database user:
cargo run --release -p canton-analytics -- run \
  --plugin plugins/contract-lifetimes/target/wasm32-wasip2/release/contract_lifetimes.wasm \
  --table tables/contract_lifetimes \
  --url ws://127.0.0.1:8000 --namespace canton --database event_graph \
  --username viewer --password "$SURREAL_PASSWORD"      # add --once to stop when caught up

cargo run --release -p canton-analytics -- show --table tables/contract_lifetimes
cargo run --release -p canton-analytics -- show --table tables/contract_lifetimes --by-template
```

### In the explorer

The explorer's **Dashboard** tab reads the table in `[analytics] table` of
`config.toml` (default `tables/contract_lifetimes`), or whatever folder you
type into its box. It shows:
- totals
- archives over time (`iced_plot`)
- mean lifetime per template (`plotters-iced2`)
- the longest-lived contracts, in a table you can sort and resize
  (`iced_table2`)

The panes can be resized and dragged around. While the tab is open, it checks
the table's `_delta_log` every 2 seconds and reloads when the runner commits.

To try it without a ledger, write a demo table with the explorer's Canton
sample plus made-up contracts:

```bash
cargo run --example demo_table -- --table ../tables/contract_lifetimes --contracts 3000
```

The Delta table is a plain folder, so DuckDB, Spark or Python
(`polars.scan_delta`) can read it too.

`cargo test` builds the plugin and runs it over the explorer's Canton sample
in an in-memory SurrealDB. It runs in steps of 2 offsets, restarts the
runner, adds a new archive, and checks the results with Polars.

## Not done yet

- Compacting small files (`OPTIMIZE`) and vacuuming old ones. delta-rs needs
  its `datafusion` feature for that.
- The state is stored base64-encoded in each commit's info. That's fine for
  thousands of open contracts. For millions it should move to a checkpoint
  file next to the table.
- Changing a plugin's schema needs a new table for now.
- The Dashboard tab only knows the contract-lifetimes table so far.
