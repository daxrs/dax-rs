use polars::prelude::{
    BooleanChunked, ChunkCompareEq, ChunkFull, DataFrame, DataFrameJoinOps, JoinType, Series,
};

use crate::engine::context::{ExecutionContext, FilterContext, FilterPredicate};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::row_context::{RowContext, ScalarValue};

// TREATAS(table_expr, col1, col2, ...) --------------------------------------

pub fn treatas_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "TREATAS requires at least two arguments: a table expression and one column reference"
                .into(),
        ));
    }

    let mut it = args.into_iter();
    let table_expr = it
        .next()
        .expect("args.len() >= 2 guarantees a first element");
    let target_args: Vec<BoundExprNode> = it.collect();

    let source_df = match eval(table_expr, fc, rc)? {
        Value::Table(_, df) => df,
        other => {
            return Err(DaxError::Type(format!(
                "TREATAS: first argument must evaluate to a table, got {other:?}"
            )))
        }
    };

    let source_cols = source_df.columns();

    if target_args.len() != source_cols.len() {
        return Err(DaxError::InvalidArgument(format!(
            "TREATAS: {} target column(s) specified but source table has {} column(s); counts must match",
            target_args.len(),
            source_cols.len()
        )));
    }

    let target_table = match &target_args[0] {
        BoundExprNode::Column(c) => c.table.clone(),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "TREATAS: arguments after the table must be column references, got {other:?}"
            )))
        }
    };

    let mut renamed: Vec<Series> = Vec::with_capacity(target_args.len());
    for (i, col_arg) in target_args.iter().enumerate() {
        let col = match col_arg {
            BoundExprNode::Column(c) => c,
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "TREATAS: arguments after the table must be column references, got {other:?}"
                )))
            }
        };
        let dtype = ctx
            .catalog
            .columns
            .get(&(col.table.clone(), col.column.clone()))
            .map(|m| m.dtype.clone())
            .unwrap_or_else(|| source_cols[i].dtype().clone());
        let mut series = source_cols[i]
            .as_materialized_series()
            .cast(&dtype)
            .map_err(|e| DaxError::Eval(format!("TREATAS: cast failed: {e}")))?;
        series.rename(col.column.clone().into());
        renamed.push(series);
    }

    let result_df = DataFrame::new_infer_height(renamed.into_iter().map(|s| s.into()).collect())
        .map_err(|e| DaxError::Eval(format!("TREATAS: failed to build result DataFrame: {e}")))?;

    Ok(Value::Table(target_table, result_df))
}

pub fn apply_treatas_modifier(
    args: Vec<BoundExprNode>,
    fc: &mut FilterContext,
    ctx: &ExecutionContext,
    current_fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<()> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "TREATAS requires at least two arguments: a table expression and one column reference"
                .into(),
        ));
    }

    let mut it = args.into_iter();
    let table_expr = it
        .next()
        .expect("args.len() >= 2 guarantees a first element");
    let target_args: Vec<BoundExprNode> = it.collect();

    let source_df = match eval(table_expr, current_fc, rc)? {
        Value::Table(_, df) => df,
        other => {
            return Err(DaxError::Type(format!(
                "TREATAS: first argument must evaluate to a table, got {other:?}"
            )))
        }
    };

    let source_cols = source_df.columns();

    if target_args.len() != source_cols.len() {
        return Err(DaxError::InvalidArgument(format!(
            "TREATAS: {} target column(s) specified but source table has {} column(s); counts must match",
            target_args.len(),
            source_cols.len()
        )));
    }

    for (i, col_arg) in target_args.iter().enumerate() {
        let col = match col_arg {
            BoundExprNode::Column(c) => c,
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "TREATAS: arguments after the table must be column references, got {other:?}"
                )))
            }
        };

        let values = source_cols[i]
            .as_materialized_series()
            .unique()
            .map_err(|e| DaxError::Eval(format!("TREATAS: unique failed: {e}")))?;

        let dtype = ctx
            .catalog
            .columns
            .get(&(col.table.clone(), col.column.clone()))
            .map(|m| m.dtype.clone())
            .unwrap_or_else(|| values.dtype().clone());

        let values = values
            .cast(&dtype)
            .map_err(|e| DaxError::Eval(format!("TREATAS: cast failed: {e}")))?;

        fc.filters
            .entry((col.table.clone(), col.column.clone()))
            .or_default()
            .push(FilterPredicate::In(values));
    }

    Ok(())
}

