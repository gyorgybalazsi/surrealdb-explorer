//! Helpers shared by the tests: a SurrealDB server to test against, a fresh
//! database on it, and a runtime to run async test code on.
//!
//! The tests need the `surreal` program (https://surrealdb.com/install),
//! either on the PATH or in `~/.surrealdb`. It is started as a temporary
//! in-memory server.

#![allow(dead_code)]

use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;

use tempfile::TempDir;

use surrealdb_explorer::config::DatabaseConfig;

pub const USERNAME: &str = "root";
pub const PASSWORD: &str = "secret";

/// A new, empty temporary folder. It is removed when the returned value is
/// dropped, which also happens when the test fails or panics.
pub fn temp_dir(name: &str) -> TempDir {
    tempfile::Builder::new()
        .prefix(&format!("surrealdb-explorer-test-{name}-"))
        .tempdir()
        .unwrap()
}

/// A temporary in-memory SurrealDB server, with root user `USERNAME` /
/// `PASSWORD`. It stops when this is dropped, and also when the test
/// process ends without dropping it (see `start_server`).
pub struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    pub port: u16,
}

impl Server {
    pub fn address(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// A root user's config for `database` in namespace `test`.
    pub fn config(&self, database: &str) -> DatabaseConfig {
        DatabaseConfig {
            server: self.address(),
            username: USERNAME.into(),
            password: PASSWORD.into(),
            namespace: "test".into(),
            database: database.into(),
            ..DatabaseConfig::default()
        }
    }

    /// Stops the server and waits for it to exit. Closing stdin makes the
    /// wrapper shell stop the server.
    fn stop(&mut self) {
        drop(self.stdin.take());
        let _ = self.child.wait();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

fn surreal_program() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    [format!("{home}/.surrealdb/surreal"), "surreal".to_string()]
        .into_iter()
        .find(|p| Command::new(p).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok())
        .expect(
            "the tests need the surreal program, on the PATH or in ~/.surrealdb: \
             see https://surrealdb.com/install",
        )
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// Starts a temporary in-memory server on a free port and waits until it
/// accepts connections.
///
/// The server runs under a small shell that stops it once its stdin closes,
/// and that itself exits if the server does. Our end of that pipe closes when
/// the test process ends, however it ends, so no server is left behind, even
/// for the shared one, which is never dropped.
///
/// The port is free when chosen but could be taken before the server binds
/// it; the server then exits, and another port is tried.
pub fn start_server() -> Server {
    let program = surreal_program();
    // A background job's stdin is /dev/null, so the watcher reads the
    // shell's stdin through fd 3.
    let script = r#"
        exec 3<&0
        "$0" db start --user "$1" --pass "$2" --bind "127.0.0.1:$3" memory >"$4" 2>&1 &
        server=$!
        (cat <&3 >/dev/null; kill $server 2>/dev/null) &
        wait $server
    "#;
    let mut output = String::new();
    for _attempt in 0..3 {
        let port = free_port();
        let log_dir = temp_dir("server");
        let log = log_dir.path().join("server.log");
        let mut child = Command::new("sh")
            .args(["-c", script, &program, USERNAME, PASSWORD, &port.to_string()])
            .arg(&log)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("could not start the surreal program");
        let stdin = child.stdin.take();
        let mut server = Server { child, stdin, port };
        for _ in 0..100 {
            if matches!(server.child.try_wait(), Ok(Some(_))) {
                break;
            }
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                // The log is only needed if the start fails. `log_dir` is
                // removed here, as the shared server is never dropped.
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        // Stop the server before its log folder is removed (when `log_dir`
        // goes out of scope), keeping its output for the message below.
        server.stop();
        output = std::fs::read_to_string(&log).unwrap_or_default();
    }
    panic!("the surreal server didn't start. Its output:\n{output}");
}

/// The server most tests share. Each test uses its own database on it.
pub fn shared_server() -> &'static Server {
    static SERVER: OnceLock<Server> = OnceLock::new();
    SERVER.get_or_init(start_server)
}

/// A fresh, empty database named `name` on the shared server. Give each test
/// its own name: the tests run in parallel.
///
/// The database is defined here, one test at a time. Left to the tests'
/// first writes, parallel tests would define their databases in the shared
/// namespace at the same moment, and SurrealDB can refuse one of those
/// writes with "Transaction conflict: Write conflict".
pub fn temp_database(name: &str) -> DatabaseConfig {
    static DEFINING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let config = shared_server().config(name);
    let _one_at_a_time = DEFINING.lock().unwrap_or_else(|e| e.into_inner());
    let (namespace, database) = (config.namespace.clone(), config.database.clone());
    let server = config.clone();
    run(async move {
        let db = surrealdb_explorer::db::connect(&server).await.unwrap();
        db.query(format!(
            "DEFINE NAMESPACE IF NOT EXISTS `{namespace}`; USE NS `{namespace}`; DEFINE DATABASE IF NOT EXISTS `{database}`;"
        ))
        .await
        .unwrap()
        .check()
        .unwrap();
    });
    config
}

/// Runs `future` to completion on a multi-threaded runtime, like the app's.
pub fn run<F>(future: F) -> F::Output
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(runtime.spawn(future)).unwrap()
}
