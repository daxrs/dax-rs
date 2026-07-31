use polars::prelude::{AnyValue, DataFrame, DataType, NamedFrom, Series};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::engine::context::ExecutionContext;
use crate::engine::context_functions::JoinStep;
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::table_col::TableCol;

#[derive(Debug, Clone)]
pub enum ScalarValue {
    Integer(i64),
    Number(f64),
    Text(String),
    Boolean(bool),
    /// Milliseconds since Unix epoch (UTC), matching Polars Datetime(Milliseconds).
    DateTime(i64),
    Blank,
}

impl ScalarValue {
    pub fn to_series(&self, name: &str) -> Series {
        match self {
            ScalarValue::Integer(i) => Series::new(name.into(), &[*i]),
            ScalarValue::Number(f) => Series::new(name.into(), &[*f]),
            ScalarValue::Text(s) => Series::new(name.into(), &[s.as_str()]),
            ScalarValue::Boolean(b) => Series::new(name.into(), &[*b]),
            ScalarValue::DateTime(ms) => Series::new(name.into(), &[*ms]),
            ScalarValue::Blank => Series::new_null(name.into(), 1),
        }
    }
}

impl<'a> TryFrom<AnyValue<'a>> for ScalarValue {
    type Error = DaxError;

    fn try_from(av: AnyValue<'a>) -> Result<Self, Self::Error> {
        match av {
            AnyValue::Int64(i) => Ok(ScalarValue::Integer(i)),
            AnyValue::Int32(i) => Ok(ScalarValue::Integer(i as i64)),
            AnyValue::Float64(f) => Ok(ScalarValue::Number(f)),
            AnyValue::Float32(f) => Ok(ScalarValue::Number(f as f64)),
            AnyValue::Boolean(b) => Ok(ScalarValue::Boolean(b)),
            AnyValue::String(s) => Ok(ScalarValue::Text(s.to_string())),
            AnyValue::Null => Ok(ScalarValue::Blank),
            AnyValue::Datetime(ms, _, _) => Ok(ScalarValue::DateTime(ms)),
            other => Err(DaxError::Type(format!(
                "unsupported AnyValue in row context: {other:?}"
            ))),
        }
    }
}

/// Cache of resolved relationship-graph join paths, keyed by (source, target) table names.
type JoinPathCache = Rc<RefCell<HashMap<(String, String), Vec<JoinStep>>>>;

/// Evaluation-time context for row iteration (SUMX, FILTER condition, etc.).
///
/// Two distinct layers:
/// - `frames`: a stack of per-row column bindings pushed/popped by X functions.
///   Each frame maps `(table, column) → ScalarValue` for the current row.
/// - `table_scope`: a table-level scope for vectorized FILTER condition evaluation.
///   Maps table name → the DataFrame being iterated (so column lookups read from
///   the scoped DataFrame rather than the global table store).
#[derive(Debug, Clone)]
pub struct RowContext {
    frames: Vec<HashMap<(String, String), ScalarValue>>,
    pub table_scope: HashMap<String, DataFrame>,
    subtotal_cols: HashSet<(String, String)>,

    /// The rows belonging to the current GROUPBY group; read by CURRENTGROUP().
    pub current_group: Option<(String, DataFrame)>,

    /// VAR bindings in scope, evaluated once and cached
    var_scope: HashMap<String, Value>,

    /// Per-query cache for `ExecutionContext::get_filtered_df`, keyed by a
    /// fingerprint of (table, active predicates for that table). Shared (via
    /// `Rc`) across every clone of this RowContext for the lifetime of one
    /// top-level query evaluation, so repeated filtering of the same table
    /// under the same effective filter set, reuses the already-filtered
    /// DataFrame instead of re-scanning the full base table from scratch.
    filter_cache: Rc<RefCell<HashMap<String, DataFrame>>>,

    /// Per-query cache for `try_find_join_path`'s relationship-graph BFS,
    /// keyed by (source, target) table names. The result only depends on
    /// `ctx.catalog.relationships`, which is immutable for the duration of a
    /// query.
    join_path_cache: JoinPathCache,
}

