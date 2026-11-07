//! Loading the sample data and erasing the database.

mod common;

use surrealdb_explorer::{data, db, graph};

#[test]
fn sample_data_loads_repeatably_and_erase_empties_the_database() {
    let config = common::temp_database("data");
    common::run(async move {
        let db = db::connect(&config).await.unwrap();

        data::load_sample_data(&db).await.unwrap();
        let g = graph::load(&db, 1000).await.unwrap();
        assert_eq!((g.nodes.len(), g.edges.len()), (6, 7));

        // Loading again doesn't duplicate anything.
        data::load_sample_data(&db).await.unwrap();
        let g = graph::load(&db, 1000).await.unwrap();
        assert_eq!((g.nodes.len(), g.edges.len()), (6, 7));

        data::erase(&db, &config).await.unwrap();
        let g = graph::load(&db, 1000).await.unwrap();
        assert_eq!((g.nodes.len(), g.edges.len()), (0, 0));

        // The erased database is usable straight away.
        data::load_sample_data(&db).await.unwrap();
        let g = graph::load(&db, 1000).await.unwrap();
        assert_eq!((g.nodes.len(), g.edges.len()), (6, 7));
    });
}

#[test]
fn a_closed_database_folder_can_be_reopened_in_the_same_process() {
    // What the Settings view does when switching to another folder and back.
    let first = common::temp_database("switch-a");
    let second = common::temp_database("switch-b");
    common::run(async move {
        let db = db::connect(&first).await.unwrap();
        data::load_sample_data(&db).await.unwrap();

        let other = db::connect(&second).await.unwrap();
        db::close(db).await.unwrap();
        let g = graph::load(&other, 1000).await.unwrap();
        assert_eq!(g.nodes.len(), 0);
        db::close(other).await.unwrap();

        let again = db::connect(&first).await.expect("the first folder opens again");
        let g = graph::load(&again, 1000).await.unwrap();
        assert_eq!(g.nodes.len(), 6);
    });
}
