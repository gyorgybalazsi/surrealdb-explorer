//! Named queries the user saved, kept in `queries.toml` next to `config.toml`.
//!
//! The file is created on first run with the default queries below. After
//! that it is the user's: a default they delete stays deleted.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Where the saved queries are read from and saved to.
pub const QUERIES_FILE: &str = "queries.toml";

/// The ticket purchase demo (canton-event-graph-demo): each accepted offer,
/// with who bought the ticket and the price paid.
pub const TICKET_DEMO_NAME: &str = "Ticket purchase demo query";
pub const TICKET_DEMO_QUERY: &str = "\
SELECT
    offset,
    array::first(->consequence->created[WHERE template.entity_name = 'TicketAgreement'].create_arguments.owner) AS buyer,
    array::first(->target->created.create_arguments.price) AS price
FROM exercised
WHERE choice_name = 'Accept'
ORDER BY offset;";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub name: String,
    pub query: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQueries {
    #[serde(default, rename = "query")]
    pub queries: Vec<SavedQuery>,
}

impl Default for SavedQueries {
    /// The queries a new `queries.toml` starts with.
    fn default() -> Self {
        SavedQueries {
            queries: vec![SavedQuery { name: TICKET_DEMO_NAME.into(), query: TICKET_DEMO_QUERY.into() }],
        }
    }
}

impl SavedQueries {
    pub fn path() -> PathBuf {
        PathBuf::from(QUERIES_FILE)
    }

    /// Reads the file, or creates it with the default queries if it doesn't exist.
    pub fn load_or_create_at(path: &Path) -> Result<SavedQueries, Error> {
        if !path.exists() {
            let queries = SavedQueries::default();
            queries.save_to(path)?;
            return Ok(queries);
        }
        let text = std::fs::read_to_string(path)?;
        Ok(toml::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        let text = format!(
            "# surrealdb-explorer saved queries. Add, run and delete them in the app's query panel.\n\n{}",
            toml::to_string_pretty(self)?
        );
        std::fs::write(path, text)?;
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&SavedQuery> {
        self.queries.iter().find(|q| q.name == name)
    }

    pub fn names(&self) -> Vec<String> {
        self.queries.iter().map(|q| q.name.clone()).collect()
    }

    /// Saves `query` under `name`, replacing a query of the same name.
    /// Returns whether one was replaced.
    pub fn put(&mut self, name: &str, query: &str) -> Result<bool, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err("give the query a name first".into());
        }
        if query.trim().is_empty() {
            return Err("the query is empty".into());
        }
        if let Some(existing) = self.queries.iter_mut().find(|q| q.name == name) {
            existing.query = query.to_string();
            return Ok(true);
        }
        self.queries.push(SavedQuery { name: name.to_string(), query: query.to_string() });
        Ok(false)
    }

    /// Removes the query called `name`. Returns whether there was one.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.queries.len();
        self.queries.retain(|q| q.name != name);
        self.queries.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sdbx-saved-queries-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(QUERIES_FILE)
    }

    #[test]
    fn a_new_file_starts_with_the_ticket_demo_query() {
        let path = temp_file("new");
        let queries = SavedQueries::load_or_create_at(&path).unwrap();
        assert!(path.exists());
        assert_eq!(queries.names(), vec![TICKET_DEMO_NAME.to_string()]);
        assert_eq!(queries.get(TICKET_DEMO_NAME).unwrap().query, TICKET_DEMO_QUERY);
        // Read back from the file, the query text is unchanged.
        assert_eq!(SavedQueries::load_or_create_at(&path).unwrap(), queries);
    }

    #[test]
    fn saved_queries_are_added_replaced_and_deleted() {
        let path = temp_file("edit");
        let mut queries = SavedQueries::load_or_create_at(&path).unwrap();
        assert!(!queries.put(" People ", "SELECT * FROM person;").unwrap());
        assert!(queries.put("People", "SELECT name FROM person;").unwrap());
        assert!(queries.put("", "SELECT 1;").is_err());
        assert!(queries.put("Empty", "  ").is_err());
        assert_eq!(queries.get("People").unwrap().query, "SELECT name FROM person;");
        queries.save_to(&path).unwrap();
        assert_eq!(SavedQueries::load_or_create_at(&path).unwrap(), queries);
    }

    #[test]
    fn a_deleted_default_stays_deleted() {
        let path = temp_file("delete");
        let mut queries = SavedQueries::load_or_create_at(&path).unwrap();
        assert!(queries.remove(TICKET_DEMO_NAME));
        assert!(!queries.remove(TICKET_DEMO_NAME));
        queries.save_to(&path).unwrap();
        assert!(SavedQueries::load_or_create_at(&path).unwrap().queries.is_empty());
    }
}
