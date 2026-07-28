use std::sync::{Arc, RwLock};

use opendal::blocking::Operator as BlockingOperator;
use polars::prelude::{AnyValue, DataType};

use crate::catalog::Catalog;
use crate::engine::Engine;
use crate::loaders::tmsl::model::Relationship;
use crate::loaders::tmsl::save_tmsl_to_op;
use crate::server::models::commands::{Command, CommandError, CommandResponse};
use crate::server::provider::{
    ColumnMeta, DatabaseProvider, MeasureMeta, ModelMeta, QueryResult, RelationshipMeta, TableMeta,
};

pub struct DaxDatabaseProvider {
    name: String,
    engine: RwLock<Engine>,
    catalogs_op: Option<Arc<BlockingOperator>>,
    tmsl_path: String,
}

impl DaxDatabaseProvider {
    pub fn new(name: impl Into<String>, engine: Engine) -> Self {
        Self {
            name: name.into(),
            engine: RwLock::new(engine),
            catalogs_op: None,
            tmsl_path: String::new(),
        }
    }

    pub fn with_catalogs_storage(
        mut self,
        op: Arc<BlockingOperator>,
        path: impl Into<String>,
    ) -> Self {
        self.catalogs_op = Some(op);
        self.tmsl_path = path.into();
        self
    }

    fn read_engine(&self) -> std::sync::RwLockReadGuard<'_, Engine> {
        self.engine
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write_engine(&self) -> std::sync::RwLockWriteGuard<'_, Engine> {
        self.engine
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl DatabaseProvider for DaxDatabaseProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn list_measures(&self) -> Vec<MeasureMeta> {
        let engine = self.read_engine();
        let catalog = &engine.ctx().catalog;
        catalog
            .measure_order
            .iter()
            .filter_map(|name| {
                let expression = catalog.measures.get(name)?;
                let table_name = catalog
                    .measure_tables
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                Some(MeasureMeta {
                    name: name.clone(),
                    table_name,
                    display_name: name.clone(),
                    expression: expression.clone(),
                    aggregator: 0, // MDMEASURE_AGGR_CALCULATED
                    data_type: 20, // DBTYPE_I8 — default for DAX calculated measures
                    is_hidden: catalog
                        .measure_is_hidden
                        .get(name)
                        .copied()
                        .unwrap_or(false),
                    format_string: catalog.measure_format_string.get(name).cloned(),
                    display_folder: catalog.measure_display_folder.get(name).cloned(),
                    description: catalog.measure_description.get(name).cloned(),
                })
            })
            .collect()
    }

    fn list_relationships(&self) -> Vec<RelationshipMeta> {
        self.read_engine()
            .ctx()
            .catalog
            .relationships
            .iter()
            .map(|r| RelationshipMeta {
                name: r.name.clone(),
                from_table: r.from_table.clone(),
                from_column: r.from_column.clone(),
                to_table: r.to_table.clone(),
                to_column: r.to_column.clone(),
                is_active: r.active,
                bidirectional: r.bidirectional,
            })
            .collect()
    }

    fn list_tables(&self) -> Vec<TableMeta> {
        let engine = self.read_engine();
        let catalog = &engine.ctx().catalog;

        catalog
            .table_order
            .iter()
            .filter_map(|name| {
                let col_names = catalog.column_order.get(name)?;
                let columns: Vec<ColumnMeta> = col_names
                    .iter()
                    .filter_map(|col_name| {
                        let meta = catalog.columns.get(&(name.clone(), col_name.clone()))?;
                        Some(ColumnMeta {
                            name: col_name.clone(),
                            data_type: polars_type_to_xsd(&meta.dtype).to_string(),
                            summarize_by: meta.summarize_by.clone(),
                            is_hidden: meta.is_hidden,
                            format_string: meta.format_string.clone(),
                            display_folder: meta.display_folder.clone(),
                            data_category: meta.data_category.clone(),
                            description: meta.description.clone(),
                            sort_by_column: meta.sort_by_column.clone(),
                            is_key: meta.is_key,
                            is_nullable: meta.is_nullable,
                            is_unique: meta.is_unique,
                        })
                    })
                    .collect();
                let is_hidden = catalog.table_is_hidden.get(name).copied().unwrap_or(false);
                let data_category = catalog.table_data_category.get(name).cloned();
                let description = catalog.table_description.get(name).cloned();
                Some(TableMeta {
                    name: name.clone(),
                    columns,
                    is_hidden,
                    data_category,
                    description,
                })
            })
            .collect()
    }