impl RowContext {
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            table_scope: HashMap::new(),
            subtotal_cols: HashSet::new(),
            current_group: None,
            var_scope: HashMap::new(),
            filter_cache: Rc::new(RefCell::new(HashMap::new())),
            join_path_cache: Rc::new(RefCell::new(HashMap::new())),
        }
    }

    /// Look up a VAR binding's already-evaluated value.
    pub fn get_var(&self, name: &str) -> Option<&Value> {
        self.var_scope.get(name)
    }

    /// Return a clone of this context with `name → value` added to the var scope.
    pub fn with_var(&self, name: String, value: Value) -> Self {
        let mut new = self.clone();
        new.var_scope.insert(name, value);
        new
    }

    /// Look up a cached filtered DataFrame by fingerprint.
    pub fn filter_cache_get(&self, key: &str) -> Option<DataFrame> {
        self.filter_cache.borrow().get(key).cloned()
    }

    /// Cache a filtered DataFrame under a fingerprint. Shared with every
    /// clone of this RowContext, so this is visible to sibling branches of
    /// evaluation within the same query, not just this call chain.
    pub fn filter_cache_insert(&self, key: String, df: DataFrame) {
        self.filter_cache.borrow_mut().insert(key, df);
    }

    /// Look up a cached relationship-graph join path by (source, target).
    pub fn join_path_cache_get(&self, source: &str, target: &str) -> Option<Vec<JoinStep>> {
        self.join_path_cache
            .borrow()
            .get(&(source.to_string(), target.to_string()))
            .cloned()
    }

    /// Cache a join path under (source, target).
    pub fn join_path_cache_insert(&self, source: &str, target: &str, path: Vec<JoinStep>) {
        self.join_path_cache
            .borrow_mut()
            .insert((source.to_string(), target.to_string()), path);
    }

    pub fn is_subtotal(&self, table: &str, col: &str) -> bool {
        self.subtotal_cols
            .contains(&(table.to_string(), col.to_string()))
    }

    pub fn with_subtotal_cols(&self, cols: HashSet<(String, String)>) -> Self {
        let mut new = self.clone();
        new.subtotal_cols = cols;
        new
    }

    /// Look up a column value from the innermost matching row frame.
    /// Tries the bare column name first, then the qualified "Table[Column]" form
    pub fn lookup(&self, table: &str, column: &str) -> Option<&ScalarValue> {
        let key = (table.to_string(), column.to_string());
        let qualified_key = (table.to_string(), TableCol::new(table, column).to_string());
        for frame in self.frames.iter().rev() {
            if let Some(v) = frame.get(&key) {
                return Some(v);
            }
            if let Some(v) = frame.get(&qualified_key) {
                return Some(v);
            }
        }
        None
    }

    /// Look up a column value from an outer row frame, `levels` steps out
    /// from the innermost (current) matching one — DAX's EARLIER(column,
    /// levels). `levels` is 1-based per DAX's own convention (1 = the first
    /// ancestor frame)
    pub fn earlier(&self, table: &str, column: &str, levels: usize) -> Option<&ScalarValue> {
        let key = (table.to_string(), column.to_string());
        let qualified_key = (table.to_string(), TableCol::new(table, column).to_string());
        self.frames
            .iter()
            .rev()
            .filter_map(|frame| frame.get(&key).or_else(|| frame.get(&qualified_key)))
            .nth(levels)
    }

    /// Look up a column value from the outermost ancestor frame — DAX's
    /// EARLIEST(column).
    pub fn earliest(&self, table: &str, column: &str) -> Option<&ScalarValue> {
        let key = (table.to_string(), column.to_string());
        let qualified_key = (table.to_string(), TableCol::new(table, column).to_string());
        let mut matches = self
            .frames
            .iter()
            .filter_map(|frame| frame.get(&key).or_else(|| frame.get(&qualified_key)));
        let outermost = matches.next()?;
        matches.next().is_some().then_some(outermost)
    }

    /// Return a clone of this context with a new row frame pushed on top.
    pub fn with_frame(&self, frame: HashMap<(String, String), ScalarValue>) -> Self {
        let mut new = self.clone();
        new.frames.push(frame);
        new
    }

    /// Returns the set of table names present in any row frame.
    pub fn frame_tables(&self) -> std::collections::HashSet<String> {
        self.frames
            .iter()
            .flat_map(|f| f.keys().map(|(t, _)| t.clone()))
            .collect()
    }

    /// Search row frames for a bare measure name, matching plain name or "Table[name]" suffix.
    pub fn lookup_measure(&self, name: &str) -> Option<&ScalarValue> {
        let suffix = format!("[{}]", name);
        for frame in self.frames.iter().rev() {
            for ((_, col_name), val) in frame {
                if col_name == name || col_name.ends_with(&suffix) {
                    return Some(val);
                }
            }
        }
        None
    }

    /// Return a clone of this context with `name → df` added to the table scope.
    pub fn with_table_scope(&self, name: String, df: DataFrame) -> Self {
        let mut new = self.clone();
        new.table_scope.insert(name, df);
        new
    }

    /// Return a clone of this context with current_group set (for GROUPBY / CURRENTGROUP()).
    pub fn with_current_group(&self, name: String, df: DataFrame) -> Self {
        let mut new = self.clone();
        new.current_group = Some((name, df));
        new
    }
}

