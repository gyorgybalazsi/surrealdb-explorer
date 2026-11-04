//! Desktop viewer for a SurrealDB database as an interactive graph, built with iced.
//!
//! Run with `cargo run`. It keeps the encrypted `./data` database open while
//! running, so the examples can't run at the same time.

mod gui;

fn main() -> iced::Result {
    gui::run()
}
