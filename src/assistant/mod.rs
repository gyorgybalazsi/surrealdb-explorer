//! Plain-English questions, answered by Claude with SurrealQL.
//!
//! In Auto mode Haiku answers first. Its query is checked against the
//! database (without changing anything); if the check fails, or Haiku says
//! it isn't sure, Opus is asked, with Haiku's attempt and the error.

mod check;
mod claude;
pub mod key;
mod schema;

pub use check::check;
pub use claude::Model;
pub use schema::describe;

use crate::Error;
use crate::config::AssistantMode;
use crate::db::Database;

/// A query Claude suggested, ready to put in the editor.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub question: String,
    pub query: String,
    pub explanation: String,
    /// The model that wrote this query.
    pub model: Model,
    pub confident: bool,
    /// What SurrealDB said is wrong with the query, if anything.
    pub problem: Option<String>,
    /// Why Opus was asked after Haiku, and anything else worth telling the
    /// user about how the answer came about.
    pub notes: Vec<String>,
}

/// Asks Claude. Cheap to clone. Holds the API key, which `Debug` hides.
#[derive(Clone)]
pub struct Assistant {
    http: reqwest::Client,
    key: String,
}

/// An earlier attempt Opus should improve on.
struct Previous<'a> {
    query: &'a str,
    /// What was wrong with it.
    issue: String,
}

impl std::fmt::Debug for Assistant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Assistant { key: <hidden> }")
    }
}

impl Assistant {
    pub fn new(key: String) -> Assistant {
        Assistant { http: reqwest::Client::new(), key }
    }

    pub async fn ask(&self, db: &Database, question: &str, mode: AssistantMode) -> Result<Suggestion, Error> {
        let schema = describe(db).await?;
        match mode {
            AssistantMode::Haiku => self.attempt(db, Model::Haiku, question, &schema, None).await,
            AssistantMode::Opus => self.attempt(db, Model::Opus, question, &schema, None).await,
            AssistantMode::Auto => {
                let first = self.attempt(db, Model::Haiku, question, &schema, None).await?;
                let (reason, issue) = match (&first.problem, first.confident) {
                    (Some(problem), _) => (
                        "Haiku's query failed the check".to_string(),
                        format!("SurrealDB rejected it with: {problem}"),
                    ),
                    (None, false) => (
                        "Haiku wasn't sure".to_string(),
                        "The first attempt said it wasn't confident that this is right.".to_string(),
                    ),
                    (None, true) => return Ok(first),
                };
                tracing::info!(%reason, "asking Opus");
                let previous = Previous { query: &first.query, issue };
                match self.attempt(db, Model::Opus, question, &schema, Some(previous)).await {
                    Ok(mut better) => {
                        better.notes.insert(0, reason);
                        Ok(better)
                    }
                    Err(e) => {
                        let mut first = first;
                        first.notes.push(format!("{reason}, but asking Opus failed: {e}"));
                        Ok(first)
                    }
                }
            }
        }
    }

    /// Asks Opus to redo an answer the user wasn't happy with.
    pub async fn retry_with_opus(&self, db: &Database, previous: &Suggestion) -> Result<Suggestion, Error> {
        let schema = describe(db).await?;
        let issue = match &previous.problem {
            Some(problem) => format!("SurrealDB rejected it with: {problem}"),
            None => "The user says its result isn't what they asked for.".to_string(),
        };
        let mut better = self
            .attempt(db, Model::Opus, &previous.question, &schema, Some(Previous { query: &previous.query, issue }))
            .await?;
        better.notes.insert(0, "asked by you".into());
        Ok(better)
    }

    async fn attempt(
        &self,
        db: &Database,
        model: Model,
        question: &str,
        schema: &str,
        previous: Option<Previous<'_>>,
    ) -> Result<Suggestion, Error> {
        let started = std::time::Instant::now();
        let user = user_message(question, schema, previous.as_ref());
        let reply = claude::ask(&self.http, &self.key, model, SYSTEM_PROMPT, &user).await?;
        let answer = reply.answer;
        let problem = check(db, &answer.query).await;
        tracing::info!(
            model = model.name(),
            served_by = %reply.served_by,
            input_tokens = reply.input_tokens,
            output_tokens = reply.output_tokens,
            confident = answer.confident,
            check_failed = problem.is_some(),
            elapsed_ms = started.elapsed().as_millis() as u64,
            "Claude answered"
        );
        tracing::debug!(%question, query = %answer.query, ?problem, "suggested query");

        let mut notes = Vec::new();
        if !reply.served_by.is_empty() && reply.served_by != model.id() {
            notes.push(format!("answered by {} instead", reply.served_by));
        }
        Ok(Suggestion {
            question: question.to_string(),
            query: answer.query.trim().to_string(),
            explanation: answer.explanation,
            model,
            confident: answer.confident,
            problem,
            notes,
        })
    }
}

const SYSTEM_PROMPT: &str = "\
You turn questions about a SurrealDB database into SurrealQL (SurrealDB 3/4 syntax) \
for a graph explorer app.

The app draws ordinary records as nodes, and records of relation tables (made with \
RELATE, with `in` and `out` fields) as edges. The user reads your query in an editor \
before running it, and can then highlight the returned records in the graph, so \
return whole records or include their `id` where that makes sense.

Rules:
- Use only the tables and fields in the schema. Record ids look like `table:key`, \
for example `person:alice`.
- Follow relations with arrows: `->knows->person` (outgoing), `<-knows<-person` \
(incoming), `<->knows<->person` (either direction).
- Selecting from a table that doesn't exist is an error in this SurrealDB version.
- Prefer read-only queries. Write statements that change data only when the question \
clearly asks for a change, and say so in the explanation.
- Don't use transactions (BEGIN, COMMIT, CANCEL).
- Several statements are fine, separated by semicolons.
- explanation: one or two short sentences in plain language.
- confident: false if the question is ambiguous, needs data the schema doesn't show, \
or you aren't sure the syntax is right.";

fn user_message(question: &str, schema: &str, previous: Option<&Previous<'_>>) -> String {
    let mut message = format!("Database schema:\n{schema}\nQuestion: {question}\n");
    if let Some(previous) = previous {
        message.push_str(&format!(
            "\nAn earlier attempt wrote this query:\n```\n{}\n```\n{}\nWrite a better query.\n",
            previous.query, previous.issue
        ));
    }
    message
}
