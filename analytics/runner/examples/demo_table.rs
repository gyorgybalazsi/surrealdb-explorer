//! Writes a contract-lifetimes Delta table to look at in the explorer's
//! Dashboard tab, without a ledger or a SurrealDB server: loads the explorer's
//! Canton sample, plus `--contracts` made-up contracts spread over a few
//! days, into an in-memory SurrealDB and runs the plugin over it.
//!
//! ```bash
//! (cd plugins/contract-lifetimes && cargo build --release --target wasm32-wasip2)
//! cargo run --example demo_table -- --table ../tables/contract_lifetimes --contracts 3000
//! ```

use std::path::PathBuf;

use anyhow::Result;
use canton_analytics::runner::Runner;
use canton_analytics::source::{self, Connection, UserLevel};
use clap::Parser;

const SAMPLE: &str = include_str!("../../../src/canton_sample.surql");

#[derive(Parser)]
struct Args {
    /// Folder of the Delta table to write (replaced if it exists).
    #[arg(long)]
    table: PathBuf,
    /// The plugin component.
    #[arg(long, default_value = "plugins/contract-lifetimes/target/wasm32-wasip2/release/contract_lifetimes.wasm")]
    plugin: PathBuf,
    /// Made-up contracts to add to the sample.
    #[arg(long, default_value_t = 2000)]
    contracts: u32,
}

/// A small deterministic generator, so the demo looks the same every time.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// `2026-11-03T00:00:00Z` plus `seconds`, as SurrealQL datetime text.
fn at(seconds: i64) -> String {
    let base = 1_793_664_000; // 2026-11-03T00:00:00Z
    let t = base + seconds;
    let days = t.div_euclid(86_400);
    let s = t.rem_euclid(86_400);
    // Days since the epoch to a date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", s / 3_600, s % 3_600 / 60, s % 60)
}

fn main() -> Result<()> {
    // Embedded SurrealDB needs more stack than tokio's default 2 MB.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(16 << 20)
        .build()?
        .block_on(async { tokio::spawn(run(Args::parse())).await? })
}

async fn run(args: Args) -> Result<()> {
    let db = source::connect(&Connection {
        url: "mem://".into(),
        namespace: "canton".into(),
        database: "event_graph".into(),
        username: String::new(),
        password: String::new(),
        user_level: UserLevel::Database,
    })
    .await?;
    db.query(SAMPLE).await?.check()?;

    // Each made-up contract is created at its own offset (after the sample's
    // 1..=5) and, mostly, archived later. Lifetimes depend on the template.
    let templates = [
        ("Main.Cash", 1_800, "Cash_Transfer"),
        ("Main.TicketOffer", 900, "TicketOffer_Accept"),
        ("Main.Ticket", 30_000, "Ticket_Redeem"),
        ("Main.Loan", 150_000, "Loan_Repay"),
        ("Main.Order", 6_000, "Order_Fill"),
    ];
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut events: Vec<(i64, String)> = Vec::new();
    let span = 4 * 86_400;
    for i in 0..args.contracts {
        let (template, typical, choice) = templates[rng.below(templates.len() as u64) as usize];
        let created = (rng.below(span) as i64) * (1 + i64::from(i) % 3) / 3;
        let life = (typical as f64 * (0.2 + 1.6 * (rng.below(1000) as f64 / 1000.0)).powi(2)) as i64 + 1;
        let id = format!("demo{i:05}");
        events.push((created, format!(
            "UPSERT created:⟨{id}⟩ MERGE {{ contract_id: '{id}', template_name: '{template}', offset: $o, created_at: d'{}' }};",
            at(created)
        )));
        if rng.below(10) < 8 {
            events.push((created + life, format!(
                "UPSERT exercised:[$o, 0] MERGE {{ offset: $o, node_id: 0, choice_name: '{choice}', target_contract_id: '{id}', consuming: true, transaction_effective_at: d'{}' }};",
                at(created + life)
            )));
        }
    }
    // Offsets follow time; one transaction per event.
    events.sort_by_key(|(t, _)| *t);
    let mut batch = String::new();
    for (k, (t, statement)) in events.iter().enumerate() {
        let offset = 6 + k as i64;
        batch.push_str(&format!(
            "UPSERT transaction:{offset} MERGE {{ offset: {offset}, effective_at: d'{}' }};\n{}\n",
            at(*t),
            statement.replace("$o", &offset.to_string())
        ));
        if k % 500 == 499 {
            db.query(std::mem::take(&mut batch)).await?.check()?;
        }
    }
    db.query(batch).await?.check()?;

    let _ = std::fs::remove_dir_all(&args.table);
    let mut runner = Runner::new(&args.plugin, &args.table, 1_000).await?;
    let steps = runner.catch_up(&db).await?;
    let rows: usize = steps.iter().map(|s| s.rows).sum();
    eprintln!("wrote {rows} rows in {} commits to {}", steps.len(), args.table.display());
    Ok(())
}
