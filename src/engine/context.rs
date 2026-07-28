//Filter/row context
use opendal::blocking::Operator as BlockingOperator;
use polars::datatypes::DataType;
use polars::prelude::*;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::Arc;

use crate::catalog::{Catalog, ColumnMeta};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::ir::operator::LiteralValue;
use crate::engine::row_context::RowContext;

/// A stable fingerprint of "which rows of `table_name` are visible", built
/// from the active predicates for that table only (sorted by column name for
/// determinism, since HashMap iteration order isn't stable). Used to memoize
/// `get_filtered_df` within a single query evaluation
fn filter_fingerprint(table_name: &str, fc: &FilterContext) -> String {
    let mut entries: Vec<(&str, &Vec<FilterPredicate>)> = fc
        .filters
        .iter()
        .filter(|((t, _), _)| t == table_name)
        .map(|((_, col), preds)| (col.as_str(), preds))
        .collect();
    entries.sort_by_key(|(col, _)| *col);

    let mut key = String::with_capacity(table_name.len() + 32);
    key.push_str(table_name);
    for (col, preds) in entries {
        key.push('|');
        key.push_str(col);
        key.push('=');
        key.push_str(&format!("{preds:?}"));
    }
    key
}

#[derive(Clone, Debug)]
pub enum FilterPredicate {
    In(Series),
    NotIn(Series),
    Gt(LiteralValue),
    Lt(LiteralValue),
    Gte(LiteralValue),
    Lte(LiteralValue),
}

#[derive(Clone, Debug)]
pub enum RelationshipOverride {
    Disabled,
    Unidirectional,
    Bidirectional,
}

#[derive(Clone, Debug)]
pub struct FilterContext {
    pub filters: HashMap<(String, String), Vec<FilterPredicate>>,

    /// Columns with a filter placed *directly* by a CALCULATE predicate or
    /// CALCULATE modifier. Never written by `expanded_filter_context`, so it
    /// only reflects direct filters
    pub direct_filters: HashSet<(String, String)>,

    /// Columns whose current `filters` entry was written by `merge_filter`
    /// (relationship propagation), not set directly. `expanded_filter_context`
    /// strips these before each propagation pass so a stale entry from an
    /// outer CALCULATE's expansion can't linger and get intersected with a
    /// freshly-derived one after an inner CALCULATE replaces the direct
    /// filter it was derived from.
    pub derived_filters: HashSet<(String, String)>,

    /// Table-level DataFrame overrides, inserted when FILTER() is used as a
    /// CALCULATE modifier. Column evaluation checks this before applying
    /// predicate filters from `self.filters`.
    pub table_overrides: HashMap<String, DataFrame>,

    /// Columns that are active grouping axes in the nearest enclosing
    /// SUMMARIZECOLUMNS call. Set by SUMMARIZECOLUMNS before evaluating each
    /// row's extension expressions; empty outside that context. Read by
    /// ISINSCOPE to distinguish "grouped by" from merely "filtered".
    pub scoped_columns: HashSet<(String, String)>,

    /// Per-CALCULATE overrides for relationship active/direction state.
    /// Keyed by relationship name. Absent = use catalog defaults.
    pub relationship_overrides: HashMap<String, RelationshipOverride>,

    /// Snapshot of the filter context that existed just before the nearest
    /// enclosing CALCULATE or SUMMARIZECOLUMNS applied its own filters.
    /// ALLSELECTED() restores to this level. None at the top level.
    pub outer_fc: Option<Box<FilterContext>>,
}

impl Default for FilterContext {
    fn default() -> Self {
        Self::new()
    }
}

impl FilterContext {
    pub fn new() -> Self {
        Self {
            filters: HashMap::new(),
            direct_filters: HashSet::new(),
            derived_filters: HashSet::new(),
            table_overrides: HashMap::new(),
            scoped_columns: HashSet::new(),
            relationship_overrides: HashMap::new(),
            outer_fc: None,
        }
    }

    /// Remove all predicate filters and table overrides for `table`.
    /// Used by ALL(table) and REMOVEFILTERS(table).
    pub fn remove_table(&mut self, table: &str) {
        self.filters.retain(|(t, _), _| t != table);
        self.derived_filters.retain(|(t, _)| t != table);
        self.table_overrides.remove(table);
    }