// BFS join-path finder ------------------------------------------------------

#[derive(Debug, Clone)]
pub struct JoinStep {
    pub left_table: String,
    pub left_col: String,
    pub right_table: String,
    pub right_col: String,
}

/// Cached wrapper around the relationship-graph BFS: the result only depends
/// on `ctx.catalog.relationships`, which doesn't change during a query, so
/// repeated lookups of the same (source, target) pair — e.g. every measure
/// call that joins the same two tables — reuse the cached path instead of
/// re-walking the graph.
pub(crate) fn try_find_join_path(
    ctx: &ExecutionContext,
    rc: &RowContext,
    source: &str,
    target: &str,
) -> Option<Vec<JoinStep>> {
    if let Some(cached) = rc.join_path_cache_get(source, target) {
        return Some(cached);
    }
    let path = try_find_join_path_uncached(ctx, source, target)?;
    rc.join_path_cache_insert(source, target, path.clone());
    Some(path)
}

fn try_find_join_path_uncached(
    ctx: &ExecutionContext,
    source: &str,
    target: &str,
) -> Option<Vec<JoinStep>> {
    use std::collections::{HashSet, VecDeque};

    if source == target {
        return Some(vec![]);
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<(String, Vec<JoinStep>)> = VecDeque::new();
    queue.push_back((source.to_string(), vec![]));
    visited.insert(source.to_string());

    while let Some((current, path)) = queue.pop_front() {
        for rel in &ctx.catalog.relationships {
            let (next_table, step) = if rel.from_table == current {
                (
                    rel.to_table.clone(),
                    JoinStep {
                        left_table: current.clone(),
                        left_col: rel.from_column.clone(),
                        right_table: rel.to_table.clone(),
                        right_col: rel.to_column.clone(),
                    },
                )
            } else if rel.to_table == current {
                (
                    rel.from_table.clone(),
                    JoinStep {
                        left_table: current.clone(),
                        left_col: rel.to_column.clone(),
                        right_table: rel.from_table.clone(),
                        right_col: rel.from_column.clone(),
                    },
                )
            } else {
                continue;
            };

            if visited.contains(&next_table) {
                continue;
            }

            let mut new_path = path.clone();
            new_path.push(step);

            if next_table == target {
                return Some(new_path);
            }

            visited.insert(next_table.clone());
            queue.push_back((next_table, new_path));
        }
    }

    None
}

pub fn find_join_path(
    ctx: &ExecutionContext,
    rc: &RowContext,
    source: &str,
    target: &str,
) -> DaxResult<Vec<JoinStep>> {
    try_find_join_path(ctx, rc, source, target).ok_or_else(|| {
        DaxError::Eval(format!(
            "No relationship path found from '{source}' to '{target}'"
        ))
    })
}

fn find_path_from_frames(
    ctx: &ExecutionContext,
    rc: &RowContext,
    target_table: &str,
) -> DaxResult<(String, Vec<JoinStep>)> {
    for frame_table in rc.frame_tables() {
        if let Some(path) = try_find_join_path(ctx, rc, &frame_table, target_table) {
            return Ok((frame_table, path));
        }
    }
    Err(DaxError::Eval(format!(
        "No relationship path found from current row context ({:?}) to table '{target_table}'",
        rc.frame_tables()
    )))
}

// RELATED / RELATEDTABLE ----------------------------------------------------

pub fn related_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "RELATED requires exactly 1 argument".into(),
        ));
    }
    let (target_table, target_column) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "RELATED: argument must be a column reference, got {other:?}"
            )))
        }
    };

    if !rc.frame_tables().is_empty() {
        let (frame_table, path) = find_path_from_frames(ctx, rc, &target_table)?;

        let first_step = &path[0];
        let mut current_key = rc
            .lookup(&frame_table, &first_step.left_col)
            .ok_or_else(|| {
                DaxError::Eval(format!(
                    "RELATED: join key '{frame_table}[{}]' not in row context",
                    first_step.left_col
                ))
            })?
            .clone();

        for (i, step) in path.iter().enumerate() {
            let is_last = i == path.len() - 1;
            let df = ctx.get_filtered_df(&step.right_table, fc, rc)?;
            let mask = scalar_eq_mask(
                df.column(&step.right_col)
                    .map_err(|_| {
                        DaxError::Eval(format!("RELATED: column '{}' not found", step.right_col))
                    })?
                    .as_materialized_series(),
                &current_key,
            )?;
            let filtered = df
                .filter(&mask)
                .map_err(|e| DaxError::Eval(format!("RELATED: filter failed: {e}")))?;

            if filtered.height() == 0 {
                return Ok(Value::Blank);
            }

            let lookup_col = if is_last {
                &target_column
            } else {
                &path[i + 1].left_col
            };
            let av = filtered
                .column(lookup_col)
                .map_err(|_| DaxError::Eval(format!("RELATED: column '{lookup_col}' not found")))?
                .as_materialized_series()
                .get(0)
                .expect("filtered.height() == 0 returns early above");

            if is_last {
                return Value::try_from(av);
            }
            current_key = ScalarValue::try_from(av)?;
        }
        unreachable!()
    } else if let Some((scoped_table, scoped_df)) = rc.table_scope.iter().next() {
        let path = find_join_path(ctx, rc, scoped_table, &target_table)?;

        let mut current_series = scoped_df
            .column(&path[0].left_col)
            .map_err(|_| {
                DaxError::Eval(format!("RELATED: column '{}' not found", path[0].left_col))
            })?
            .as_materialized_series()
            .clone();

        for (i, step) in path.iter().enumerate() {
            let is_last = i == path.len() - 1;
            let right_df = ctx.get_filtered_df(&step.right_table, fc, rc)?;
            let key_series = right_df
                .column(&step.right_col)
                .map_err(|_| {
                    DaxError::Eval(format!("RELATED: column '{}' not found", step.right_col))
                })?
                .as_materialized_series();
            let val_col = if is_last {
                &target_column
            } else {
                &path[i + 1].left_col
            };
            let val_series = right_df
                .column(val_col)
                .map_err(|_| DaxError::Eval(format!("RELATED: column '{val_col}' not found")))?
                .as_materialized_series();
            current_series = lookup_series(&current_series, key_series, val_series)?;
        }
        Ok(Value::Series(current_series))
    } else {
        Err(DaxError::Eval(
            "RELATED: must be called within a row iteration or FILTER context".into(),
        ))
    }
}

