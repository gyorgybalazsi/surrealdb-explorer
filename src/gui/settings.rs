//! The settings view: edits `config.toml`.

use iced::widget::{Column, button, column, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill};
use crate::config::{
    AnalyticsConfig, CONFIG_FILE, Config, DatabaseConfig, GraphConfig, UserLevel,
    LOG_LEVELS, LoggingConfig, McpConfig, MemoryConfig, Visualization,
};

use crate::gui::app::{DataAction, Message};
use crate::gui::sidebar::{ERROR, MUTED, SIDEBAR_WIDTH, SUCCESS, heading};

/// A text field of the settings form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    Server,
    Username,
    Password,
    Namespace,
    Database,
    ViewerUsername,
    ViewerPassword,
    MaxRecords,
    MaxResultRecords,
    RefreshSeconds,
    OverviewAbove,
    CantonTransactions,
    LogLevel,
    LogDirectory,
}

/// The settings as edited, kept as text until saved.
pub struct SettingsForm {
    pub server: String,
    pub username: String,
    pub user_level: UserLevel,
    /// The password or a 1Password reference; shown hidden.
    pub password: String,
    pub namespace: String,
    pub database: String,
    pub viewer_username: String,
    pub viewer_user_level: UserLevel,
    /// The viewer's password or a 1Password reference; shown hidden.
    pub viewer_password: String,
    pub max_records: String,
    pub max_result_records: String,
    /// Changed with the checkbox in the query panel; kept so saving doesn't reset it.
    pub connect_results: bool,
    pub log_level: String,
    pub log_directory: String,
    /// Not edited in the form; kept so saving doesn't reset it.
    pub mcp: McpConfig,
    /// Not edited in the form (the Dashboard has the folder box).
    pub analytics: AnalyticsConfig,
    /// Not edited in the form; kept so saving doesn't reset it.
    pub memory: MemoryConfig,
    pub refresh_seconds: String,
    pub overview_above: String,
    pub canton_transactions: String,
    /// Not edited here (the sidebar has the picker and the checkboxes).
    pub visualization: Visualization,
    pub auto_refresh: bool,
    pub fit_on_filter: bool,
    /// Result of the last save: a confirmation or an error.
    pub message: Option<Result<String, String>>,
}

impl SettingsForm {
    pub fn from_config(c: &Config) -> Self {
        SettingsForm {
            server: c.database.server.clone(),
            username: c.database.username.clone(),
            user_level: c.database.user_level,
            password: c.database.password.clone(),
            namespace: c.database.namespace.clone(),
            database: c.database.database.clone(),
            viewer_username: c.database.viewer_username.clone(),
            viewer_user_level: c.database.viewer_user_level,
            viewer_password: c.database.viewer_password.clone(),
            max_records: c.graph.max_records_per_table.to_string(),
            max_result_records: c.graph.max_result_records.to_string(),
            connect_results: c.graph.connect_results,
            log_level: c.logging.level.clone(),
            log_directory: c.logging.directory.clone(),
            mcp: c.mcp.clone(),
            analytics: c.analytics.clone(),
            memory: c.memory.clone(),
            visualization: c.graph.visualization,
            auto_refresh: c.graph.auto_refresh,
            refresh_seconds: c.graph.refresh_seconds.to_string(),
            overview_above: c.graph.overview_above.to_string(),
            canton_transactions: c.graph.canton_transactions.to_string(),
            fit_on_filter: c.graph.fit_on_filter,
            message: None,
        }
    }

    pub fn set(&mut self, field: SettingsField, value: String) {
        let target = match field {
            SettingsField::Server => &mut self.server,
            SettingsField::Username => &mut self.username,
            SettingsField::Password => &mut self.password,
            SettingsField::Namespace => &mut self.namespace,
            SettingsField::Database => &mut self.database,
            SettingsField::ViewerUsername => &mut self.viewer_username,
            SettingsField::ViewerPassword => &mut self.viewer_password,
            SettingsField::MaxRecords => &mut self.max_records,
            SettingsField::MaxResultRecords => &mut self.max_result_records,
            SettingsField::RefreshSeconds => &mut self.refresh_seconds,
            SettingsField::OverviewAbove => &mut self.overview_above,
            SettingsField::CantonTransactions => &mut self.canton_transactions,
            SettingsField::LogLevel => &mut self.log_level,
            SettingsField::LogDirectory => &mut self.log_directory,
        };
        *target = value;
        self.message = None;
    }