    /// Remove predicate filters for a specific column.
    /// Used by ALL(table[column]).
    pub fn remove_column(&mut self, table: &str, column: &str) {
        let key = (table.to_string(), column.to_string());
        self.filters.remove(&key);
        self.derived_filters.remove(&key);
    }

    /// Remove all predicate filters and table overrides for `table`, except
    /// for the columns listed in `keep`. Used by ALLEXCEPT(table, col1, ...).
    pub fn remove_table_except(&mut self, table: &str, keep: &[(String, String)]) {
        self.filters
            .retain(|(t, c), _| t != table || keep.iter().any(|(kt, kc)| kt == t && kc == c));
        self.derived_filters
            .retain(|(t, c)| t != table || keep.iter().any(|(kt, kc)| kt == t && kc == c));
        self.table_overrides.remove(table);
    }
}

/// Build a boolean mask by ANDing all predicates against `col`.
pub fn build_mask(col: &Series, predicates: &[FilterPredicate]) -> DaxResult<BooleanChunked> {
    let len = col.len();
    let mut acc = BooleanChunked::new("mask".into(), &vec![true; len]);

    for predicate in predicates {
        let mask: BooleanChunked = match predicate {
            FilterPredicate::In(values) => {
                let values = values
                    .cast(col.dtype())
                    .map_err(|e| DaxError::Type(format!("filter cast failed: {e}")))?;
                membership_mask(col, &values, true)?
            }
            FilterPredicate::NotIn(values) => {
                let values = values
                    .cast(col.dtype())
                    .map_err(|e| DaxError::Type(format!("filter cast failed: {e}")))?;
                membership_mask(col, &values, false)?
            }
            FilterPredicate::Gt(lit) => {
                scalar_cmp(col, lit, |a, b| a > b, |a: i64, b| a > b, |a, b| a > b)?
            }
            FilterPredicate::Lt(lit) => {
                scalar_cmp(col, lit, |a, b| a < b, |a: i64, b| a < b, |a, b| a < b)?
            }
            FilterPredicate::Gte(lit) => {
                scalar_cmp(col, lit, |a, b| a >= b, |a: i64, b| a >= b, |a, b| a >= b)?
            }
            FilterPredicate::Lte(lit) => {
                scalar_cmp(col, lit, |a, b| a <= b, |a: i64, b| a <= b, |a, b| a <= b)?
            }
        };
        acc = acc & mask;
    }

    Ok(acc)
}