    fn model_meta(&self) -> ModelMeta {
        let engine = self.read_engine();
        let catalog = &engine.ctx().catalog;
        ModelMeta {
            culture: catalog.culture.clone().unwrap_or_else(|| "en-US".into()),
            collation: catalog
                .collation
                .clone()
                .unwrap_or_else(|| "Latin1_General_100_BIN2".into()),
            compatibility_level: catalog.compatibility_level.unwrap_or(1500),
            default_mode: catalog
                .default_mode
                .clone()
                .unwrap_or_else(|| "Import".into()),
            created_timestamp: catalog.created_at.format("%Y-%m-%dT%H:%M:%S").to_string(),
            last_schema_update: catalog
                .last_schema_update
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            last_refreshed: catalog
                .last_refreshed
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string(),
            ..ModelMeta::default()
        }
    }

    fn refresh_data(&self) -> Result<(), String> {
        self.write_engine().reload_data()
    }

    fn reload_model(&self) -> Result<(), String> {
        let op = self
            .catalogs_op
            .as_ref()
            .ok_or_else(|| "model has no catalog storage configured".to_string())?;

        let (datasets_op, timezone) = {
            let engine = self.read_engine();
            (
                engine.ctx().datasets_op.clone(),
                engine.timezone().map(str::to_string),
            )
        };

        let mut new_engine =
            Engine::from_storage(op, &self.tmsl_path, datasets_op).map_err(|e| e.to_string())?;
        new_engine.set_timezone(timezone.as_deref())?;
        new_engine.warmup();

        *self.write_engine() = new_engine;
        Ok(())
    }

    fn apply_commands(&self, commands: &[Command], dry_run: bool) -> CommandResponse {
        let mut catalog = self.read_engine().ctx().catalog.clone();
        let (applied, errors) = run_commands(&mut catalog, commands);

        if dry_run {
            return CommandResponse { dry_run: true, applied, errors };
        }

        if !errors.is_empty() {
            return CommandResponse { dry_run: false, applied, errors };
        }

        if let Some(op) = &self.catalogs_op {
            if let Err(e) = save_tmsl_to_op(op, &self.tmsl_path, &catalog) {
                return CommandResponse {
                    dry_run: false,
                    applied: 0,
                    errors: vec![CommandError {
                        index: 0,
                        command: "save".into(),
                        message: format!("Failed to persist changes: {e}"),
                    }],
                };
            }
        }

        self.write_engine().ctx_mut().catalog = catalog;
        CommandResponse { dry_run: false, applied, errors }
    }

    fn validate_dax(&self, query: &str) -> crate::server::provider::ValidationResult {
        use crate::engine::dax::ast::Definition;
        use crate::engine::dax::parser::parse_query;
        use crate::engine::error::DaxError;
        use crate::engine::ir::binder::bind;
        use crate::engine::ir::builder::build_expression;
        use crate::server::provider::{ValidationError, ValidationResult};

        let parsed = match parse_query(query.trim()) {
            Ok(q) => q,
            Err(e) => {
                return ValidationResult {
                    valid: false,
                    errors: vec![ValidationError { kind: "syntax".into(), message: e.to_string() }],
                }
            }
        };

        let engine = self.read_engine();
        let ctx = engine.ctx();
        let mut errors: Vec<ValidationError> = Vec::new();

        let classify = |e: DaxError| -> ValidationError {
            let (kind, message) = match e {
                DaxError::Parse(m) => ("syntax", m),
                DaxError::Type(m) => ("type", m),
                DaxError::UnknownName(m) => ("semantic", m),
                DaxError::InvalidArgument(m) => ("semantic", m),
                DaxError::Eval(m) => ("semantic", m),
            };
            ValidationError { kind: kind.into(), message }
        };

        for def in parsed.define {
            let expr = match def {
                Definition::Var { expr, .. } => expr,
                Definition::Measure { expr, .. } => expr,
            };
            if let Err(e) = build_expression(*expr).and_then(|ir| bind(ir, ctx)) {
                errors.push(classify(e));
            }
        }

        for stmt in parsed.statements {
            if let Err(e) = build_expression(*stmt.expr).and_then(|ir| bind(ir, ctx)) {
                errors.push(classify(e));
            }
            for (order_expr, _) in stmt.order_by {
                if let Err(e) = build_expression(order_expr).and_then(|ir| bind(ir, ctx)) {
                    errors.push(classify(e));
                }
            }
        }

        ValidationResult { valid: errors.is_empty(), errors }
    }