    /// The edited settings as a checked config.
    pub fn to_config(&self) -> Result<Config, String> {
        fn whole<T: std::str::FromStr>(value: &str, name: &str) -> Result<T, String> {
            value.trim().parse().map_err(|_| format!("{name} must be a whole number"))
        }
        let max_records_per_table = whole(&self.max_records, "max records per table")?;
        let max_result_records = whole(&self.max_result_records, "max records in a query result")?;
        let refresh_seconds = whole(&self.refresh_seconds, "the auto refresh interval")?;
        let overview_above = whole(&self.overview_above, "the table overview threshold")?;
        let canton_transactions = whole(&self.canton_transactions, "transactions loaded at a time")?;
        let config = Config {
            database: DatabaseConfig {
                server: self.server.trim().into(),
                username: self.username.trim().into(),
                user_level: self.user_level,
                password: self.password.clone(),
                namespace: self.namespace.trim().into(),
                database: self.database.trim().into(),
                viewer_username: self.viewer_username.trim().into(),
                viewer_user_level: self.viewer_user_level,
                viewer_password: self.viewer_password.clone(),
            },
            graph: GraphConfig {
                max_records_per_table,
                max_result_records,
                connect_results: self.connect_results,
                auto_refresh: self.auto_refresh,
                refresh_seconds,
                overview_above,
                canton_transactions,
                // Set in the Canton view's sidebar; the app keeps the live value.
                canton_columns: 3,
                fit_on_filter: self.fit_on_filter,
                // Set in the Canton view's sidebar; the app keeps the live value.
                canton_follow_newest: false,
                visualization: self.visualization,
            },
            logging: LoggingConfig {
                level: self.log_level.clone(),
                directory: self.log_directory.trim().into(),
            },
            mcp: self.mcp.clone(),
            analytics: self.analytics.clone(),
            memory: self.memory.clone(),
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    /// `pending` is a data action waiting for confirmation, `busy` that one is
    /// running, `connected` that the database is open.
    pub fn view(&self, pending: Option<DataAction>, busy: bool, connected: bool) -> Element<'_, Message> {
        let field = |label: &'static str, hint: &'static str, value: &str, which: SettingsField| {
            column![
                text(label).size(13),
                text_input("", value)
                    .on_input(move |v| Message::SettingChanged(which, v))
                    .size(13),
                text(hint).size(11).color(MUTED),
            ]
            .spacing(3)
        };

        let mut col = Column::new().spacing(10).padding(16).width(SIDEBAR_WIDTH);
        col = col.push(
            row![
                text("Settings").size(18).width(Fill),
                button(text("Back").size(12)).style(button::secondary).on_press(Message::CloseSettings),
            ]
            .align_y(iced::Alignment::Center),
        );
        col = col.push(text(format!("Saved in {CONFIG_FILE}.")).size(12).color(MUTED));

        col = col.push(heading("Database"));
        col = col.push(
            text("The app connects to a running SurrealDB server, which other programs can use at the same time.")
                .size(11)
                .color(MUTED),
        );
        col = col.push(field("Server", "For example ws://127.0.0.1:8000", &self.server, SettingsField::Server));
        col = col.push(field("Username", "A user of the server; the next setting says where it is defined.", &self.username, SettingsField::Username));
        col = col.push(
            column![
                text("User level").size(13),
                pick_list(UserLevel::ALL, Some(self.user_level), Message::UserLevelChanged).text_size(13),
                text("Where the user is defined. A namespace or database user signs in to the namespace and database below.")
                    .size(11)
                    .color(MUTED),
            ]
            .spacing(3),
        );
        col = col.push(
            column![
                text("Password").size(13),
                text_input("", &self.password)
                    .on_input(|v| Message::SettingChanged(SettingsField::Password, v))
                    .secure(!self.password.trim().starts_with("op://"))
                    .size(13),
                text("The password, or a 1Password reference (op://…).").size(11).color(MUTED),
            ]
            .spacing(3),
        );
        col = col.push(field("Namespace", "", &self.namespace, SettingsField::Namespace));
        col = col.push(field("Database", "", &self.database, SettingsField::Database));

        col = col.push(heading("Read-only queries"));
        col = col.push(
            text("Queries typed in the query panel or sent by Claude Code run as this user. Give it the VIEWER role, \
                  so the server refuses any query that would change data. Left empty, they run as the user above, \
                  and the app refuses queries that look like writes.")
                .size(11)
                .color(MUTED),
        );
        col = col.push(field(
            "Viewer username",
            "For example: DEFINE USER viewer ON DATABASE PASSWORD '…' ROLES VIEWER",
            &self.viewer_username,
            SettingsField::ViewerUsername,
        ));
        col = col.push(
            column![
                text("Viewer user level").size(13),
                pick_list(UserLevel::ALL, Some(self.viewer_user_level), Message::ViewerUserLevelChanged).text_size(13),
            ]
            .spacing(3),
        );
        col = col.push(
            column![
                text("Viewer password").size(13),
                text_input("", &self.viewer_password)
                    .on_input(|v| Message::SettingChanged(SettingsField::ViewerPassword, v))
                    .secure(!self.viewer_password.trim().starts_with("op://"))
                    .size(13),
                text("The password, or a 1Password reference (op://…).").size(11).color(MUTED),
            ]
            .spacing(3),
        );
        col = col.push(heading("Graph: all views"));
        col = col.push(field(
            "Max records per table",
            "For the whole-database view. Applies on save (the graph reloads).",
            &self.max_records,
            SettingsField::MaxRecords,
        ));
        col = col.push(field(
            "Max records in a query result",
            "A query's result replaces the graph; at most this many records are drawn.",
            &self.max_result_records,
            SettingsField::MaxResultRecords,
        ));
        col = col.push(field(
            "Table overview above",
            "A database with more records than this opens on an overview of its tables.",
            &self.overview_above,
            SettingsField::OverviewAbove,
        ));
        col = col.push(field(
            "Auto refresh every (seconds)",
            "How often auto refresh checks the database. Turn it on in the sidebar.",
            &self.refresh_seconds,
            SettingsField::RefreshSeconds,
        ));

        col = col.push(heading("Canton event graph view"));
        col = col.push(field(
            "Transactions loaded at a time",
            "The window the Older / Newer / Latest buttons page through. The view's filters are in the sidebar.",
            &self.canton_transactions,
            SettingsField::CantonTransactions,
        ));

        col = col.push(heading("Logging"));
        col = col.push(
            column![
                text("Level").size(13),
                pick_list(LOG_LEVELS, Some(self.log_level.as_str()), |level: &str| {
                    Message::SettingChanged(SettingsField::LogLevel, level.to_string())
                })
                .text_size(13),
                text("The RUST_LOG environment variable overrides this.").size(11).color(MUTED),
            ]
            .spacing(3),
        );
        col = col.push(field("Folder", "Daily log files are written here.", &self.log_directory, SettingsField::LogDirectory));

        col = col.push(
            text("Saving applies the changes straight away; only a new log folder waits for a restart.")
                .size(11)
                .color(MUTED),
        );
        col = col.push(button(text("Save & apply").size(13)).style(button::primary).on_press(Message::SaveSettings));
        match &self.message {
            Some(Ok(m)) => col = col.push(text(m).size(12).color(SUCCESS)),
            Some(Err(e)) => col = col.push(text(e).size(12).color(ERROR)),
            None => {}
        }

        col = col.push(heading("Claude Code"));
        col = col.push(if self.mcp.enabled {
            column![
                text(format!("Tools for Claude Code at http://127.0.0.1:{}/mcp", self.mcp.port)).size(12),
                text("Register them once, in a terminal:").size(11).color(MUTED),
                text(format!(
                    "claude mcp add --transport http --scope user surrealdb-explorer http://127.0.0.1:{}/mcp",
                    self.mcp.port
                ))
                .size(11)
                .font(iced::Font::MONOSPACE),
                text("Turn off or change the port with [mcp] in config.toml (applies at the next start).")
                    .size(11)
                    .color(MUTED),
            ]
            .spacing(3)
        } else {
            column![text("Turned off ([mcp] enabled = false in config.toml).").size(12).color(MUTED)]
        });

        col = col.push(heading("Data"));
        col = col.push(data_section(pending, busy, connected));

        scrollable(col).height(Fill).into()
    }
}

/// Load sample data / erase database, each with a confirmation step.
fn data_section(pending: Option<DataAction>, busy: bool, connected: bool) -> Element<'static, Message> {
    if !connected {
        return text("The database isn't open.").size(12).color(MUTED).into();
    }
    if busy {
        return text("Working…").size(12).color(MUTED).into();
    }
    match pending {
        None => column![
            row![
                button(text("Load sample data").size(13))
                    .style(button::secondary)
                    .on_press(Message::AskDataAction(DataAction::LoadSampleData)),
                button(text("Load Canton sample").size(13))
                    .style(button::secondary)
                    .on_press(Message::AskDataAction(DataAction::LoadCantonSample)),
            ]
            .spacing(8),
            button(text("Erase database").size(13))
                .style(button::danger)
                .on_press(Message::AskDataAction(DataAction::EraseDatabase)),
            text("Each asks for confirmation first.").size(11).color(MUTED),
        ]
        .spacing(4)
        .into(),
        Some(action) => {
            let (question, confirm, style): (&str, &str, fn(&iced::Theme, button::Status) -> button::Style) =
                match action {
                    DataAction::LoadSampleData => (
                        "Add the sample people and their relations? This deletes all existing `knows` relations; other data is kept.",
                        "Load sample data",
                        button::primary,
                    ),
                    DataAction::LoadCantonSample => (
                        "Add a small Canton event graph (parties, transactions, created and exercised events) and show it as a Canton event graph? Other data is kept.",
                        "Load Canton sample",
                        button::primary,
                    ),
                    DataAction::EraseDatabase => (
                        "Erase everything in this database: all tables, records and definitions? This can't be undone.",
                        "Erase database",
                        button::danger,
                    ),
                };
            column![
                text(question).size(12).color(if action == DataAction::EraseDatabase { ERROR } else { MUTED }),
                row![
                    button(text(confirm).size(13)).style(style).on_press(Message::ConfirmDataAction),
                    button(text("Cancel").size(13)).style(button::secondary).on_press(Message::CancelDataAction),
                ]
                .spacing(8),
            ]
            .spacing(6)
            .into()
        }
    }
}
