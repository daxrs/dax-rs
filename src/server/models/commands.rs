use crate::catalog::{ColumnDataCategory, SummarizeBy, TableDataCategory};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnDataType {
    String,
    Int64,
    Double,
    Decimal,
    DateTime,
    Boolean,
    Binary,
}

impl From<&ColumnDataType> for polars::prelude::DataType {
    fn from(dt: &ColumnDataType) -> Self {
        use polars::prelude::{DataType, TimeUnit};
        match dt {
            ColumnDataType::String => DataType::String,
            ColumnDataType::Int64 => DataType::Int64,
            ColumnDataType::Double => DataType::Float64,
            ColumnDataType::Decimal => DataType::Float64,
            ColumnDataType::DateTime => DataType::Datetime(TimeUnit::Milliseconds, None),
            ColumnDataType::Boolean => DataType::Boolean,
            ColumnDataType::Binary => DataType::Binary,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    #[serde(default)]
    pub dry_run: bool,
    pub commands: Vec<Command>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResponse {
    pub dry_run: bool,
    pub applied: usize,
    pub errors: Vec<CommandError>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub index: usize,
    pub command: String,
    pub message: String,
}

// Command enum --------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    // Model
    RenameModel(RenameModelCmd),
    SetModelProperty(SetModelPropertyCmd),
    // Table
    CreateTable(CreateTableCmd),
    DeleteTable(DeleteTableCmd),
    RenameTable(RenameTableCmd),
    SetTableProperty(SetTablePropertyCmd),
    // Column
    AddColumn(AddColumnCmd),
    DeleteColumn(DeleteColumnCmd),
    RenameColumn(RenameColumnCmd),
    ChangeColumnType(ChangeColumnTypeCmd),
    SetColumnProperty(SetColumnPropertyCmd),
    // Measure
    CreateMeasure(CreateMeasureCmd),
    DeleteMeasure(DeleteMeasureCmd),
    RenameMeasure(RenameMeasureCmd),
    UpdateMeasureExpression(UpdateMeasureExpressionCmd),
    SetMeasureFormatString(SetMeasureFormatStringCmd),
    // Relationship
    CreateRelationship(CreateRelationshipCmd),
    DeleteRelationship(DeleteRelationshipCmd),
    UpdateRelationship(UpdateRelationshipCmd),
}

impl Command {
    pub fn type_name(&self) -> &'static str {
        match self {
            Command::RenameModel(_) => "RenameModel",
            Command::SetModelProperty(_) => "SetModelProperty",
            Command::CreateTable(_) => "CreateTable",
            Command::DeleteTable(_) => "DeleteTable",
            Command::RenameTable(_) => "RenameTable",
            Command::SetTableProperty(_) => "SetTableProperty",
            Command::AddColumn(_) => "AddColumn",
            Command::DeleteColumn(_) => "DeleteColumn",
            Command::RenameColumn(_) => "RenameColumn",
            Command::ChangeColumnType(_) => "ChangeColumnType",
            Command::SetColumnProperty(_) => "SetColumnProperty",
            Command::CreateMeasure(_) => "CreateMeasure",
            Command::DeleteMeasure(_) => "DeleteMeasure",
            Command::RenameMeasure(_) => "RenameMeasure",
            Command::UpdateMeasureExpression(_) => "UpdateMeasureExpression",
            Command::SetMeasureFormatString(_) => "SetMeasureFormatString",
            Command::CreateRelationship(_) => "CreateRelationship",
            Command::DeleteRelationship(_) => "DeleteRelationship",
            Command::UpdateRelationship(_) => "UpdateRelationship",
        }
    }
}

// Property update enums -----------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelPropertyUpdate {
    Culture(String),
    Collation(String),
    DefaultMode(String),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TablePropertyUpdate {
    IsHidden(bool),
    Description(Option<String>),
    DataCategory(Option<TableDataCategory>),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ColumnPropertyUpdate {
    IsHidden(bool),
    Description(Option<String>),
    FormatString(Option<String>),
    DisplayFolder(Option<String>),
    DataCategory(Option<ColumnDataCategory>),
    SortByColumn(Option<String>),
    SummarizeBy(SummarizeBy),
}

// Model payloads ------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameModelCmd {
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelPropertyCmd {
    pub property: ModelPropertyUpdate,
}

// Table payloads ------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTableCmd {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteTableCmd {
    pub table: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTableCmd {
    pub table: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTablePropertyCmd {
    pub table: String,
    pub property: TablePropertyUpdate,
}

// Column payloads -----------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddColumnCmd {
    pub table: String,
    pub name: String,
    pub data_type: ColumnDataType,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub is_nullable: bool,
    #[serde(default)]
    pub is_key: bool,
    #[serde(default)]
    pub is_unique: bool,
    pub summarize_by: Option<SummarizeBy>,
    pub format_string: Option<String>,
    pub display_folder: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteColumnCmd {
    pub table: String,
    pub column: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameColumnCmd {
    pub table: String,
    pub column: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeColumnTypeCmd {
    pub table: String,
    pub column: String,
    pub data_type: ColumnDataType,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetColumnPropertyCmd {
    pub table: String,
    pub column: String,
    pub property: ColumnPropertyUpdate,
}

// Measure payloads ----------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMeasureCmd {
    pub table: String,
    pub name: String,
    pub expression: String,
    pub format_string: Option<String>,
    pub display_folder: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub is_hidden: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMeasureCmd {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameMeasureCmd {
    pub name: String,
    pub new_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMeasureExpressionCmd {
    pub name: String,
    pub expression: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMeasureFormatStringCmd {
    pub name: String,
    pub format_string: Option<String>,
}

// Relationship payloads -----------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRelationshipCmd {
    pub name: String,
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    #[serde(default = "default_true")]
    pub is_active: bool,
    #[serde(default)]
    pub bidirectional: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteRelationshipCmd {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateRelationshipCmd {
    pub name: String,
    pub is_active: Option<bool>,
    pub bidirectional: Option<bool>,
}

fn default_true() -> bool {
    true
}
