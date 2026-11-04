//! Native desktop viewer for the SurrealDB graph, built with iced.
//!
//! Run with `cargo run --features gui --bin graph-gui`.
//! Like the web server, it keeps the `./data` database open while running,
//! so only one of them can run at a time.

mod app;
mod canvas;
mod layout;
mod model;

use app::App;

fn main() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("SurrealDB graph")
        .subscription(App::subscription)
        .theme(App::theme)
        .window_size((1200.0, 800.0))
        .antialiasing(true)
        .run()
}
