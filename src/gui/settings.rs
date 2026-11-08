//! The settings view: edits `config.toml`.

use iced::widget::{Column, button, column, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill};
use surrealdb_explorer::config::{
    AssistantConfig, AssistantMode, CONFIG_FILE, Config, DatabaseConfig, GraphConfig, LOG_LEVELS,
    LoggingConfig,
};

use crate::gui::app::{DataAction, Message};
use crate::gui::sidebar::{ERROR, MUTED, SIDEBAR_WIDTH, SUCCESS, heading};

/// A text field of the settings form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    DatabasePath,
    Namespace,
    Database,
    MaxRecords,
    LogLevel,
    LogDirectory,
    ApiKey,
}

/// The settings as edited, kept as text until saved.
pub struct SettingsForm {
    pub database_path: String,
    pub namespace: String,
    pub database: String,
    pub max_records: String,
    pub log_level: String,
    pub log_directory: String,
    pub assistant_mode: AssistantMode,
    /// 1Password reference, not the key itself.
    pub api_key: String,
    /// Not edited here (the query panel has the checkbox); kept so saving
    /// doesn't reset it.
    pub show_connections: bool,
    /// Result of the last save: a confirmation or an error.
    pub message: Option<Result<String, String>>,
}

impl SettingsForm {
    pub fn from_config(c: &Config) -> Self {
        SettingsForm {
            database_path: c.database.path.clone(),
            namespace: c.database.namespace.clone(),
            database: c.database.database.clone(),
            max_records: c.graph.max_records_per_table.to_string(),
            log_level: c.logging.level.clone(),
            log_directory: c.logging.directory.clone(),
            assistant_mode: c.assistant.mode,
            api_key: c.assistant.api_key.clone(),
            show_connections: c.assistant.show_connections,
            message: None,
        }
    }

    pub fn set(&mut self, field: SettingsField, value: String) {
        let target = match field {
            SettingsField::DatabasePath => &mut self.database_path,
            SettingsField::Namespace => &mut self.namespace,
            SettingsField::Database => &mut self.database,
            SettingsField::MaxRecords => &mut self.max_records,
            SettingsField::LogLevel => &mut self.log_level,
            SettingsField::LogDirectory => &mut self.log_directory,
            SettingsField::ApiKey => &mut self.api_key,
        };
        *target = value;
        self.message = None;
    }

    /// The edited settings as a checked config.
    pub fn to_config(&self) -> Result<Config, String> {
        let max_records_per_table = self
            .max_records
            .trim()
            .parse()
            .map_err(|_| "max records per table must be a whole number".to_string())?;
        let config = Config {
            database: DatabaseConfig {
                path: self.database_path.trim().into(),
                namespace: self.namespace.trim().into(),
                database: self.database.trim().into(),
            },
            graph: GraphConfig { max_records_per_table },
            logging: LoggingConfig {
                level: self.log_level.clone(),
                directory: self.log_directory.trim().into(),
            },
            assistant: AssistantConfig {
                mode: self.assistant_mode,
                api_key: self.api_key.trim().into(),
                show_connections: self.show_connections,
            },
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
        col = col.push(field("Folder", "Relative to the folder the app runs from.", &self.database_path, SettingsField::DatabasePath));
        col = col.push(field("Namespace", "", &self.namespace, SettingsField::Namespace));
        col = col.push(field("Database", "", &self.database, SettingsField::Database));
        col = col.push(heading("Graph"));
        col = col.push(field(
            "Max records per table",
            "Applies on save (the graph reloads).",
            &self.max_records,
            SettingsField::MaxRecords,
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

        col = col.push(heading("Assistant"));
        col = col.push(
            column![
                text("Model").size(13),
                pick_list(AssistantMode::ALL, Some(self.assistant_mode), Message::AssistantModeChanged).text_size(13),
                text("Auto asks Haiku first, and Opus when Haiku isn't sure or its query fails the check.")
                    .size(11)
                    .color(MUTED),
            ]
            .spacing(3),
        );
        col = col.push(field(
            "API key in 1Password",
            "A reference like op://Personal/item/credential, read with the op tool. Never the key itself.",
            &self.api_key,
            SettingsField::ApiKey,
        ));

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
                button(text("Erase database").size(13))
                    .style(button::danger)
                    .on_press(Message::AskDataAction(DataAction::EraseDatabase)),
            ]
            .spacing(8),
            text("Both ask for confirmation first.").size(11).color(MUTED),
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
