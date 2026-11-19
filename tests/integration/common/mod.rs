//! Helpers shared by the tests: a SurrealDB server to test against, a fresh
//! database on it, and a runtime to run async test code on.
//!
//! The tests need the `surreal` program (https://surrealdb.com/install),
//! either on the PATH or in `~/.surrealdb`. It is started as a temporary
//! in-memory server.

#![allow(dead_code)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;

use surrealdb_explorer::config::DatabaseConfig;

pub const USERNAME: &str = "root";
pub const PASSWORD: &str = "secret";

pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("surrealdb-explorer-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
}

impl Drop for Server {
    fn drop(&mut self) {
        // Closing stdin makes the wrapper shell stop the server.
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.flush();
        }
        let _ = self.child.wait();
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
/// The server runs under a small shell that stops it once its stdin closes.
/// Our end of that pipe closes when the test process ends, however it ends,
/// so no server is left behind, even for the shared one, which is never
/// dropped.
pub fn start_server() -> Server {
    let program = surreal_program();
    let port = free_port();
    let script = r#"
        "$0" start --user "$1" --pass "$2" --bind "127.0.0.1:$3" memory >/dev/null 2>&1 &
        server=$!
        cat >/dev/null
        kill $server
        wait $server
    "#;
    let mut child = Command::new("sh")
        .args(["-c", script, &program, USERNAME, PASSWORD, &port.to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("could not start the surreal program");
    let stdin = child.stdin.take();
    let server = Server { child, stdin, port };
    for _ in 0..100 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return server;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(server);
    panic!("the surreal server on port {port} didn't start within 10 seconds");
}

/// The server most tests share. Each test uses its own database on it.
pub fn shared_server() -> &'static Server {
    static SERVER: OnceLock<Server> = OnceLock::new();
    SERVER.get_or_init(start_server)
}

/// A fresh, empty database named `name` on the shared server. Give each test
/// its own name: the tests run in parallel.
pub fn temp_database(name: &str) -> DatabaseConfig {
    shared_server().config(name)
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