/// Returns a mask where each element is `include` iff the value is in `values`.
/// Null elements in `col` match null elements in `values` (null-to-null equality).
/// Null elements that don't match (or when include=false) produce false.
pub fn membership_mask(col: &Series, values: &Series, include: bool) -> DaxResult<BooleanChunked> {
    use rustc_hash::FxHashSet;
    let has_null_in_values = values.null_count() > 0;
    match col.dtype() {
        DataType::Int32 => {
            let val_set: FxHashSet<i32> = values
                .i32()
                .expect("values cast to Int32")
                .into_no_null_iter()
                .collect();
            Ok(col
                .i32()
                .expect("dtype matched Int32")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Int64 => {
            let val_set: FxHashSet<i64> = values
                .i64()
                .expect("values cast to Int64")
                .into_no_null_iter()
                .collect();
            Ok(col
                .i64()
                .expect("dtype matched Int64")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Float64 => {
            let val_v: Vec<f64> = values
                .f64()
                .expect("values cast to Float64")
                .into_no_null_iter()
                .collect();
            Ok(col
                .f64()
                .expect("dtype matched Float64")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_v.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::String => {
            let val_set: FxHashSet<&str> = values
                .str()
                .expect("values cast to String")
                .no_null_iter()
                .collect();
            Ok(col
                .str()
                .expect("dtype matched String")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Boolean => {
            let val_set: FxHashSet<bool> = values
                .bool()
                .expect("values cast to Boolean")
                .no_null_iter()
                .collect();
            Ok(col
                .bool()
                .expect("dtype matched Boolean")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Date => {
            let val_set: FxHashSet<i32> = values
                .cast(&DataType::Int32)
                .expect("Date values to i32")
                .i32()
                .expect("cast to Int32 above guarantees this")
                .into_no_null_iter()
                .collect();
            Ok(col
                .cast(&DataType::Int32)
                .expect("Date col to i32")
                .i32()
                .expect("cast to Int32 above guarantees this")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Datetime(_, _) | DataType::Duration(_) | DataType::Time => {
            let val_set: FxHashSet<i64> = values
                .cast(&DataType::Int64)
                .expect("temporal values to i64")
                .i64()
                .expect("cast to Int64 above guarantees this")
                .into_no_null_iter()
                .collect();
            Ok(col
                .cast(&DataType::Int64)
                .expect("temporal col to i64")
                .i64()
                .expect("cast to Int64 above guarantees this")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Int8
        | DataType::Int16
        | DataType::Int128
        | DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::UInt128 => {
            let val_set: FxHashSet<i64> = values
                .cast(&DataType::Int64)
                .expect("int values to i64")
                .i64()
                .expect("cast to Int64 above guarantees this")
                .into_no_null_iter()
                .collect();
            Ok(col
                .cast(&DataType::Int64)
                .expect("int col to i64")
                .i64()
                .expect("cast to Int64 above guarantees this")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_set.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Float16 | DataType::Float32 => {
            let val_v: Vec<f64> = values
                .cast(&DataType::Float64)
                .expect("f32 values to f64")
                .f64()
                .expect("cast to Float64 above guarantees this")
                .into_no_null_iter()
                .collect();
            Ok(col
                .cast(&DataType::Float64)
                .expect("f32 col to f64")
                .f64()
                .expect("cast to Float64 above guarantees this")
                .iter()
                .map(|opt| match opt {
                    Some(v) => val_v.contains(&v) == include,
                    None => include && has_null_in_values,
                })
                .collect())
        }
        DataType::Binary
        | DataType::BinaryOffset
        | DataType::List(_)
        | DataType::Null
        | DataType::Unknown(_) => Err(DaxError::Type(format!(
            "dtype {:?} cannot be used in membership filter",
            col.dtype()
        ))),
        DataType::Categorical(_, _) => Err(DaxError::Type(format!(
            "dtype {:?} cannot be used in membership filter",
            col.dtype()
        ))),
        DataType::Enum(_, _) => Err(DaxError::Type(format!(
            "dtype {:?} cannot be used in membership filter",
            col.dtype()
        ))),
        DataType::Struct(_) => Err(DaxError::Type(format!(
            "dtype {:?} cannot be used in membership filter",
            col.dtype()
        ))),
    }
}

fn scalar_cmp(
    col: &Series,
    lit: &LiteralValue,
    num_cmp: impl Fn(f64, f64) -> bool,
    int_cmp: impl Fn(i64, i64) -> bool,
    str_cmp: impl Fn(&str, &str) -> bool,
) -> DaxResult<BooleanChunked> {
    match (col.dtype(), lit) {
        (DataType::Float64, LiteralValue::Number(n)) => Ok(col
            .f64()
            .expect("dtype matched Float64")
            .into_no_null_iter()
            .map(|v| num_cmp(v, *n))
            .collect()),
        (DataType::Float64, LiteralValue::Integer(i)) => Ok(col
            .f64()
            .expect("dtype matched Float64")
            .into_no_null_iter()
            .map(|v| num_cmp(v, *i as f64))
            .collect()),
        (DataType::Int64, LiteralValue::Number(n)) => Ok(col
            .i64()
            .expect("dtype matched Int64")
            .into_no_null_iter()
            .map(|v| num_cmp(v as f64, *n))
            .collect()),
        (DataType::Int64, LiteralValue::Integer(i)) => Ok(col
            .i64()
            .expect("dtype matched Int64")
            .into_no_null_iter()
            .map(|v| int_cmp(v, *i))
            .collect()),
        (DataType::Int32, LiteralValue::Number(n)) => Ok(col
            .i32()
            .expect("dtype matched Int32")
            .into_no_null_iter()
            .map(|v| num_cmp(v as f64, *n))
            .collect()),
        (DataType::Int32, LiteralValue::Integer(i)) => Ok(col
            .i32()
            .expect("dtype matched Int32")
            .into_no_null_iter()
            .map(|v| int_cmp(v as i64, *i))
            .collect()),
        (DataType::String, LiteralValue::String(s)) => Ok(col
            .str()
            .expect("dtype matched String")
            .no_null_iter()
            .map(|v| str_cmp(v, s.as_str()))
            .collect()),
        (DataType::Datetime(_, _), LiteralValue::DateTime(ms)) => Ok(col
            .datetime()
            .expect("dtype matched Datetime")
            .phys
            .into_no_null_iter()
            .map(|v| int_cmp(v, *ms))
            .collect()),
        _ => Err(DaxError::Type(format!(
            "type mismatch in scalar comparison: column is {:?}, predicate is {:?}",
            col.dtype(),
            lit
        ))),
    }
}

pub struct ExecutionContext {
    /// Loaded tables (Sales, Products, etc.)
    pub tables: HashMap<String, DataFrame>,

    /// Catalog for metadata lookup
    pub catalog: Catalog,

    /// Pre-resolved measure expressions, keyed by measure name.
    /// Populated by MeasureResolver after construction.
    pub resolved_measures: HashMap<String, BoundExprNode>,

    /// IANA timezone name used by NOW() and TODAY() for local-time calculations.
    /// None means the system's local timezone.
    pub timezone: Option<String>,

    /// Storage operator used to read parquet files for all tables.
    pub(crate) datasets_op: Arc<BlockingOperator>,
}

/// Checks that every column declared in the catalog for `table_name`
/// is present in `df`. Returns a named error for the first missing column.
pub fn validate_table_schema(
    df: &DataFrame,
    table_name: &str,
    catalog: &Catalog,
) -> Result<(), String> {
    for (t, col) in catalog.columns.keys() {
        if t != table_name {
            continue;
        }
        df.column(col).map_err(|_| {
            format!(
                "Table '{table_name}': column '{col}' declared in model \
                 but missing from data source"
            )
        })?;
    }
    Ok(())
}

fn load_parquet(op: &BlockingOperator, table: &str, path: &str) -> Result<DataFrame, String> {
    let bytes = op
        .read(path)
        .map_err(|e| format!("Table '{table}': failed to read '{path}': {e}"))?
        .to_bytes()
        .to_vec();
    ParquetReader::new(Cursor::new(bytes))
        .finish()
        .map_err(|e| format!("Table '{table}': failed to parse parquet at '{path}': {e}"))
}

/// Appends one all-null row to `df`, matching SSAS tabular blank-member semantics.
/// Dimension tables (the "one"/toTable side of a relationship) always carry a blank
/// member row so that VALUES() and table iterators include it.
fn append_blank_member_row(df: DataFrame) -> Result<DataFrame, String> {
    let null_cols: Vec<Column> = df
        .columns()
        .iter()
        .map(|col| Column::new_scalar(col.name().clone(), Scalar::null(col.dtype().clone()), 1))
        .collect();
    let null_df = DataFrame::new_infer_height(null_cols)
        .map_err(|e| format!("blank member: failed to build null row: {e}"))?;
    df.vstack(&null_df)
        .map_err(|e| format!("blank member: failed to append null row: {e}"))
}

fn cast_columns(
    df: &mut DataFrame,
    table: &str,
    columns: &HashMap<(String, String), ColumnMeta>,
) -> Result<(), String> {
    for ((t, col), meta) in columns {
        if t != table {
            continue;
        }
        let series = df.column(col)
            .map_err(|_| format!("Table '{table}': column '{col}' declared in catalog but missing from DataFrame"))?
            .as_materialized_series()
            .cast(&meta.dtype)
            .map_err(|e| format!("Table '{table}': failed to cast column '{col}' to {:?}: {e}", meta.dtype))?;
        df.with_column(series.into())
            .map_err(|e| format!("Table '{table}': failed to apply cast for '{col}': {e}"))?;
    }
    Ok(())
}

impl ExecutionContext {
    pub fn try_new(catalog: Catalog, datasets_op: Arc<BlockingOperator>) -> Result<Self, String> {
        let mut tables = HashMap::new();

        for (table, path) in &catalog.data_sources {
            let mut df = load_parquet(&datasets_op, table, path)?;
            validate_table_schema(&df, table, &catalog)?;
            cast_columns(&mut df, table, &catalog.columns)?;
            if catalog.relationships.iter().any(|r| r.to_table == *table) {
                df = append_blank_member_row(df)?;
            }
            tables.insert(table.clone(), df);
        }

        Ok(Self {
            catalog,
            tables,
            resolved_measures: HashMap::new(),
            timezone: None,
            datasets_op,
        })
    }

    /// Reload all parquet tables into a temporary map, then swap atomically.
    /// On any load failure the existing data is left untouched.
    pub fn reload_tables(&mut self) -> Result<(), String> {
        let mut new_tables: HashMap<String, DataFrame> = HashMap::new();

        for (table, path) in &self.catalog.data_sources {
            let mut df = load_parquet(&self.datasets_op, table, path)?;
            validate_table_schema(&df, table, &self.catalog)?;
            cast_columns(&mut df, table, &self.catalog.columns)?;
            if self
                .catalog
                .relationships
                .iter()
                .any(|r| r.from_table == *table)
            {
                df = append_blank_member_row(df)?;
            }
            new_tables.insert(table.clone(), df);
        }

        self.tables = new_tables;
        self.catalog.last_refreshed = chrono::Utc::now();
        Ok(())
    }

    pub fn get_table(&self, name: &str) -> Option<&DataFrame> {
        self.tables.get(name)
    }

    /// Returns a copy of `table_name`'s DataFrame with all current filters applied.
    /// A `table_overrides` entry (from FILTER used as a CALCULATE arg) takes
    /// priority over the predicate-based filters in `fc.filters`.
    ///
    /// Memoized for the lifetime of one query evaluation via `rc`'s filter
    /// cache: without this, every nested CALCULATE/SUMMARIZE evaluation
    /// re-derives the filtered table from the full, unfiltered base table
    /// from scratch, even when an identical (table, filter-state) pair was
    /// just computed a moment ago one level up.
    pub fn get_filtered_df(
        &self,
        table_name: &str,
        fc: &FilterContext,
        rc: &RowContext,
    ) -> DaxResult<DataFrame> {
        let mut df;
        let cache_key;

        if let Some(override_df) = fc.table_overrides.get(table_name) {
            df = override_df.clone();
            cache_key = None;
        } else {
            let key = filter_fingerprint(table_name, fc);
            if let Some(cached) = rc.filter_cache_get(&key) {
                return Ok(cached);
            }
            df = self
                .tables
                .get(table_name)
                .ok_or_else(|| DaxError::UnknownName(format!("unknown table '{table_name}'")))?
                .clone();
            cache_key = Some(key);
        }

        for ((t, col), predicates) in fc.filters.iter() {
            if t != table_name {
                continue;
            }

            let series = df
                .column(col)
                .map_err(|_| {
                    DaxError::Eval(format!("column '{col}' not found in table '{table_name}'"))
                })?
                .as_materialized_series();
            let mask = build_mask(series, predicates)?;
            df = df
                .filter(&mask)
                .map_err(|e| DaxError::Eval(format!("filter failed for '{table_name}': {e}")))?;
        }

        if let Some(key) = cache_key {
            rc.filter_cache_insert(key, df.clone());
        }
        Ok(df)
    }

    pub fn expanded_filter_context(
        &self,
        base: &FilterContext,
        rc: &RowContext,
    ) -> DaxResult<FilterContext> {
        let mut fc = base.clone();

        // Strip any relationship-derived filters inherited from an outer
        // CALCULATE's expansion before re-propagating. Without this, a stale
        // derived entry (e.g. Sales[ProductSK] propagated from an outer
        // Product[Color]="Red") lingers after an inner CALCULATE replaces the
        // direct filter it came from, and merge_filter below intersects the
        // stale entry with the freshly-derived one instead of replacing it.
        for key in std::mem::take(&mut fc.derived_filters) {
            fc.filters.remove(&key);
        }

        let mut changed = true;

        let relationships = self.catalog.relationships.clone();

        while changed {
            changed = false;

            for rel in &relationships {
                let bidir = match fc.relationship_overrides.get(&rel.name) {
                    Some(RelationshipOverride::Disabled) => continue,
                    Some(RelationshipOverride::Unidirectional) => false,
                    Some(RelationshipOverride::Bidirectional) => true,
                    None => {
                        if !rel.active {
                            continue;
                        }
                        rel.bidirectional
                    }
                };

                // FORWARD: to (dim) → from (fact)
                let to_has_filters = fc.filters.keys().any(|(t, _)| t == &rel.to_table)
                    || fc.table_overrides.contains_key(&rel.to_table);
                if to_has_filters {
                    let filtered_to = self.get_filtered_df(&rel.to_table, &fc, rc)?;
                    let join_values = filtered_to
                        .column(&rel.to_column)
                        .map_err(|_| {
                            DaxError::Eval(format!(
                                "join column '{}' not found in '{}'",
                                rel.to_column, rel.to_table
                            ))
                        })?
                        .as_materialized_series()
                        .unique()
                        .map_err(|e| {
                            DaxError::Eval(format!(
                                "unique failed for '{}.{}': {e}",
                                rel.to_table, rel.to_column
                            ))
                        })?;

                    let from_key = (rel.from_table.clone(), rel.from_column.clone());
                    if self.merge_filter(&mut fc, from_key, join_values)? {
                        changed = true;
                    }
                }

                // REVERSE: from (fact) → to (dim) (if bidirectional)
                if bidir {
                    let from_has_filters = fc.filters.keys().any(|(t, _)| t == &rel.from_table);
                    if from_has_filters {
                        let filtered_from = self.get_filtered_df(&rel.from_table, &fc, rc)?;
                        let join_values = filtered_from
                            .column(&rel.from_column)
                            .map_err(|_| {
                                DaxError::Eval(format!(
                                    "join column '{}' not found in '{}'",
                                    rel.from_column, rel.from_table
                                ))
                            })?
                            .as_materialized_series()
                            .unique()
                            .map_err(|e| {
                                DaxError::Eval(format!(
                                    "unique failed for '{}.{}': {e}",
                                    rel.from_table, rel.from_column
                                ))
                            })?;

                        let to_key = (rel.to_table.clone(), rel.to_column.clone());
                        if self.merge_filter(&mut fc, to_key, join_values)? {
                            changed = true;
                        }
                    }
                }
            }
        }

        Ok(fc)
    }

    fn merge_filter(
        &self,
        fc: &mut FilterContext,
        key: (String, String),
        new_values: Series,
    ) -> DaxResult<bool> {
        let col_meta = self
            .catalog
            .columns
            .get(&(key.0.clone(), key.1.clone()))
            .ok_or_else(|| {
                DaxError::Eval(format!("no catalog metadata for '{}.{}'", key.0, key.1))
            })?;

        let dtype = col_meta.dtype.clone();
        let new_values = new_values
            .cast(&dtype)
            .map_err(|e| DaxError::Eval(format!("cast failed for '{}.{}': {e}", key.0, key.1)))?;

        fc.derived_filters.insert(key.clone());
        let entry = fc.filters.entry(key).or_default();

        if let Some(FilterPredicate::In(existing)) = entry
            .iter_mut()
            .find(|p| matches!(p, FilterPredicate::In(_)))
        {
            let existing_cast = existing.cast(&dtype).map_err(|e| {
                DaxError::Eval(format!("cast of existing filter values failed: {e}"))
            })?;

            let intersected = match dtype {
                DataType::Int32 => {
                    let ev: Vec<i32> = existing_cast
                        .i32()
                        .expect("dtype matched Int32")
                        .into_no_null_iter()
                        .collect();
                    let nv: HashSet<i32> = new_values
                        .i32()
                        .expect("cast to Int32")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<i32> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                }
                DataType::Int64 => {
                    let ev: Vec<i64> = existing_cast
                        .i64()
                        .expect("dtype matched Int64")
                        .into_no_null_iter()
                        .collect();
                    let nv: HashSet<i64> = new_values
                        .i64()
                        .expect("cast to Int64")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<i64> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                }
                DataType::Float64 => {
                    // f64 is not Hash; linear scan is acceptable for float filter values
                    let ev: Vec<f64> = existing_cast
                        .f64()
                        .expect("dtype matched Float64")
                        .into_no_null_iter()
                        .collect();
                    let nv: Vec<f64> = new_values
                        .f64()
                        .expect("cast to Float64")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<f64> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                }
                DataType::String => {
                    let ev: Vec<String> = existing_cast
                        .str()
                        .expect("dtype matched String")
                        .no_null_iter()
                        .map(|s| s.to_string())
                        .collect();
                    let nv: HashSet<String> = new_values
                        .str()
                        .expect("cast to String")
                        .no_null_iter()
                        .map(|s| s.to_string())
                        .collect();
                    let filtered: Vec<String> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                }
                DataType::Date => {
                    let ev: Vec<i32> = existing_cast
                        .cast(&DataType::Int32)
                        .expect("Date to i32")
                        .i32()
                        .expect("cast to Int32 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let nv: HashSet<i32> = new_values
                        .cast(&DataType::Int32)
                        .expect("Date to i32")
                        .i32()
                        .expect("cast to Int32 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<i32> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                        .cast(&DataType::Date)
                        .expect("i32 back to Date")
                }
                DataType::Datetime(_, _) | DataType::Duration(_) | DataType::Time => {
                    let ev: Vec<i64> = existing_cast
                        .cast(&DataType::Int64)
                        .expect("temporal to i64")
                        .i64()
                        .expect("cast to Int64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let nv: HashSet<i64> = new_values
                        .cast(&DataType::Int64)
                        .expect("temporal to i64")
                        .i64()
                        .expect("cast to Int64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<i64> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                        .cast(&dtype)
                        .expect("i64 back to temporal")
                }
                DataType::Boolean => {
                    let ev: Vec<bool> = existing_cast
                        .bool()
                        .expect("dtype matched Boolean")
                        .no_null_iter()
                        .collect();
                    let nv: HashSet<bool> = new_values
                        .bool()
                        .expect("cast to Boolean")
                        .no_null_iter()
                        .collect();
                    let filtered: Vec<bool> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                }
                DataType::Int8
                | DataType::Int16
                | DataType::Int128
                | DataType::UInt8
                | DataType::UInt16
                | DataType::UInt32
                | DataType::UInt64
                | DataType::UInt128 => {
                    let ev: Vec<i64> = existing_cast
                        .cast(&DataType::Int64)
                        .expect("int to i64")
                        .i64()
                        .expect("cast to Int64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let nv: HashSet<i64> = new_values
                        .cast(&DataType::Int64)
                        .expect("int to i64")
                        .i64()
                        .expect("cast to Int64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<i64> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                        .cast(&dtype)
                        .expect("i64 back to int type")
                }
                DataType::Float16 | DataType::Float32 => {
                    let ev: Vec<f64> = existing_cast
                        .cast(&DataType::Float64)
                        .expect("f32 to f64")
                        .f64()
                        .expect("cast to Float64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let nv: Vec<f64> = new_values
                        .cast(&DataType::Float64)
                        .expect("f32 to f64")
                        .f64()
                        .expect("cast to Float64 above guarantees this")
                        .into_no_null_iter()
                        .collect();
                    let filtered: Vec<f64> = ev.into_iter().filter(|v| nv.contains(v)).collect();
                    Series::new("filter".into(), filtered)
                        .cast(&DataType::Float32)
                        .expect("f64 back to f32")
                }
                DataType::Binary
                | DataType::BinaryOffset
                | DataType::List(_)
                | DataType::Null
                | DataType::Unknown(_) => {
                    return Err(DaxError::Type(format!(
                        "dtype {dtype:?} cannot be used as a relationship join key"
                    )))
                }
                DataType::Categorical(_, _) => {
                    return Err(DaxError::Type(format!(
                        "dtype {dtype:?} cannot be used as a relationship join key"
                    )))
                }
                DataType::Enum(_, _) => {
                    return Err(DaxError::Type(format!(
                        "dtype {dtype:?} cannot be used as a relationship join key"
                    )))
                }
                DataType::Struct(_) => {
                    return Err(DaxError::Type(format!(
                        "dtype {dtype:?} cannot be used as a relationship join key"
                    )))
                }
            };

            let changed = intersected.len() != existing.len();
            *existing = intersected;
            Ok(changed)
        } else {
            entry.push(FilterPredicate::In(new_values));
            Ok(true)
        }
    }
}