    fn dependencies_of(&self, object: &str) -> Option<crate::server::provider::DependencyInfo> {
        use crate::server::models::lineage::{parse_column_object, DependencyGraph};
        use crate::server::provider::{ColumnRef, DependencyInfo};

        let engine = self.read_engine();
        let catalog = &engine.ctx().catalog;
        let graph = DependencyGraph::build(&catalog.measures);

        let measure_key = catalog
            .measures
            .keys()
            .find(|k| k.eq_ignore_ascii_case(object))
            .cloned();

        if let Some(name) = measure_key {
            let (upstream_m, upstream_c) = graph.upstream_of_measure(&name);
            let downstream = graph.direct_downstream_of_measure(&name);
            let impacted = graph.transitive_downstream(&name);
            let table = catalog.measure_tables.get(&name).cloned();
            return Some(DependencyInfo {
                object: name,
                object_type: "measure".into(),
                table,
                upstream_measures: upstream_m,
                upstream_columns: upstream_c
                    .into_iter()
                    .map(|(t, c)| ColumnRef { table: t, column: c })
                    .collect(),
                downstream_measures: downstream,
                impacted_measures: impacted,
            });
        }

        if let Some((table, column)) = parse_column_object(object) {
            let exists = catalog
                .columns
                .contains_key(&(table.clone(), column.clone()));
            if exists {
                let downstream = graph.measures_using_column(&table, &column);
                let impacted = graph.impacted_by_column(&table, &column);
                return Some(DependencyInfo {
                    object: format!("{table}[{column}]"),
                    object_type: "column".into(),
                    table: Some(table),
                    upstream_measures: vec![],
                    upstream_columns: vec![],
                    downstream_measures: downstream,
                    impacted_measures: impacted,
                });
            }
        }

        None
    }

    fn execute_dax(&self, query: &str) -> Result<Vec<QueryResult>, String> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.read_engine()
                .evaluate_query(query.trim())
                .map_err(|e| e.to_string())
        }))
        .unwrap_or_else(|payload| {
            let msg = payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            Err(format!("DAX engine panicked: {msg}"))
        });

        result?.into_iter().map(value_to_query_result).collect()
    }
}

fn run_commands(catalog: &mut Catalog, commands: &[Command]) -> (usize, Vec<CommandError>) {
    let mut applied = 0;
    let mut errors = Vec::new();
    for (i, cmd) in commands.iter().enumerate() {
        match apply_command(catalog, cmd) {
            Ok(()) => applied += 1,
            Err(message) => {
                errors.push(CommandError {
                    index: i,
                    command: cmd.type_name().to_string(),
                    message,
                });
                break;
            }
        }
    }
    (applied, errors)
}