pub fn relatedtable_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "RELATEDTABLE requires exactly 1 argument".into(),
        ));
    }
    let target_table = match &args[0] {
        BoundExprNode::Table(t) => t.name.clone(),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "RELATEDTABLE: argument must be a table reference, got {other:?}"
            )))
        }
    };

    let (frame_table, path) = find_path_from_frames(ctx, rc, &target_table)?;

    let first_step = &path[0];
    let mut current_key = rc
        .lookup(&frame_table, &first_step.left_col)
        .ok_or_else(|| {
            DaxError::Eval(format!(
                "RELATEDTABLE: join key '{frame_table}[{}]' not in row context",
                first_step.left_col
            ))
        })?
        .clone();

    for i in 0..path.len() - 1 {
        let step = &path[i];
        let df = ctx.get_filtered_df(&step.right_table, fc, rc)?;
        let mask = scalar_eq_mask(
            df.column(&step.right_col)
                .map_err(|_| {
                    DaxError::Eval(format!(
                        "RELATEDTABLE: column '{}' not found",
                        step.right_col
                    ))
                })?
                .as_materialized_series(),
            &current_key,
        )?;
        let filtered = df
            .filter(&mask)
            .map_err(|e| DaxError::Eval(format!("RELATEDTABLE: filter failed: {e}")))?;
        if filtered.height() == 0 {
            let empty = ctx.get_filtered_df(&target_table, fc, rc)?.slice(0, 0);
            return Ok(Value::Table(target_table, empty));
        }
        let next_col = &path[i + 1].left_col;
        let av = filtered
            .column(next_col)
            .map_err(|_| DaxError::Eval(format!("RELATEDTABLE: column '{next_col}' not found")))?
            .as_materialized_series()
            .get(0)
            .expect("filtered.height() == 0 returns early above");
        current_key = ScalarValue::try_from(av)?;
    }

    let last_step = path
        .last()
        .expect("path is non-empty: path[0] accessed above");
    let df = ctx.get_filtered_df(&target_table, fc, rc)?;
    let mask = scalar_eq_mask(
        df.column(&last_step.right_col)
            .map_err(|_| {
                DaxError::Eval(format!(
                    "RELATEDTABLE: column '{}' not found",
                    last_step.right_col
                ))
            })?
            .as_materialized_series(),
        &current_key,
    )?;
    Ok(Value::Table(
        target_table,
        df.filter(&mask)
            .map_err(|e| DaxError::Eval(format!("RELATEDTABLE: filter failed: {e}")))?,
    ))
}