impl Default for RowContext {
    fn default() -> Self {
        Self::new()
    }
}

fn collect_referenced_columns(
    expr: &BoundExprNode,
    table_name: &str,
    ctx: &ExecutionContext,
    visited_measures: &mut HashSet<String>,
    out: &mut HashSet<String>,
) -> bool {
    match expr {
        BoundExprNode::Literal(_) | BoundExprNode::Table(_) | BoundExprNode::VarRef(_) => true,
        BoundExprNode::Column(c) => {
            if c.table == table_name {
                out.insert(c.column.clone());
            }
            true
        }
        BoundExprNode::Measure(m) => {
            match ctx.resolved_measures.get(&m.name) {
                Some(_) if !visited_measures.insert(m.name.clone()) => true, // cycle guard
                Some(tree) => {
                    collect_referenced_columns(tree, table_name, ctx, visited_measures, out)
                }
                None => false,
            }
        }
        BoundExprNode::UnaryOp(op) => {
            collect_referenced_columns(&op.expr, table_name, ctx, visited_measures, out)
        }
        BoundExprNode::BinaryOp(op) => {
            collect_referenced_columns(&op.left, table_name, ctx, visited_measures, out)
                && collect_referenced_columns(&op.right, table_name, ctx, visited_measures, out)
        }
        BoundExprNode::Function(f) => {
            let name = f.name.to_ascii_uppercase();
            if name == "RELATED" || name == "RELATEDTABLE" {
                return false;
            }
            f.args
                .iter()
                .all(|a| collect_referenced_columns(a, table_name, ctx, visited_measures, out))
        }
        BoundExprNode::Calculate(c) => {
            collect_referenced_columns(&c.expression, table_name, ctx, visited_measures, out)
        }
        BoundExprNode::Summarize(s) => {
            collect_referenced_columns(&s.table, table_name, ctx, visited_measures, out)
                && s.group_by
                    .iter()
                    .all(|g| collect_referenced_columns(g, table_name, ctx, visited_measures, out))
                && s.rollup_cols.iter().all(|(c, _)| {
                    collect_referenced_columns(c, table_name, ctx, visited_measures, out)
                })
                && s.extensions.iter().all(|(_, e)| {
                    collect_referenced_columns(e, table_name, ctx, visited_measures, out)
                })
        }
        BoundExprNode::SummarizeColumns(s) => {
            s.group_by_cols
                .iter()
                .all(|g| collect_referenced_columns(g, table_name, ctx, visited_measures, out))
                && s.rollup_groups.iter().flatten().all(|(cols, _)| {
                    cols.iter().all(|c| {
                        collect_referenced_columns(c, table_name, ctx, visited_measures, out)
                    })
                })
                && s.filters
                    .iter()
                    .all(|f| collect_referenced_columns(f, table_name, ctx, visited_measures, out))
                && s.extensions.iter().all(|(_, e, _)| {
                    collect_referenced_columns(e, table_name, ctx, visited_measures, out)
                })
        }
        BoundExprNode::TableConstructor(rows) => rows.iter().all(|row| {
            row.iter()
                .all(|e| collect_referenced_columns(e, table_name, ctx, visited_measures, out))
        }),
        BoundExprNode::Var(v) => {
            v.bindings
                .iter()
                .all(|(_, e)| collect_referenced_columns(e, table_name, ctx, visited_measures, out))
                && collect_referenced_columns(&v.result, table_name, ctx, visited_measures, out)
        }
    }
}

