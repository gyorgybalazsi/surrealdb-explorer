//! Drawing a query's result: exactly what it returned, read by id.

use crate::common;

use surrealdb_explorer::{data, db, graph, query};

#[test]
fn a_result_graph_holds_what_the_queries_returned() {
    let config = common::temp_database("result-graph");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        data::load_sample_data(&db).await.unwrap();

        // Who knows someone who knows bob, and the relations on the way,
        // both written to start from bob.
        let answer = query::run(&db, "SELECT * FROM person:bob<-knows<-person<-knows<-person;".into()).await;
        let context = query::run(
            &db,
            "RETURN array::flatten([person:bob<-knows, person:bob<-knows<-person<-knows]);".into(),
        )
        .await;
        let answer_ids = query::record_ids(&answer);
        let context_ids = query::record_ids(&context);
        assert!(!answer_ids.is_empty(), "{answer:?}");
        assert!(context_ids.iter().all(|id| id.starts_with("knows:")), "{context_ids:?}");

        let result = graph::load_result(&db, &answer_ids, &context_ids, 100, false).await.unwrap();
        // Every answer is drawn, and so is bob, at the end of the relations.
        assert_eq!(result.answer.len(), answer_ids.iter().collect::<std::collections::HashSet<_>>().len());
        assert!(result.data.nodes.iter().any(|n| n.id == "person:bob"));
        assert_eq!(result.data.edges.len(), context_ids.len());
        assert_eq!(result.left_out, 0);

        // Text that only looks like an id is ignored.
        let odd = vec!["https://example.com".to_string(), "12:30".to_string(), "person:nobody".to_string()];
        let nothing = graph::load_result(&db, &odd, &[], 100, false).await.unwrap();
        assert!(nothing.data.nodes.is_empty());

        // The limit: answers first, the rest counted as left out.
        let everyone: Vec<String> = query::record_ids(&query::run(&db, "SELECT VALUE id FROM person;".into()).await);
        let limited = graph::load_result(&db, &everyone, &[], 2, false).await.unwrap();
        assert_eq!(limited.data.nodes.len(), 2);
        assert_eq!(limited.left_out, everyone.len() - 2);

        // Connecting adds the relations between the drawn records.
        let unconnected = graph::load_result(&db, &everyone, &[], 100, false).await.unwrap();
        assert!(unconnected.data.edges.is_empty());
        let connected = graph::load_result(&db, &everyone, &[], 100, true).await.unwrap();
        assert_eq!(connected.data.edges.len(), 7, "all knows relations of the sample data");
    });
}