fn apply_command(catalog: &mut Catalog, cmd: &Command) -> Result<(), String> {
    match cmd {
        Command::RenameModel(c) => {
            catalog.rename_model(&c.new_name);
            Ok(())
        }
        Command::SetModelProperty(c) => {
            use crate::server::models::commands::ModelPropertyUpdate;
            match &c.property {
                ModelPropertyUpdate::Culture(v) => catalog.set_culture(v.clone()),
                ModelPropertyUpdate::Collation(v) => catalog.set_collation(v.clone()),
                ModelPropertyUpdate::DefaultMode(v) => catalog.set_default_mode(v.clone()),
            }
            Ok(())
        }

        Command::CreateTable(c) => catalog.create_table(&c.name),
        Command::DeleteTable(c) => catalog.delete_table(&c.table),
        Command::RenameTable(c) => catalog.rename_table(&c.table, &c.new_name),
        Command::SetTableProperty(c) => {
            use crate::server::models::commands::TablePropertyUpdate;
            match &c.property {
                TablePropertyUpdate::IsHidden(v) => catalog.set_table_is_hidden(&c.table, *v),
                TablePropertyUpdate::Description(v) => {
                    catalog.set_table_description(&c.table, v.clone())
                }
                TablePropertyUpdate::DataCategory(v) => {
                    catalog.set_table_data_category(&c.table, v.clone())
                }
            }
        }

        Command::AddColumn(c) => {
            use crate::catalog::ColumnMeta as CM;
            let dtype = DataType::from(&c.data_type);
            let summarize_by = c.summarize_by.clone().unwrap_or_default();
            catalog.add_column(
                &c.table,
                CM {
                    table: c.table.clone(),
                    column: c.name.clone(),
                    dtype,
                    summarize_by,
                    is_hidden: c.is_hidden,
                    is_nullable: c.is_nullable,
                    is_key: c.is_key,
                    is_unique: c.is_unique,
                    format_string: c.format_string.clone(),
                    display_folder: c.display_folder.clone(),
                    data_category: None,
                    description: c.description.clone(),
                    sort_by_column: None,
                },
            )
        }
        Command::DeleteColumn(c) => catalog.delete_column(&c.table, &c.column),
        Command::RenameColumn(c) => catalog.rename_column(&c.table, &c.column, &c.new_name),
        Command::ChangeColumnType(c) => {
            catalog.change_column_type(&c.table, &c.column, (&c.data_type).into())
        }
        Command::SetColumnProperty(c) => {
            use crate::server::models::commands::ColumnPropertyUpdate;
            match &c.property {
                ColumnPropertyUpdate::IsHidden(v) => {
                    catalog.set_column_is_hidden(&c.table, &c.column, *v)
                }
                ColumnPropertyUpdate::Description(v) => {
                    catalog.set_column_description(&c.table, &c.column, v.clone())
                }
                ColumnPropertyUpdate::FormatString(v) => {
                    catalog.set_column_format_string(&c.table, &c.column, v.clone())
                }
                ColumnPropertyUpdate::DisplayFolder(v) => {
                    catalog.set_column_display_folder(&c.table, &c.column, v.clone())
                }
                ColumnPropertyUpdate::DataCategory(v) => {
                    catalog.set_column_data_category(&c.table, &c.column, v.clone())
                }
                ColumnPropertyUpdate::SortByColumn(v) => {
                    catalog.set_column_sort_by(&c.table, &c.column, v.clone())
                }
                ColumnPropertyUpdate::SummarizeBy(v) => {
                    catalog.set_column_summarize_by(&c.table, &c.column, v.clone())
                }
            }
        }

        Command::CreateMeasure(c) => catalog.create_measure(
            &c.table,
            &c.name,
            &c.expression,
            c.format_string.clone(),
            c.is_hidden,
            c.description.clone(),
            c.display_folder.clone(),
        ),
        Command::DeleteMeasure(c) => catalog.delete_measure(&c.name),
        Command::RenameMeasure(c) => catalog.rename_measure(&c.name, &c.new_name),
        Command::UpdateMeasureExpression(c) => {
            catalog.update_measure_expression(&c.name, &c.expression)
        }
        Command::SetMeasureFormatString(c) => {
            catalog.set_measure_format_string(&c.name, c.format_string.clone())
        }

        Command::CreateRelationship(c) => catalog.create_relationship(Relationship {
            name: c.name.clone(),
            from_table: c.from_table.clone(),
            from_column: c.from_column.clone(),
            to_table: c.to_table.clone(),
            to_column: c.to_column.clone(),
            active: c.is_active,
            bidirectional: c.bidirectional,
        }),
        Command::DeleteRelationship(c) => catalog.delete_relationship(&c.name),
        Command::UpdateRelationship(c) => {
            catalog.update_relationship(&c.name, c.is_active, c.bidirectional)
        }
    }
}

fn value_to_query_result(value: crate::engine::expressions::Value) -> Result<QueryResult, String> {
    use crate::engine::expressions::Value;
    match value {
        Value::Table(_, df) => {
            let col_names = df.get_column_names();
            let columns: Vec<(String, String)> = col_names
                .iter()
                .map(|name| {
                    let xsd = df
                        .column(name)
                        .map(|s| polars_type_to_xsd(s.dtype()).to_string())
                        .unwrap_or_else(|_| "string".to_string());
                    let field = if name.contains('[') {
                        name.to_string()
                    } else {
                        format!("[{}]", name)
                    };
                    (field, xsd)
                })
                .collect();

            let height = df.height();
            let mut rows: Vec<Vec<Option<String>>> = Vec::with_capacity(height);
            for row_idx in 0..height {
                let row: Vec<Option<String>> = col_names
                    .iter()
                    .map(|col| {
                        df.column(col).ok().and_then(|s| {
                            anyvalue_to_string(s.get(row_idx).unwrap_or(AnyValue::Null))
                        })
                    })
                    .collect();
                rows.push(row);
            }

            Ok(QueryResult { columns, rows })
        }
        Value::Integer(i) => Ok(scalar_result("Value", "integer", Some(i.to_string()))),
        Value::Number(n) => Ok(scalar_result("Value", "double", Some(n.to_string()))),
        Value::String(s) => Ok(scalar_result("Value", "string", Some(s))),
        Value::Boolean(b) => Ok(scalar_result("Value", "boolean", Some(b.to_string()))),
        Value::Blank => Ok(scalar_result("Value", "string", None)),
        Value::DateTime(ms) => {
            let iso = ms_to_iso8601(ms);
            Ok(scalar_result("Value", "dateTime", Some(iso)))
        }
        Value::Series(_) => Err("bare Series result is not serialisable to a rowset".into()),
    }
}