fn referenced_columns_for(
    exprs: &[&BoundExprNode],
    table_name: &str,
    ctx: &ExecutionContext,
) -> Option<HashSet<String>> {
    let mut out = HashSet::new();
    let mut visited = HashSet::new();
    let safe = exprs
        .iter()
        .all(|e| collect_referenced_columns(e, table_name, ctx, &mut visited, &mut out));
    safe.then_some(out)
}

/// Does this expression tree (transitively, through measures) require a
/// genuine per-row RowContext frame to evaluate correctly — currently true
/// only for EARLIER/EARLIEST. Vectorized table-shaping functions (FILTER,
/// etc.) use this to decide whether to fall back to a slower row-by-row
/// evaluation that actually pushes a frame; a vectorized `table_scope`
/// evaluation never pushes one, so EARLIER/EARLIEST can't see anything
/// through it.
pub(crate) fn needs_row_context(expr: &BoundExprNode, ctx: &ExecutionContext) -> bool {
    fn walk(
        expr: &BoundExprNode,
        ctx: &ExecutionContext,
        visited_measures: &mut HashSet<String>,
    ) -> bool {
        match expr {
            BoundExprNode::Literal(_)
            | BoundExprNode::Table(_)
            | BoundExprNode::VarRef(_)
            | BoundExprNode::Column(_) => false,
            BoundExprNode::Measure(m) => {
                if !visited_measures.insert(m.name.clone()) {
                    return false; // cycle guard
                }
                match ctx.resolved_measures.get(&m.name) {
                    Some(tree) => walk(tree, ctx, visited_measures),
                    None => false,
                }
            }
            BoundExprNode::UnaryOp(op) => walk(&op.expr, ctx, visited_measures),
            BoundExprNode::BinaryOp(op) => {
                walk(&op.left, ctx, visited_measures) || walk(&op.right, ctx, visited_measures)
            }
            BoundExprNode::Function(f) => {
                let name = f.name.to_ascii_uppercase();
                name == "EARLIER"
                    || name == "EARLIEST"
                    || f.args.iter().any(|a| walk(a, ctx, visited_measures))
            }
            BoundExprNode::Calculate(c) => walk(&c.expression, ctx, visited_measures),
            BoundExprNode::Summarize(s) => {
                walk(&s.table, ctx, visited_measures)
                    || s.group_by.iter().any(|g| walk(g, ctx, visited_measures))
                    || s.rollup_cols
                        .iter()
                        .any(|(c, _)| walk(c, ctx, visited_measures))
                    || s.extensions
                        .iter()
                        .any(|(_, e)| walk(e, ctx, visited_measures))
            }
            BoundExprNode::SummarizeColumns(s) => {
                s.group_by_cols
                    .iter()
                    .any(|g| walk(g, ctx, visited_measures))
                    || s.rollup_groups
                        .iter()
                        .flatten()
                        .any(|(cols, _)| cols.iter().any(|c| walk(c, ctx, visited_measures)))
                    || s.filters.iter().any(|f| walk(f, ctx, visited_measures))
                    || s.extensions
                        .iter()
                        .any(|(_, e, _)| walk(e, ctx, visited_measures))
            }
            BoundExprNode::TableConstructor(rows) => rows
                .iter()
                .any(|row| row.iter().any(|e| walk(e, ctx, visited_measures))),
            BoundExprNode::Var(v) => {
                v.bindings
                    .iter()
                    .any(|(_, e)| walk(e, ctx, visited_measures))
                    || walk(&v.result, ctx, visited_measures)
            }
        }
    }
    walk(expr, ctx, &mut HashSet::new())
}

