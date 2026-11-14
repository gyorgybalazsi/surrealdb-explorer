//! Connecting to a SurrealDB server, as one client among others. Starts a
//! temporary in-memory server with the `surreal` program; skipped (with a
//! note) if that program isn't installed.

mod common;

use std::process::{Child, Command, Stdio};

use surrealdb_explorer::config::{Connection, DatabaseConfig, UserLevel};
use surrealdb_explorer::{data, db, graph, query};

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn surreal_program() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    [format!("{home}/.surrealdb/surreal"), "surreal".to_string()]
        .into_iter()
        .find(|p| Command::new(p).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok())
}

fn start_server(port: u16) -> Option<Server> {
    let program = surreal_program()?;
    let child = Command::new(program)
        .args(["start", "--user", "root", "--pass", "secret", "--bind", &format!("127.0.0.1:{port}"), "memory"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Wait until it accepts connections.
    for _ in 0..50 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return Some(Server(child));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    None
}

#[test]
fn two_clients_share_a_server() {
    let port = 18_765;
    let Some(_server) = start_server(port) else {
        eprintln!("skipped: the surreal program isn't installed");
        return;
    };
    let config = DatabaseConfig {
        connection: Connection::Server,
        server: format!("ws://127.0.0.1:{port}"),
        username: "root".into(),
        password: "secret".into(),
        ..DatabaseConfig::default()
    };
    let wrong = DatabaseConfig { password: "wrong".into(), ..config.clone() };

    common::run(async move {
        assert!(db::connect(&wrong).await.is_err(), "a wrong password is refused");

        // One client writes, like a streaming app; another reads, like the explorer.
        let writer = db::connect(&config).await.unwrap();
        let reader = db::connect(&config).await.unwrap();
        data::load_sample_data(&writer).await.unwrap();

        let graph = graph::load(&reader, 1000).await.unwrap();
        assert_eq!(graph.nodes.len(), 6);
        assert_eq!(graph.edges.len(), 7);

        let outcome = query::run(&reader, "SELECT * FROM person:bob<-knows<-person;".into()).await;
        assert!(!query::record_ids(&outcome).is_empty());

        db::close(writer).await.unwrap();
        db::close(reader).await.unwrap();
    });
}

/// Users defined on a namespace or a database, like the Canton syncer's
/// (`DEFINE USER … ON DATABASE`), sign in to their own namespace or database.
#[test]
fn namespace_and_database_users_sign_in() {
    let port = 18_766;
    let Some(_server) = start_server(port) else {
        eprintln!("skipped: the surreal program isn't installed");
        return;
    };
    let root = DatabaseConfig {
        connection: Connection::Server,
        server: format!("ws://127.0.0.1:{port}"),
        username: "root".into(),
        password: "secret".into(),
        namespace: "canton".into(),
        database: "event_graph".into(),
        ..DatabaseConfig::default()
    };
    let viewer = DatabaseConfig {
        username: "viewer".into(),
        password: "viewer-pass".into(),
        user_level: UserLevel::Database,
        ..root.clone()
    };
    let ns_admin = DatabaseConfig {
        username: "ns_admin".into(),
        password: "ns-pass".into(),
        user_level: UserLevel::Namespace,
        ..root.clone()
    };

    common::run(async move {
        let admin = db::connect(&root).await.unwrap();
        admin
            .query(
                "DEFINE NAMESPACE canton; USE NS canton; \
                 DEFINE USER ns_admin ON NAMESPACE PASSWORD 'ns-pass' ROLES OWNER; \
                 DEFINE DATABASE event_graph; USE DB event_graph; \
                 DEFINE USER viewer ON DATABASE PASSWORD 'viewer-pass' ROLES VIEWER;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();
        data::load_sample_data(&admin).await.unwrap();

        // A database user reads its own database, but can't write with VIEWER.
        let reader = db::connect(&viewer).await.unwrap();
        let graph = graph::load(&reader, 1000).await.unwrap();
        assert_eq!(graph.nodes.len(), 6);
        let write = query::run(&reader, "CREATE person:eve SET name = 'Eve';".into()).await;
        let refused = match &write.statements {
            Ok(statements) => statements.iter().any(|s| s.result.is_err()),
            Err(_) => true,
        };
        assert!(refused, "a VIEWER can't create records");

        // Signed in as a root user, the same name and password are refused.
        let as_root = DatabaseConfig { user_level: UserLevel::Root, ..viewer.clone() };
        assert!(db::connect(&as_root).await.is_err());
        // A database user can't sign in to another database.
        let elsewhere = DatabaseConfig { database: "other".into(), ..viewer.clone() };
        assert!(db::connect(&elsewhere).await.is_err());

        // A namespace user reaches every database in its namespace.
        let ns = db::connect(&ns_admin).await.unwrap();
        assert_eq!(graph::load(&ns, 1000).await.unwrap().nodes.len(), 6);

        for c in [admin, reader, ns] {
            db::close(c).await.unwrap();
        }
    });
}
