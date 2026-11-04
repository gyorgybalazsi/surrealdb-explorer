//! The iced desktop app: window, sidebar, graph canvas and layout.

mod app;
mod canvas;
mod layout;
mod model;

use app::App;

/// Opens the window and runs until it is closed.
pub fn run() -> iced::Result {
    iced::application(App::boot, App::update, App::view)
        .title("SurrealDB graph")
        .subscription(App::subscription)
        .theme(App::theme)
        .window_size((1200.0, 800.0))
        .antialiasing(true)
        .run()
}