enum ColumnCursor<'a> {
    Int64(Box<dyn Iterator<Item = Option<i64>> + 'a>),
    Float64(Box<dyn Iterator<Item = Option<f64>> + 'a>),
    Boolean(Box<dyn Iterator<Item = Option<bool>> + 'a>),
    Text(Box<dyn Iterator<Item = Option<&'a str>> + 'a>),
    DateTime(Box<dyn Iterator<Item = Option<i64>> + 'a>),
}

impl ColumnCursor<'_> {
    fn next_scalar(&mut self) -> ScalarValue {
        match self {
            ColumnCursor::Int64(it) => it
                .next()
                .flatten()
                .map(ScalarValue::Integer)
                .unwrap_or(ScalarValue::Blank),
            ColumnCursor::Float64(it) => it
                .next()
                .flatten()
                .map(ScalarValue::Number)
                .unwrap_or(ScalarValue::Blank),
            ColumnCursor::Boolean(it) => it
                .next()
                .flatten()
                .map(ScalarValue::Boolean)
                .unwrap_or(ScalarValue::Blank),
            ColumnCursor::Text(it) => it
                .next()
                .flatten()
                .map(|s| ScalarValue::Text(s.to_string()))
                .unwrap_or(ScalarValue::Blank),
            ColumnCursor::DateTime(it) => it
                .next()
                .flatten()
                .map(ScalarValue::DateTime)
                .unwrap_or(ScalarValue::Blank),
        }
    }
}

fn build_column_cursor(series: &Series) -> DaxResult<ColumnCursor<'_>> {
    match series.dtype() {
        DataType::Int64 => Ok(ColumnCursor::Int64(Box::new(
            series.i64().expect("dtype matched Int64").iter(),
        ))),
        DataType::Int32 => Ok(ColumnCursor::Int64(Box::new(
            series
                .i32()
                .expect("dtype matched Int32")
                .iter()
                .map(|o| o.map(|v| v as i64)),
        ))),
        DataType::Float64 => Ok(ColumnCursor::Float64(Box::new(
            series.f64().expect("dtype matched Float64").iter(),
        ))),
        DataType::Float32 => Ok(ColumnCursor::Float64(Box::new(
            series
                .f32()
                .expect("dtype matched Float32")
                .iter()
                .map(|o| o.map(|v| v as f64)),
        ))),
        DataType::Boolean => Ok(ColumnCursor::Boolean(Box::new(
            series.bool().expect("dtype matched Boolean").iter(),
        ))),
        DataType::String => Ok(ColumnCursor::Text(Box::new(
            series.str().expect("dtype matched String").iter(),
        ))),
        DataType::Datetime(_, _) => Ok(ColumnCursor::DateTime(Box::new(
            series
                .datetime()
                .expect("dtype matched Datetime")
                .phys
                .iter(),
        ))),
        other @ (DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::UInt128
        | DataType::Int8
        | DataType::Int16
        | DataType::Int128
        | DataType::Float16
        | DataType::Binary
        | DataType::BinaryOffset
        | DataType::Date
        | DataType::Duration(_)
        | DataType::Time
        | DataType::List(_)
        | DataType::Null
        | DataType::Categorical(_, _)
        | DataType::Enum(_, _)
        | DataType::Struct(_)
        | DataType::Unknown(_)) => Err(DaxError::Type(format!(
            "unsupported dtype in row context: {other:?}"
        ))),
    }
}

pub struct RowFrameCursor<'a> {
    table_name: String,
    columns: Vec<(String, ColumnCursor<'a>)>,
}

impl<'a> RowFrameCursor<'a> {
    pub fn new(
        table_name: &str,
        df: &'a DataFrame,
        exprs: &[&BoundExprNode],
        ctx: &ExecutionContext,
    ) -> DaxResult<Self> {
        let columns = match referenced_columns_for(exprs, table_name, ctx) {
            Some(names) => {
                let mut resolved = Vec::with_capacity(names.len());
                for name in names {
                    let qualified = TableCol::new(table_name, &name).to_string();
                    let stored_name = if df.column(&qualified).is_ok() {
                        qualified
                    } else {
                        name
                    };
                    let series = df
                        .column(&stored_name)
                        .map_err(|e| DaxError::Eval(format!("row context: {e}")))?
                        .as_materialized_series();
                    resolved.push((stored_name, build_column_cursor(series)?));
                }
                resolved
            }
            None => df
                .columns()
                .iter()
                .map(|col| {
                    let series = col.as_materialized_series();
                    Ok((col.name().to_string(), build_column_cursor(series)?))
                })
                .collect::<DaxResult<Vec<_>>>()?,
        };
        Ok(Self { table_name: table_name.to_string(), columns })
    }

    pub fn next_frame(&mut self) -> HashMap<(String, String), ScalarValue> {
        let mut frame = HashMap::with_capacity(self.columns.len());
        for (name, cursor) in &mut self.columns {
            frame.insert(
                (self.table_name.clone(), name.clone()),
                cursor.next_scalar(),
            );
        }
        frame
    }
}
