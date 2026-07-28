pub mod commands;
pub(crate) mod lineage;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use std::sync::Arc;

use crate::server::provider::{ColumnMeta, MeasureMeta, RelationshipMeta, TableMeta};
use crate::server::ServerProvider;
use commands::CommandRequest;

#[derive(Clone)]
struct AppState {
    provider: Arc<dyn ServerProvider>,
}

pub fn routes(provider: Arc<dyn ServerProvider>) -> Router {
    Router::new()
        .route("/models", get(list_models))
        .route("/models/{id}", get(get_model))
        .route("/models/{id}/tables", get(list_tables))
        .route("/models/{id}/tables/{table_id}", get(get_table))
        .route("/models/{id}/relationships", get(list_relationships))
        .route("/models/{id}/measures", get(list_measures))
        .route("/models/{id}/dependencies/{object}", get(get_dependencies))
        .route("/models/{id}/dax/validate", post(validate_dax))
        .route("/models/{id}/dax/evaluate", post(evaluate_dax))
        .route("/models/{id}/commands", post(run_commands))
        .route("/models/{id}/refreshdata", post(refresh_data))
        .route("/models/{id}/reloadmodel", post(reload_model))
        .with_state(AppState { provider })
}

// Evaluate response types ------------------------------------------------------

#[derive(Serialize)]
struct EvaluateColumn {
    name: String,
    data_type: String,
}

#[derive(Serialize)]
struct EvaluateTable {
    columns: Vec<EvaluateColumn>,
    rows: Vec<Vec<Option<String>>>,
}

// Validation response types ----------------------------------------------------

#[derive(Serialize)]
struct ValidateErrorResponse {
    kind: String,
    message: String,
}

#[derive(Serialize)]
struct ValidateResponse {
    valid: bool,
    errors: Vec<ValidateErrorResponse>,
}

// Dependency response types ----------------------------------------------------

#[derive(Serialize)]
struct ColumnRefResponse {
    table: String,
    column: String,
}

#[derive(Serialize)]
struct UpstreamResponse {
    measures: Vec<String>,
    columns: Vec<ColumnRefResponse>,
}

#[derive(Serialize)]
struct DownstreamResponse {
    measures: Vec<String>,
}

#[derive(Serialize)]
struct DependencyResponse {
    object: String,
    object_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    table: Option<String>,
    upstream: UpstreamResponse,
    downstream: DownstreamResponse,
    impacted_measures: Vec<String>,
}

// Response types ---------------------------------------------------------------

