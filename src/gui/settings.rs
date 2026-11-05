//! The settings view: edits `config.toml`.

use iced::widget::{Column, button, checkbox, column, pick_list, row, scrollable, text, text_input};
use iced::{Element, Fill};
use surrealdb_explore::config::{
    CONFIG_FILE, Config, DatabaseConfig, GraphConfig, LOG_LEVELS, LoggingConfig,
};

use crate::gui::app::Message;
use crate::gui::sidebar::{ERROR, MUTED, SIDEBAR_WIDTH, SUCCESS, heading};

/// A text field of the settings form.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SettingsField {
    DatabasePath,
    Namespace,
    Database,
    KeychainService,
    MaxRecords,
    LogLevel,
    LogDirectory,
}

/// The settings as edited, kept as text until saved.
pub struct SettingsForm {
    pub database_path: String,
    pub namespace: String,
    pub database: String,
    pub encryption: bool,
    pub keychain_service: String,
    pub max_records: String,
    pub log_level: String,
    pub log_directory: String,
    /// Result of the last save: a confirmation or an error.
    pub message: Option<Result<String, String>>,
}

impl SettingsForm {
    pub fn from_config(c: &Config) -> Self {
        SettingsForm {
            database_path: c.database.path.clone(),
            namespace: c.database.namespace.clone(),
            database: c.database.database.clone(),
            encryption: c.database.encryption,
            keychain_service: c.database.keychain_service.clone(),
            max_records: c.graph.max_records_per_table.to_string(),
            log_level: c.logging.level.clone(),
            log_directory: c.logging.directory.clone(),
            message: None,
        }
    }

    pub fn set(&mut self, field: SettingsField, value: String) {
        let target = match field {
            SettingsField::DatabasePath => &mut self.database_path,
            SettingsField::Namespace => &mut self.namespace,
            SettingsField::Database => &mut self.database,
            SettingsField::KeychainService => &mut self.keychain_service,
            SettingsField::MaxRecords => &mut self.max_records,
            SettingsField::LogLevel => &mut self.log_level,
            SettingsField::LogDirectory => &mut self.log_directory,
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
                encryption: self.encryption,
                keychain_service: self.keychain_service.trim().into(),
            },
            graph: GraphConfig { max_records_per_table },
            logging: LoggingConfig {
                level: self.log_level.clone(),
                directory: self.log_directory.trim().into(),
            },
        };
        config.validate().map_err(|e| e.to_string())?;
        Ok(config)
    }

    pub fn view(&self) -> Element<'_, Message> {
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
        col = col.push(
            checkbox(self.encryption)
                .label("Encrypted at rest")
                .on_toggle(Message::EncryptionToggled)
                .size(16)
                .text_size(13),
        );
        col = col.push(
            text("Must match how the folder was created: an encrypted folder needs the key, an unencrypted one can't be opened with it.")
                .size(11)
                .color(MUTED),
        );
        if self.encryption {
            col = col.push(field(
                "Keychain service",
                "Keychain item holding the key. A new name creates a new key.",
                &self.keychain_service,
                SettingsField::KeychainService,
            ));
        }

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

        col = col.push(text("Database and logging changes take effect after a restart.").size(11).color(MUTED));
        col = col.push(button(text("Save").size(13)).style(button::primary).on_press(Message::SaveSettings));
        match &self.message {
            Some(Ok(m)) => col = col.push(text(m).size(12).color(SUCCESS)),
            Some(Err(e)) => col = col.push(text(e).size(12).color(ERROR)),
            None => {}
        }

        scrollable(col).height(Fill).into()
    }
}
