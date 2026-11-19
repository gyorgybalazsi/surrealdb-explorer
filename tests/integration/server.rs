//! Server features beyond a single root user: several clients at once, and
//! users defined on a namespace or a database. Each test starts its own
//! server, as they define users and namespaces of their own.

use crate::common;

use surrealdb_explorer::config::{DatabaseConfig, UserLevel};
use surrealdb_explorer::{data, db, graph, query};

#[test]
fn two_clients_share_a_server() {
    let server = common::start_server();
    let config = DatabaseConfig {
        server: server.address(),
        username: common::USERNAME.into(),
        password: common::PASSWORD.into(),
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
    let server = common::start_server();
    let root = DatabaseConfig {
        server: server.address(),
        username: common::USERNAME.into(),
        password: common::PASSWORD.into(),
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
                "DEFINE NAMESPACE IF NOT EXISTS canton; USE NS canton; \
                 DEFINE USER ns_admin ON NAMESPACE PASSWORD 'ns-pass' ROLES OWNER; \
                 DEFINE DATABASE IF NOT EXISTS event_graph; USE DB event_graph; \
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
        // SurrealDB refuses it either with an error or by creating nothing.
        query::run(&reader, "CREATE person:eve SET name = 'Eve';".into()).await;
        let mut res = admin.query("SELECT VALUE id FROM person:eve").await.unwrap();
        let created: Vec<surrealdb::types::Value> = res.take(0).unwrap();
        assert!(created.is_empty(), "a VIEWER can't create records");

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

/// A database user can't remove its own database, so erasing removes what's
/// in it instead, and keeps the user it signed in with.
#[test]
fn a_database_user_erases_its_database() {
    let server = common::start_server();
    let root = DatabaseConfig {
        server: server.address(),
        username: common::USERNAME.into(),
        password: common::PASSWORD.into(),
        namespace: "canton".into(),
        database: "event_graph".into(),
        ..DatabaseConfig::default()
    };
    let owner = DatabaseConfig {
        username: "syncer".into(),
        password: "syncer-pass".into(),
        user_level: UserLevel::Database,
        ..root.clone()
    };

    common::run(async move {
        let admin = db::connect(&root).await.unwrap();
        admin
            .query(
                "DEFINE NAMESPACE IF NOT EXISTS canton; USE NS canton; \
                 DEFINE DATABASE IF NOT EXISTS event_graph; USE DB event_graph; \
                 DEFINE USER syncer ON DATABASE PASSWORD 'syncer-pass' ROLES OWNER; \
                 DEFINE FUNCTION fn::canton::latest() { RETURN 1 }; DEFINE PARAM $limit VALUE 5;",
            )
            .await
            .unwrap()
            .check()
            .unwrap();

        let syncer = db::connect(&owner).await.unwrap();
        data::load_canton_sample(&syncer).await.unwrap();
        assert!(!graph::load(&syncer, 1000).await.unwrap().nodes.is_empty());

        data::erase(&syncer, &owner).await.unwrap();
        let info = query::run(&syncer, "INFO FOR DB".into()).await.statements.unwrap().remove(0).result.unwrap();
        for kind in ["tables", "functions", "params"] {
            assert_eq!(info[kind], serde_json::json!({}), "{kind} left: {info}");
        }
        assert!(info["users"].get("syncer").is_some(), "the user signing in stays: {info}");

        // It can still sign in and use the empty database.
        db::close(syncer).await.unwrap();
        let again = db::connect(&owner).await.unwrap();
        assert!(graph::load(&again, 1000).await.unwrap().nodes.is_empty());

        for c in [admin, again] {
            db::close(c).await.unwrap();
        }
    });
}


/// With a read-only viewer user set, the queries a person types or Claude
/// Code sends run as that user: the server keeps them from changing data.
#[test]
fn queries_run_as_the_read_only_viewer() {
    let mut config = common::temp_database("viewer_queries");
    common::run(async move {
        let main = db::connect(&config).await.unwrap();
        main.query("DEFINE USER viewer ON DATABASE PASSWORD 'viewer-pass' ROLES VIEWER;")
            .await
            .unwrap()
            .check()
            .unwrap();
        data::load_sample_data(&main).await.unwrap();

        // Without a viewer user, queries use the main connection.
        assert!(matches!(db::Queries::connect(&config).await, db::Queries::Main));

        config.viewer_username = "viewer".into();
        config.viewer_password = "wrong".into();
        let failed = db::Queries::connect(&config).await;
        assert!(failed.pick(&main).is_err(), "a viewer that can't sign in runs nothing, rather than the main user");

        config.viewer_password = "viewer-pass".into();
        let queries = db::Queries::connect(&config).await;
        assert!(queries.enforced());
        let viewer = queries.pick(&main).unwrap();

        let read = "SELECT VALUE id FROM person:alice;";
        let outcome = query::run(&viewer, read.into()).await;
        assert!(!query::record_ids(&outcome).is_empty(), "the viewer reads");

        // Writes change nothing. The server skips data changes without an
        // error and refuses definitions; the note says what happened.
        let write = "DELETE person:alice; CREATE person:eve; DEFINE TABLE secret;";
        let mut outcome = query::run(&viewer, write.into()).await;
        query::explain_refused_writes(&mut outcome, write);
        assert_eq!(outcome.note.as_deref(), Some(query::WRITES_IGNORED));
        let statements = outcome.statements.unwrap();
        let refused = statements[2].result.as_ref().unwrap_err();
        assert!(refused.starts_with("Refused: queries run as a read-only user"), "{refused}");

        let left = query::run(&main, "SELECT VALUE id FROM person:alice, person:eve; INFO FOR DB;".into()).await;
        let left = left.statements.unwrap();
        assert_eq!(left[0].result.as_ref().unwrap(), &serde_json::json!(["person:alice"]));
        assert!(left[1].result.as_ref().unwrap()["tables"].get("secret").is_none());

        // A read gets no note.
        let mut outcome = query::run(&viewer, read.into()).await;
        query::explain_refused_writes(&mut outcome, read);
        assert!(outcome.note.is_none());

        queries.close().await;
        db::close(main).await.unwrap();
    });
}