#[derive(Serialize)]
struct ModelSummary {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct ModelDetail {
    id: String,
    name: String,
    culture: String,
    collation: String,
    compatibility_level: u32,
    default_mode: String,
    state: String,
    read_write_mode: String,
    created_timestamp: String,
    last_schema_update: String,
    last_refreshed: String,
}

#[derive(Serialize)]
struct ColumnResponse {
    name: String,
    data_type: String,
    summarize_by: String,
    is_hidden: bool,
    is_key: bool,
    is_nullable: bool,
    is_unique: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sort_by_column: Option<String>,
}

#[derive(Serialize)]
struct TableResponse {
    id: String,
    name: String,
    is_hidden: bool,
    columns: Vec<ColumnResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data_category: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct RelationshipResponse {
    name: String,
    from_table: String,
    from_column: String,
    to_table: String,
    to_column: String,
    is_active: bool,
    bidirectional: bool,
}

#[derive(Serialize)]
struct MeasureResponse {
    name: String,
    table: String,
    expression: String,
    is_hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format_string: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

// Handlers ---------------------------------------------------------------------

async fn list_models(State(state): State<AppState>) -> Json<Vec<ModelSummary>> {
    Json(
        state
            .provider
            .list_databases()
            .into_iter()
            .map(|db| ModelSummary { id: db.name.clone(), name: db.name })
            .collect(),
    )
}

async fn get_model(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let meta = db.model_meta();
    Json(ModelDetail {
        id: id.clone(),
        name: id,
        culture: meta.culture,
        collation: meta.collation,
        compatibility_level: meta.compatibility_level,
        default_mode: meta.default_mode,
        state: meta.state,
        read_write_mode: meta.read_write_mode,
        created_timestamp: meta.created_timestamp,
        last_schema_update: meta.last_schema_update,
        last_refreshed: meta.last_refreshed,
    })
    .into_response()
}

async fn list_tables(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(
        db.list_tables()
            .into_iter()
            .map(TableResponse::from)
            .collect::<Vec<_>>(),
    )
    .into_response()
}

async fn get_table(
    State(state): State<AppState>,
    Path((id, table_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(table) = db.list_tables().into_iter().find(|t| t.name == table_id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(TableResponse::from(table)).into_response()
}

async fn list_relationships(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(
        db.list_relationships()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<RelationshipResponse>>(),
    )
    .into_response()
}

async fn list_measures(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(
        db.list_measures()
            .into_iter()
            .map(Into::into)
            .collect::<Vec<MeasureResponse>>(),
    )
    .into_response()
}

async fn evaluate_dax(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: String,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match db.execute_dax(&body) {
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
        Ok(results) => {
            let tables: Vec<EvaluateTable> = results
                .into_iter()
                .map(|qr| {
                    let n_cols = qr.columns.len();
                    let n_rows = qr.rows.first().map(|c| c.len()).unwrap_or(0);
                    let rows = (0..n_rows)
                        .map(|ri| {
                            (0..n_cols)
                                .map(|ci| {
                                    qr.rows
                                        .get(ci)
                                        .and_then(|col| col.get(ri))
                                        .cloned()
                                        .flatten()
                                })
                                .collect()
                        })
                        .collect();
                    EvaluateTable {
                        columns: qr
                            .columns
                            .into_iter()
                            .map(|(name, data_type)| EvaluateColumn { name, data_type })
                            .collect(),
                        rows,
                    }
                })
                .collect();
            Json(tables).into_response()
        }
    }
}

async fn validate_dax(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: String,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let result = db.validate_dax(&body);
    Json(ValidateResponse {
        valid: result.valid,
        errors: result
            .errors
            .into_iter()
            .map(|e| ValidateErrorResponse { kind: e.kind, message: e.message })
            .collect(),
    })
    .into_response()
}

async fn get_dependencies(
    State(state): State<AppState>,
    Path((id, object)): Path<(String, String)>,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(info) = db.dependencies_of(&object) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(DependencyResponse {
        object: info.object,
        object_type: info.object_type,
        table: info.table,
        upstream: UpstreamResponse {
            measures: info.upstream_measures,
            columns: info
                .upstream_columns
                .into_iter()
                .map(|c| ColumnRefResponse { table: c.table, column: c.column })
                .collect(),
        },
        downstream: DownstreamResponse { measures: info.downstream_measures },
        impacted_measures: info.impacted_measures,
    })
    .into_response()
}

// Conversions ------------------------------------------------------------------

impl From<TableMeta> for TableResponse {
    fn from(t: TableMeta) -> Self {
        TableResponse {
            id: t.name.clone(),
            name: t.name,
            is_hidden: t.is_hidden,
            data_category: t.data_category.as_ref().map(|c| c.as_str().to_string()),
            description: t.description,
            columns: t.columns.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<ColumnMeta> for ColumnResponse {
    fn from(c: ColumnMeta) -> Self {
        ColumnResponse {
            name: c.name,
            data_type: c.data_type,
            summarize_by: c.summarize_by.as_csdl_str().to_string(),
            is_hidden: c.is_hidden,
            is_key: c.is_key,
            is_nullable: c.is_nullable,
            is_unique: c.is_unique,
            format_string: c.format_string,
            display_folder: c.display_folder,
            data_category: c.data_category.as_ref().map(|c| c.as_str().to_string()),
            description: c.description,
            sort_by_column: c.sort_by_column,
        }
    }
}

impl From<RelationshipMeta> for RelationshipResponse {
    fn from(r: RelationshipMeta) -> Self {
        RelationshipResponse {
            name: r.name,
            from_table: r.from_table,
            from_column: r.from_column,
            to_table: r.to_table,
            to_column: r.to_column,
            is_active: r.is_active,
            bidirectional: r.bidirectional,
        }
    }
}

impl From<MeasureMeta> for MeasureResponse {
    fn from(m: MeasureMeta) -> Self {
        MeasureResponse {
            name: m.name,
            table: m.table_name,
            expression: m.expression,
            is_hidden: m.is_hidden,
            format_string: m.format_string,
            display_folder: m.display_folder,
            description: m.description,
        }
    }
}

async fn run_commands(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<CommandRequest>,
) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(db.apply_commands(&req.commands, req.dry_run)).into_response()
}

async fn refresh_data(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::task::spawn_blocking(move || db.refresh_data()).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("refresh task failed: {e}"),
        )
            .into_response(),
    }
}

async fn reload_model(State(state): State<AppState>, Path(id): Path<String>) -> impl IntoResponse {
    let Some(db) = state.provider.database(&id) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    match tokio::task::spawn_blocking(move || db.reload_model()).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("reload task failed: {e}"),
        )
            .into_response(),
    }
}