fn scalar_eq_mask(series: &Series, key: &ScalarValue) -> DaxResult<BooleanChunked> {
    match key {
        ScalarValue::Integer(i) => {
            let cast = series
                .cast(&polars::prelude::DataType::Int64)
                .map_err(|e| DaxError::Type(format!("RELATED: cast to Int64 failed: {e}")))?;
            Ok(cast.i64().expect("cast to Int64 succeeded").equal(*i))
        }
        ScalarValue::Number(n) => {
            let cast = series
                .cast(&polars::prelude::DataType::Float64)
                .map_err(|e| DaxError::Type(format!("RELATED: cast to Float64 failed: {e}")))?;
            Ok(cast.f64().expect("cast to Float64 succeeded").equal(*n))
        }
        ScalarValue::Text(s) => Ok(series
            .str()
            .map_err(|e| DaxError::Type(format!("RELATED: expected string series: {e}")))?
            .equal(s.as_str())),
        ScalarValue::DateTime(ms) => Ok(series
            .datetime()
            .map_err(|e| DaxError::Type(format!("RELATED: expected datetime series: {e}")))?
            .phys
            .equal(*ms)),
        ScalarValue::Blank => Ok(BooleanChunked::full("mask".into(), false, series.len())),
        other => Err(DaxError::Type(format!(
            "RELATED: unsupported join key type {other:?}"
        ))),
    }
}

fn lookup_series(from: &Series, keys: &Series, values: &Series) -> DaxResult<Series> {
    let mut key_col = keys.clone();
    key_col.rename(from.name().clone());
    let lookup_df = DataFrame::new_infer_height(vec![key_col.into(), values.clone().into()])
        .expect("key and value columns come from the same DataFrame");
    let from_df = DataFrame::new_infer_height(vec![from.clone().into()])
        .expect("single-column DataFrame always succeeds");

    let joined = from_df
        .join(
            &lookup_df,
            [from.name().as_str()],
            [from.name().as_str()],
            JoinType::Left.into(),
            None,
        )
        .map_err(|e| DaxError::Eval(format!("RELATED: join failed: {e}")))?;

    Ok(joined
        .column(values.name())
        .expect("values column present after left join")
        .as_materialized_series()
        .clone())
}

// USERELATIONSHIP / CROSSFILTER ---------------------------------------------

pub fn userelationship_fn(
    _args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    Err(DaxError::InvalidArgument(
        "USERELATIONSHIP is only valid as a CALCULATE modifier".into(),
    ))
}

pub fn crossfilter_fn(
    _args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    Err(DaxError::InvalidArgument(
        "CROSSFILTER is only valid as a CALCULATE modifier".into(),
    ))
}

// Shared helper: find a relationship by its two column endpoints (either order)

pub(crate) fn find_relationship_by_columns<'a>(
    catalog: &'a crate::catalog::Catalog,
    table_a: &str,
    col_a: &str,
    table_b: &str,
    col_b: &str,
) -> Option<&'a crate::loaders::tmsl::model::Relationship> {
    catalog.relationships.iter().find(|r| {
        (r.from_table == table_a
            && r.from_column == col_a
            && r.to_table == table_b
            && r.to_column == col_b)
            || (r.from_table == table_b
                && r.from_column == col_b
                && r.to_table == table_a
                && r.to_column == col_a)
    })
}

