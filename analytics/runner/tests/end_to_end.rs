//! The contract-lifetimes plugin over the explorer's Canton sample, in an
//! in-memory SurrealDB: runs in small steps, resumes after a restart, and
//! reads the Delta table back with Polars.

use std::path::{Path, PathBuf};
use std::process::Command;

use canton_analytics::analysis;
use canton_analytics::runner::{Runner, Step};
use canton_analytics::source::{self, Connection, UserLevel};
use polars::prelude::*;

const SAMPLE: &str = include_str!("../../../src/canton_sample.surql");

/// Builds the plugin for wasm32-wasip2 and returns its path.
fn build_plugin() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugins/contract-lifetimes");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--release", "--target", "wasm32-wasip2"])
        .current_dir(&dir)
        .status()
        .expect("cargo runs");
    assert!(status.success(), "building the plugin failed; is the wasm32-wasip2 target installed?");
    dir.join("target/wasm32-wasip2/release/contract_lifetimes.wasm")
}

fn strings(frame: &DataFrame, column: &str) -> Vec<String> {
    let c = frame.column(column).unwrap().str().unwrap().clone();
    (0..c.len()).map(|i| c.get(i).unwrap_or_default().to_string()).collect()
}

fn temp_table(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("canton-analytics-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Embedded SurrealDB needs more stack than test threads have.
fn run<F: std::future::Future<Output = ()> + Send + 'static>(f: F) {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().thread_stack_size(16 << 20).build().unwrap();
    rt.block_on(async { tokio::spawn(f).await.unwrap() });
}

fn memory_db() -> Connection {
    Connection {
        url: "mem://".into(),
        namespace: "canton".into(),
        database: "event_graph".into(),
        username: String::new(),
        password: String::new(),
        user_level: UserLevel::Database,
    }
}

#[test]
fn lifetimes_are_appended_in_steps_resumed_and_read_with_polars() {
    let plugin = build_plugin();
    let table = temp_table("lifetimes");
    run(async move {
        let db = source::connect(&memory_db()).await.unwrap();
        db.query(SAMPLE).await.unwrap().check().unwrap();

        // Offsets 1..=5 in steps of 2; the archives at offset 3 and 4 need
        // creates seen in earlier steps, which the state carries over.
        let mut runner = Runner::new(&plugin, &table, 2).await.unwrap();
        assert_eq!(runner.plugin.info.name, "contract-lifetimes");
        let steps = runner.catch_up(&db).await.unwrap();
        assert_eq!(
            steps,
            vec![
                Step { from: 0, to: 2, rows: 0 },
                Step { from: 2, to: 4, rows: 3 },
                Step { from: 4, to: 5, rows: 0 },
            ]
        );
        assert_eq!(runner.step(&db).await.unwrap(), None, "nothing new");
        drop(runner);

        // A new runner (as after a restart) resumes from the last commit,
        // with the contracts still open. Carol's ticket is now archived.
        db.query(
            "UPSERT transaction:6 MERGE { offset: 6, effective_at: '2026-11-02T22:00:00Z' };
             UPSERT exercised:[6, 0] MERGE { offset: 6, node_id: 0, choice_name: 'Ticket_Redeem',
                target_contract_id: '00c5e9f6', consuming: true, transaction_effective_at: '2026-11-02T22:00:00Z' };",
        )
        .await
        .unwrap()
        .check()
        .unwrap();
        let mut runner = Runner::new(&plugin, &table, 2).await.unwrap();
        assert_eq!(runner.progress.cursor, 5);
        assert_eq!(runner.catch_up(&db).await.unwrap(), vec![Step { from: 5, to: 6, rows: 1 }]);

        // Polars reads the table's current version.
        let frame = analysis::scan(&table)
            .await
            .unwrap()
            .sort(["contract_id"], SortMultipleOptions::default())
            .collect()
            .unwrap();
        assert_eq!(frame.height(), 4);
        let ids = strings(&frame, "contract_id");
        assert_eq!(ids, ["00c1a5b2", "00c2b6c3", "00c4d8e5", "00c5e9f6"]);
        let seconds: Vec<Option<f64>> = {
            let c = frame.column("lifetime_seconds").unwrap().f64().unwrap().clone();
            (0..c.len()).map(|i| c.get(i)).collect()
        };
        // Cash 09:00 -> 09:12, offer 09:05 -> 09:12, Bob's ticket 09:12 -> 10:30,
        // Carol's ticket 10:30 -> 22:00.
        assert_eq!(seconds, [Some(720.0), Some(420.0), Some(4680.0), Some(41400.0)]);
        let created: Vec<Option<i64>> = {
            let c = frame.column("created_offset").unwrap().i64().unwrap().clone();
            (0..c.len()).map(|i| c.get(i)).collect()
        };
        assert_eq!(created, [Some(1), Some(2), Some(3), Some(4)]);

        let summary = analysis::lifetimes_by_template(analysis::scan(&table).await.unwrap()).collect().unwrap();
        let templates = strings(&summary, "template_name");
        assert_eq!(templates[0], "Main.Ticket", "two tickets were archived");
        println!("{frame}\n{summary}");
    });
}
