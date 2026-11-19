//! Loading the sample data and erasing the database.

use crate::common;

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