fn extract_column_ref(node: &BoundExprNode) -> DaxResult<(String, String)> {
    match node {
        BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
        other => Err(DaxError::InvalidArgument(format!(
            "Expected a column reference, got {other:?}"
        ))),
    }
}

// USERELATIONSHIP(col1, col2) -----------------------------------------------

pub fn apply_userelationship_modifier(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &mut FilterContext,
) -> DaxResult<()> {
    use crate::engine::context::RelationshipOverride;

    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "USERELATIONSHIP requires exactly 2 column reference arguments".into(),
        ));
    }

    let (table_a, col_a) = extract_column_ref(&args[0])?;
    let (table_b, col_b) = extract_column_ref(&args[1])?;

    let target_rel = find_relationship_by_columns(&ctx.catalog, &table_a, &col_a, &table_b, &col_b)
        .ok_or_else(|| {
            DaxError::InvalidArgument(format!(
                "USERELATIONSHIP: no relationship found between \
             '{table_a}[{col_a}]' and '{table_b}[{col_b}]'"
            ))
        })?;
    let target_name = target_rel.name.clone();
    let target_bidir = target_rel.bidirectional;

    let tables = [table_a.as_str(), table_b.as_str()];
    for rel in &ctx.catalog.relationships {
        if rel.name == target_name {
            continue;
        }
        let involves_both = tables.iter().any(|&t| t == rel.from_table)
            && tables.iter().any(|&t| t == rel.to_table);
        if involves_both && rel.active && !fc.relationship_overrides.contains_key(&rel.name) {
            fc.relationship_overrides
                .insert(rel.name.clone(), RelationshipOverride::Disabled);
        }
    }

    let override_val = if target_bidir {
        RelationshipOverride::Bidirectional
    } else {
        RelationshipOverride::Unidirectional
    };
    fc.relationship_overrides.insert(target_name, override_val);

    Ok(())
}

// CROSSFILTER(col1, col2, direction) ----------------------------------------

pub fn apply_crossfilter_modifier(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &mut FilterContext,
) -> DaxResult<()> {
    use crate::engine::context::RelationshipOverride;
    use crate::engine::ir::operator::{CrossFilterDirection, LiteralValue};

    if args.len() != 3 {
        return Err(DaxError::InvalidArgument(
            "CROSSFILTER requires exactly 3 arguments: CROSSFILTER(col1, col2, direction)".into(),
        ));
    }

    let (table_a, col_a) = extract_column_ref(&args[0])?;
    let (table_b, col_b) = extract_column_ref(&args[1])?;

    let override_val = match &args[2] {
        BoundExprNode::Literal(lit) => match &lit.value {
            LiteralValue::CrossFilterDirection(CrossFilterDirection::None) => {
                RelationshipOverride::Disabled
            }
            LiteralValue::CrossFilterDirection(CrossFilterDirection::OneWay) => {
                RelationshipOverride::Unidirectional
            }
            LiteralValue::CrossFilterDirection(CrossFilterDirection::Both) => {
                RelationshipOverride::Bidirectional
            }
            _ => {
                return Err(DaxError::InvalidArgument(
                    "CROSSFILTER: direction must be NONE, ONEWAY, or BOTH".into(),
                ))
            }
        },
        _ => {
            return Err(DaxError::InvalidArgument(
                "CROSSFILTER: direction must be NONE, ONEWAY, or BOTH".into(),
            ))
        }
    };

    let target_rel = find_relationship_by_columns(&ctx.catalog, &table_a, &col_a, &table_b, &col_b)
        .ok_or_else(|| {
            DaxError::InvalidArgument(format!(
                "CROSSFILTER: no relationship found between \
             '{table_a}[{col_a}]' and '{table_b}[{col_b}]'"
            ))
        })?;

    fc.relationship_overrides
        .insert(target_rel.name.clone(), override_val);

    Ok(())
}
