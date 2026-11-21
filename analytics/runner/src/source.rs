//! The SurrealDB side: connecting, and running a plugin's queries.

use anyhow::{Context, Result, anyhow};
use clap::{Args, ValueEnum};
use surrealdb::Surreal;
use surrealdb::engine::any::{self, Any};
use surrealdb::opt::auth::{Database as DatabaseUser, Namespace as NamespaceUser, Root};
use surrealdb::types::Value;

/// Where a server user is defined, as in the explorer's settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum UserLevel {
    Root,
    Namespace,
    Database,
}

#[derive(Debug, Clone, Args)]
pub struct Connection {
    /// SurrealDB address, e.g. ws://127.0.0.1:8000 (or mem:// for a test).
    #[arg(long, env = "SURREAL_URL", default_value = "ws://127.0.0.1:8000")]
    pub url: String,
    #[arg(long, env = "SURREAL_NAMESPACE")]
    pub namespace: String,
    #[arg(long, env = "SURREAL_DATABASE")]
    pub database: String,
    /// User to sign in as; a read-only VIEWER is enough. No sign-in if empty.
    #[arg(long, env = "SURREAL_USER", default_value = "")]
    pub username: String,
    #[arg(long, env = "SURREAL_PASSWORD", default_value = "", hide_env_values = true)]
    pub password: String,
    #[arg(long, value_enum, default_value = "database")]
    pub user_level: UserLevel,
}

pub type Db = Surreal<Any>;

pub async fn connect(c: &Connection) -> Result<Db> {
    let db = any::connect(c.url.as_str()).await.with_context(|| format!("could not connect to {}", c.url))?;
    if !c.username.is_empty() {
        let (username, password) = (c.username.clone(), c.password.clone());
        let signed_in = match c.user_level {
            UserLevel::Root => db.signin(Root { username, password }).await,
            UserLevel::Namespace => db.signin(NamespaceUser { namespace: c.namespace.clone(), username, password }).await,
            UserLevel::Database => {
                db.signin(DatabaseUser {
                    namespace: c.namespace.clone(),
                    database: c.database.clone(),
                    username,
                    password,
                })
                .await
            }
        };
        signed_in.with_context(|| format!("could not sign in as {}", c.username))?;
    }
    db.use_ns(&c.namespace).use_db(&c.database).await?;
    Ok(db)
}

/// Runs `high_water`, whose last statement returns one integer (or NONE
/// when there is no data).
pub async fn high_water(db: &Db, sql: &str) -> Result<i64> {
    let mut response = db.query(sql).await?.check()?;
    let last = response.num_statements().saturating_sub(1);
    let v: Value = response.take(last)?;
    match v.into_json_value() {
        serde_json::Value::Number(n) => n.as_i64().ok_or_else(|| anyhow!("the high-water query returned {n}")),
        serde_json::Value::Null => Ok(0),
        other => Err(anyhow!("the high-water query must return an integer, not {other}")),
    }
}

/// Runs `sql` with `$from` and `$to` bound; one JSON document per statement.
pub async fn run(db: &Db, sql: &str, from: i64, to: i64) -> Result<Vec<String>> {
    let mut response = db.query(sql).bind(("from", from)).bind(("to", to)).await?.check()?;
    let mut docs = Vec::new();
    for i in 0..response.num_statements() {
        let v: Value = response.take(i)?;
        docs.push(serde_json::to_string(&v.into_json_value())?);
    }
    Ok(docs)
}