fn ms_to_iso8601(ms: i64) -> String {
    let secs = ms / 1000;
    let millis = (ms % 1000).unsigned_abs() as u32;
    let total_days = secs.div_euclid(86400);
    let time_secs = secs.rem_euclid(86400) as u32;
    let (h, m, s) = (time_secs / 3600, (time_secs % 3600) / 60, time_secs % 60);

    let jd = total_days + 2440588;
    let f = jd + 1401 + (((4 * jd + 274277) / 146097) * 3) / 4 - 38;
    let e = 4 * f + 3;
    let g = (e % 1461) / 4;
    let dg = 5 * g + 2;
    let day = (dg % 153) / 5 + 1;
    let month = (dg / 153 + 2) % 12 + 1;
    let year = e / 1461 - 4716 + (14 - month) / 12;

    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

fn scalar_result(col: &str, xsd: &str, value: Option<String>) -> QueryResult {
    QueryResult {
        columns: vec![(col.to_string(), xsd.to_string())],
        rows: vec![vec![value]],
    }
}

fn anyvalue_to_string(v: AnyValue) -> Option<String> {
    match v {
        AnyValue::Null => None,
        AnyValue::Float64(n) => Some(n.to_string()),
        AnyValue::Float32(n) => Some(n.to_string()),
        AnyValue::Int64(n) => Some(n.to_string()),
        AnyValue::Int32(n) => Some(n.to_string()),
        AnyValue::Int16(n) => Some(n.to_string()),
        AnyValue::Int8(n) => Some(n.to_string()),
        AnyValue::UInt64(n) => Some(n.to_string()),
        AnyValue::UInt32(n) => Some(n.to_string()),
        AnyValue::Boolean(b) => Some(b.to_string()),
        AnyValue::String(s) => Some(s.to_string()),
        AnyValue::StringOwned(s) => Some(s.to_string()),
        other => Some(format!("{other}")),
    }
}

fn polars_type_to_xsd(dt: &DataType) -> &'static str {
    match dt {
        DataType::String => "string",
        DataType::Int64 | DataType::Int32 | DataType::Int16 | DataType::Int8 => "integer",
        DataType::UInt64 | DataType::UInt32 => "unsignedLong",
        DataType::Float64 | DataType::Float32 => "double",
        DataType::Boolean => "boolean",
        DataType::Datetime(_, _) | DataType::Date => "dateTime",
        DataType::Binary => "base64Binary",
        _ => "string",
    }
}

use crate::server::provider::{DatabaseMeta, ServerProvider};

pub struct DaxServerProvider {
    databases: Vec<Arc<dyn DatabaseProvider>>,
}

impl DaxServerProvider {
    pub fn new(databases: Vec<DaxDatabaseProvider>) -> Self {
        Self {
            databases: databases
                .into_iter()
                .map(|d| -> Arc<dyn DatabaseProvider> { Arc::new(d) })
                .collect(),
        }
    }
}

impl ServerProvider for DaxServerProvider {
    fn list_databases(&self) -> Vec<DatabaseMeta> {
        self.databases
            .iter()
            .map(|db| {
                let meta = db.model_meta();
                DatabaseMeta {
                    id: db.name().to_string(),
                    name: db.name().to_string(),
                    last_schema_update: meta.last_schema_update,
                    last_refreshed: meta.last_refreshed,
                }
            })
            .collect()
    }

    fn database(&self, name: &str) -> Option<Arc<dyn DatabaseProvider>> {
        self.databases
            .iter()
            .find(|db| db.name().eq_ignore_ascii_case(name))
            .cloned()
    }
}
