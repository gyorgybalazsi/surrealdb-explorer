//! Loading and calling an analytics plugin: a WebAssembly component that
//! exports the `canton:analytics/plugin` interface (see `wit/analytics.wit`).
//!
//! The plugin runs in a sandbox: no preopened folders, no environment
//! variables, no network, a memory limit, and a fuel budget per call so an
//! endless loop stops instead of hanging the runner.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use deltalake::arrow::datatypes::SchemaRef;
use deltalake::arrow::ipc::reader::StreamReader;
use deltalake::arrow::record_batch::RecordBatch;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};

wasmtime::component::bindgen!({
    world: "analytics-plugin",
    path: "../wit",
});

use exports::canton::analytics::plugin::{Output, PluginInfo, QuerySource};

/// Most memory a plugin may use.
const MEMORY_LIMIT: usize = 1 << 30;
/// Fuel for one call, roughly one unit per WebAssembly instruction.
const FUEL_PER_CALL: u64 = 50_000_000_000;

struct Host {
    wasi: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for Host {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

/// A loaded plugin, ready to call.
pub struct Plugin {
    store: Store<Host>,
    bindings: AnalyticsPlugin,
    pub info: PluginInfo,
    pub source: QuerySource,
    pub schema: SchemaRef,
}

impl Plugin {
    pub fn load(path: &Path) -> Result<Plugin> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config)?;
        let component = Component::from_file(&engine, path)
            .map_err(|e| anyhow::Error::from(e).context(format!("could not load the plugin {}", path.display())))?;

        let mut linker = Linker::new(&engine);
        // The standard library of a wasm32-wasip2 program imports WASI; the
        // context below grants it nothing beyond stdout/stderr, clocks and
        // random numbers.
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        let host = Host {
            wasi: WasiCtx::builder().inherit_stderr().build(),
            table: ResourceTable::new(),
            limits: StoreLimitsBuilder::new().memory_size(MEMORY_LIMIT).build(),
        };
        let mut store = Store::new(&engine, host);
        store.limiter(|h| &mut h.limits);
        store.set_fuel(FUEL_PER_CALL)?;
        let bindings = AnalyticsPlugin::instantiate(&mut store, &component, &linker)?;

        let api = bindings.canton_analytics_plugin();
        let info = api.call_info(&mut store)?;
        let source = api.call_source(&mut store)?;
        let schema = read_ipc(&api.call_schema(&mut store)?)?.0;
        Ok(Plugin { store, bindings, info, source, schema })
    }

    /// Calls the plugin's `transform`. Returns its rows and new state.
    pub fn transform(&mut self, results: Vec<String>, state: Vec<u8>) -> Result<(Vec<RecordBatch>, Vec<u8>)> {
        self.store.set_fuel(FUEL_PER_CALL)?;
        let api = self.bindings.canton_analytics_plugin();
        let Output { rows, state } = api
            .call_transform(&mut self.store, &results, &state)?
            .map_err(|e| anyhow!("the plugin {} failed: {e}", self.info.name))?;
        let (schema, batches) = read_ipc(&rows)?;
        if schema.fields() != self.schema.fields() {
            return Err(anyhow!(
                "the plugin {} returned rows that don't match its schema:\n  got {schema:?}\n  expected {:?}",
                self.info.name,
                self.schema
            ));
        }
        Ok((batches, state))
    }
}

fn read_ipc(bytes: &[u8]) -> Result<(SchemaRef, Vec<RecordBatch>)> {
    let reader = StreamReader::try_new(bytes, None).context("the plugin returned bad Arrow IPC data")?;
    let schema = reader.schema();
    let batches = reader.collect::<Result<Vec<_>, _>>()?;
    Ok((schema, batches))
}
