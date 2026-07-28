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
