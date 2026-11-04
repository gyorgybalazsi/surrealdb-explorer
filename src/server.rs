//! The web server: serves the page and the graph data.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Json};
use axum::routing::get;

use crate::db::Database;
use crate::graph;

const ADDRESS: &str = "127.0.0.1:3000";

/// Serves the graph page until the program is stopped, and opens it in the browser.
pub async fn serve(db: Database) -> Result<(), Box<dyn std::error::Error>> {
    let app = Router::new()
        .route("/", get(index))
        .route("/style.css", get(style))
        .route("/app.js", get(script))
        .route("/api/graph", get(graph_data))
        .with_state(Arc::new(db));

    let listener = tokio::net::TcpListener::bind(ADDRESS).await?;
    let url = format!("http://{ADDRESS}");
    println!("Graph is at {url} (press Ctrl+C to stop)");
    // Open the page in the default browser (macOS).
    let _ = std::process::Command::new("open").arg(&url).spawn();
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<&'static str> {
    Html(include_str!("web/index.html"))
}

async fn style() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css")], include_str!("web/style.css"))
}

async fn script() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript")],
        include_str!("web/app.js"),
    )
}

/// The current graph as JSON. Read fresh on every request,
/// so reloading the page shows the latest database contents.
async fn graph_data(State(db): State<Arc<Database>>) -> impl IntoResponse {
    match graph::load(&db).await {
        Ok(data) => Json(data).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}
