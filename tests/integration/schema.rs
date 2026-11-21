//! The schema description Claude Code gets from the get_schema tool.

use crate::common;

use surrealdb_explorer::{data, db, schema};

#[test]
fn describes_tables_fields_and_what_relations_connect() {
    let config = common::temp_database("schema");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();
        assert_eq!(schema::describe(&db).await.unwrap(), "The database has no tables yet.");

        data::load_sample_data(&db).await.unwrap();
        let schema = schema::describe(&db).await.unwrap();
        assert!(schema.contains("- person: 6 records (drawn as nodes)"), "{schema}");
        assert!(schema.contains("- knows: relation table (7 records, drawn as edges)"), "{schema}");
        assert!(schema.contains("connects: person ->knows-> person"), "{schema}");
        assert!(schema.contains("fields: ") && schema.contains("name"), "{schema}");
        assert!(schema.contains("example: {"), "{schema}");
    });
}
