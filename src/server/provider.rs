use crate::catalog::{ColumnDataCategory, SummarizeBy, TableDataCategory};
use crate::server::models::commands::{Command, CommandError, CommandResponse};
use std::sync::Arc;

#[derive(Clone)]
pub struct DatabaseMeta {
    pub id: String,
    pub name: String,
    pub last_schema_update: String,
    pub last_refreshed: String,
}

pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
    pub summarize_by: SummarizeBy,
    pub is_hidden: bool,
    pub format_string: Option<String>,
    pub display_folder: Option<String>,
    pub data_category: Option<ColumnDataCategory>,
    pub description: Option<String>,
    pub sort_by_column: Option<String>,
    pub is_key: bool,
    pub is_nullable: bool,
    pub is_unique: bool,
}

pub struct TableMeta {
    pub name: String,
    pub columns: Vec<ColumnMeta>,
    pub is_hidden: bool,
    pub data_category: Option<TableDataCategory>,
    pub description: Option<String>,
}

pub struct MeasureMeta {
    pub name: String,
    pub table_name: String,
    pub display_name: String,
    pub expression: String,
    pub aggregator: i32,
    /// OLEDB data type constant (e.g. 5 = Double, 3 = Integer).
    pub data_type: u16,
    pub is_hidden: bool,
    pub format_string: Option<String>,
    pub display_folder: Option<String>,
    pub description: Option<String>,
}

pub struct RelationshipMeta {
    pub name: String,
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub is_active: bool,
    pub bidirectional: bool,
}

pub struct QueryResult {
    pub columns: Vec<(String, String)>,
    pub rows: Vec<Vec<Option<String>>>,
}

pub struct ModelMeta {
    pub culture: String,
    pub collation: String,
    pub compatibility_level: u32,
    pub storage_engine_used: String,
    pub default_mode: String,
    pub state: String,
    pub read_write_mode: String,
    pub created_timestamp: String,
    pub last_schema_update: String,
    pub last_refreshed: String,
}

impl Default for ModelMeta {
    fn default() -> Self {
        Self {
            culture: "en-US".into(),
            collation: "Latin1_General_100_BIN2".into(),
            compatibility_level: 1500,
            storage_engine_used: "InMemory".into(),
            default_mode: "Import".into(),
            state: "FullyProcessed".into(),
            read_write_mode: "ReadOnly".into(),
            created_timestamp: "2026-01-01T00:00:00".into(),
            last_schema_update: "2026-01-01T00:00:00".into(),
            last_refreshed: "2026-01-01T00:00:00".into(),
        }
    }
}

pub trait ServerProvider: Send + Sync {
    fn list_databases(&self) -> Vec<DatabaseMeta>;
    fn database(&self, name: &str) -> Option<Arc<dyn DatabaseProvider>>;

    /// Creates an empty database (a model with no tables), stores it, and makes
    /// it available straight away. Blocking: call it from `spawn_blocking`.
    fn create_database(&self, name: &str) -> Result<DatabaseMeta, CreateDatabaseError> {
        let _ = name;
        Err(CreateDatabaseError::NotSupported)
    }
}

/// Why `ServerProvider::create_database` failed.
#[derive(Debug)]
pub enum CreateDatabaseError {
    /// The name breaks the naming rule (see `validate_database_name`).
    InvalidName(String),
    /// A database with this name (case-insensitive), or its file, already exists.
    AlreadyExists(String),
    /// This server has no models storage to create databases in.
    NotSupported,
    /// Writing or loading the new model file failed.
    Storage(String),
}

impl std::fmt::Display for CreateDatabaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateDatabaseError::InvalidName(message) => write!(f, "invalid name: {message}"),
            CreateDatabaseError::AlreadyExists(message) => write!(f, "{message}"),
            CreateDatabaseError::NotSupported => {
                write!(
                    f,
                    "this server has no models storage to create databases in"
                )
            }
            CreateDatabaseError::Storage(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for CreateDatabaseError {}

/// Longest allowed database name, in characters.
pub const MAX_DATABASE_NAME_LEN: usize = 100;

/// A database name is also its file name in the models storage, so it's kept
/// to characters that are safe on every storage back-end: 1–100 letters,
/// digits, spaces, `_` and `-`, with no space at the start or end.
pub fn validate_database_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("the name is empty".into());
    }
    if name.chars().count() > MAX_DATABASE_NAME_LEN {
        return Err(format!(
            "the name is longer than {MAX_DATABASE_NAME_LEN} characters"
        ));
    }
    if name.starts_with(' ') || name.ends_with(' ') {
        return Err("the name starts or ends with a space".into());
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_alphanumeric() || matches!(c, ' ' | '_' | '-')))
    {
        return Err(format!(
            "'{bad}' is not allowed; use letters, digits, spaces, '_' and '-'"
        ));
    }
    Ok(())
}

pub struct ColumnRef {
    pub table: String,
    pub column: String,
}

#[derive(Clone)]
pub struct ValidationError {
    pub kind: String,
    pub message: String,
}

pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
}

pub struct DependencyInfo {
    pub object: String,
    pub object_type: String,
    pub table: Option<String>,
    pub upstream_measures: Vec<String>,
    pub upstream_columns: Vec<ColumnRef>,
    pub downstream_measures: Vec<String>,
    pub impacted_measures: Vec<String>,
}

pub trait DatabaseProvider: Send + Sync {
    fn name(&self) -> &str;
    fn list_measures(&self) -> Vec<MeasureMeta>;
    fn list_tables(&self) -> Vec<TableMeta>;
    fn list_relationships(&self) -> Vec<RelationshipMeta>;
    fn execute_dax(&self, query: &str) -> Result<Vec<QueryResult>, String>;
    fn execute_mdx(
        &self,
        _mdx: &str,
    ) -> Result<(crate::mdxtranslator::TranslatedQuery, QueryResult), String> {
        Err("MDX execution is not supported by this provider".into())
    }
    fn model_meta(&self) -> ModelMeta {
        ModelMeta::default()
    }
    fn refresh_data(&self) -> Result<(), String> {
        Err("not supported".into())
    }
    fn reload_model(&self) -> Result<(), String> {
        Err("not supported".into())
    }
    fn dependencies_of(&self, _object: &str) -> Option<DependencyInfo> {
        None
    }
    fn validate_dax(&self, _query: &str) -> ValidationResult {
        ValidationResult { valid: true, errors: vec![] }
    }

    fn apply_commands(&self, commands: &[Command], dry_run: bool) -> CommandResponse {
        let _ = (commands, dry_run);
        CommandResponse {
            dry_run,
            applied: 0,
            errors: vec![CommandError {
                index: 0,
                command: String::new(),
                message: "not supported".into(),
            }],
        }
    }
}
