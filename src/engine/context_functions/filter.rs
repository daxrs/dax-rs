use polars::prelude::{BooleanChunked, NamedFrom};

use crate::engine::context::{ExecutionContext, FilterContext, RelationshipOverride};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::ir::operator::{BoundColumn, BoundFunction};
use crate::engine::row_context::RowContext;

use super::select_unique;

// FILTER(table, condition)  ------------------------------------------------

pub fn filter_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "FILTER requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table_expr = it.next().expect("args.len() == 2 guarantees two elements");
    let condition_expr = it.next().expect("args.len() == 2 guarantees two elements");

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "FILTER: first argument must be a table, got {other:?}"
            )))
        }
    };

    let rc_scoped = rc.with_table_scope(table_name.clone(), df.clone());
    let condition = eval(condition_expr, fc, &rc_scoped)?;

    let mask: BooleanChunked = match condition {
        Value::Series(s) => s
            .bool()
            .map_err(|_| {
                DaxError::Type("FILTER condition must evaluate to a boolean series".into())
            })?
            .clone(),
        Value::Boolean(b) => BooleanChunked::new("mask".into(), vec![b; df.height()]),
        other => {
            return Err(DaxError::Type(format!(
                "FILTER condition must be boolean, got {other:?}"
            )))
        }
    };

    let filtered = df
        .filter(&mask)
        .map_err(|e| DaxError::Eval(format!("FILTER: failed to apply mask: {e}")))?;
    Ok(Value::Table(table_name, filtered))
}

// ALL / ALLEXCEPT / REMOVEFILTERS -------------------------------------------

pub fn all_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.is_empty() {
        return Err(DaxError::InvalidArgument(
            "ALL requires at least 1 argument".into(),
        ));
    }

    match &args[0] {
        BoundExprNode::Table(t) => {
            let df = ctx
                .tables
                .get(&t.name)
                .ok_or_else(|| DaxError::UnknownName(format!("ALL: unknown table '{}'", t.name)))?
                .clone();
            Ok(Value::Table(t.name.clone(), df))
        }
        BoundExprNode::Column(_) => {
            let cols: Vec<(&str, &str)> = args
                .iter()
                .map(|a| match a {
                    BoundExprNode::Column(c) => Ok((c.table.as_str(), c.column.as_str())),
                    other => Err(DaxError::InvalidArgument(format!(
                        "ALL: expected column reference, got {other:?}"
                    ))),
                })
                .collect::<DaxResult<_>>()?;

            let table_name = cols[0].0;
            let df = ctx
                .tables
                .get(table_name)
                .ok_or_else(|| DaxError::UnknownName(format!("ALL: unknown table '{table_name}'")))?
                .clone();

            let col_name_strings: Vec<String> = cols.iter().map(|(_, c)| c.to_string()).collect();
            let unique = select_unique(&df, &col_name_strings, "ALL")?;
            Ok(Value::Table(table_name.to_string(), unique))
        }
        other => Err(DaxError::InvalidArgument(format!(
            "ALL: expected table or column reference, got {other:?}"
        ))),
    }
}

pub fn allexcept_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "ALLEXCEPT requires at least 2 arguments".into(),
        ));
    }
    let table_name = match &args[0] {
        BoundExprNode::Table(t) => t.name.clone(),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ALLEXCEPT: first argument must be a table, got {other:?}"
            )))
        }
    };
    let df = ctx
        .tables
        .get(&table_name)
        .ok_or_else(|| DaxError::UnknownName(format!("ALLEXCEPT: unknown table '{table_name}'")))?
        .clone();
    Ok(Value::Table(table_name, df))
}

pub fn removefilters_fn(
    _args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    Err(DaxError::InvalidArgument(
        "REMOVEFILTERS can only be used as a filter modifier inside CALCULATE, not as a table expression".into()
    ))
}

// ALLSELECTED ---------------------------------------------------------------

pub fn allselected_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.is_empty() {
        return Err(DaxError::InvalidArgument(
            "ALLSELECTED requires at least one argument".into(),
        ));
    }

    // As a table function, ALLSELECTED returns the rows visible in the current
    // filter context. The shadow filter context (outer_fc) only applies when
    // ALLSELECTED is used as a CALCULATE modifier (apply_allselected_modifier).
    match &args[0] {
        BoundExprNode::Table(t) => {
            let df = ctx.get_filtered_df(&t.name, fc, rc)?;
            Ok(Value::Table(t.name.clone(), df))
        }
        BoundExprNode::Column(_) => {
            let cols: Vec<(&str, &str)> = args
                .iter()
                .map(|a| match a {
                    BoundExprNode::Column(c) => Ok((c.table.as_str(), c.column.as_str())),
                    other => Err(DaxError::InvalidArgument(format!(
                        "ALLSELECTED: expected column reference, got {other:?}"
                    ))),
                })
                .collect::<DaxResult<_>>()?;

            let table_name = cols[0].0;
            let df = ctx.get_filtered_df(table_name, fc, rc)?;
            let col_name_strings: Vec<String> = cols.iter().map(|(_, c)| c.to_string()).collect();
            let unique = select_unique(&df, &col_name_strings, "ALLSELECTED")?;
            Ok(Value::Table(table_name.to_string(), unique))
        }
        other => Err(DaxError::InvalidArgument(format!(
            "ALLSELECTED: expected table or column reference, got {other:?}"
        ))),
    }
}

pub fn apply_allselected_modifier(
    func: &BoundFunction,
    outer: &FilterContext,
    new_fc: &mut FilterContext,
) -> DaxResult<()> {
    let first = func.args.first().ok_or_else(|| {
        DaxError::InvalidArgument("ALLSELECTED requires at least one argument".into())
    })?;

    match first {
        BoundExprNode::Table(t) => {
            new_fc.remove_table(&t.name);
            for ((tbl, col), preds) in &outer.filters {
                if tbl == &t.name {
                    new_fc
                        .filters
                        .insert((tbl.clone(), col.clone()), preds.clone());
                }
            }
            if let Some(df) = outer.table_overrides.get(&t.name) {
                new_fc.table_overrides.insert(t.name.clone(), df.clone());
            }
        }
        BoundExprNode::Column(_) => {
            for arg in &func.args {
                let col = match arg {
                    BoundExprNode::Column(c) => c,
                    other => {
                        return Err(DaxError::InvalidArgument(format!(
                            "ALLSELECTED: expected column reference, got {other:?}"
                        )))
                    }
                };
                let key = (col.table.clone(), col.column.clone());
                new_fc.filters.remove(&key);
                if let Some(preds) = outer.filters.get(&key) {
                    new_fc.filters.insert(key, preds.clone());
                }
            }
        }
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ALLSELECTED: expected table or column reference, got {other:?}"
            )))
        }
    }
    Ok(())
}

pub fn apply_allexcept_modifier(func: &BoundFunction, fc: &mut FilterContext) -> DaxResult<()> {
    if func.args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "ALLEXCEPT requires at least 2 arguments".into(),
        ));
    }
    let table = match &func.args[0] {
        BoundExprNode::Table(t) => t.name.clone(),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ALLEXCEPT: first argument must be a table, got {other:?}"
            )))
        }
    };
    let keep: Vec<(String, String)> = func.args[1..]
        .iter()
        .map(|arg| match arg {
            BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
            other => Err(DaxError::InvalidArgument(format!(
                "ALLEXCEPT: column arguments must be column references, got {other:?}"
            ))),
        })
        .collect::<DaxResult<_>>()?;
    fc.remove_table_except(&table, &keep);
    Ok(())
}

pub fn apply_all_modifier(
    func: &BoundFunction,
    fc: &mut FilterContext,
    ctx: &ExecutionContext,
) -> DaxResult<()> {
    let first = func.args.first().ok_or_else(|| {
        DaxError::InvalidArgument("ALL/REMOVEFILTERS requires at least one argument".into())
    })?;

    match first {
        BoundExprNode::Table(t) => {
            fc.remove_table(&t.name);
            // Clearing the table's own filters isn't enough on its own: if a
            // related table still has an active filter, the very next
            // expanded_filter_context call would immediately re-derive a
            // fresh restriction on this table via relationship propagation,
            // silently undoing the ALL(). Disable every relationship
            // touching this table so it stays fully unfiltered for the rest
            // of this CALCULATE's evaluation.
            for rel in &ctx.catalog.relationships {
                if rel.from_table == t.name || rel.to_table == t.name {
                    fc.relationship_overrides
                        .insert(rel.name.clone(), RelationshipOverride::Disabled);
                }
            }
        }
        BoundExprNode::Column(_) => {
            for arg in &func.args {
                if let BoundExprNode::Column(BoundColumn { table, column, .. }) = arg {
                    fc.remove_column(table, column);
                    for rel in &ctx.catalog.relationships {
                        if (&rel.from_table == table && &rel.from_column == column)
                            || (&rel.to_table == table && &rel.to_column == column)
                        {
                            fc.relationship_overrides
                                .insert(rel.name.clone(), RelationshipOverride::Disabled);
                        }
                    }
                }
            }
        }
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ALL/REMOVEFILTERS expects a table or column reference, got {other:?}"
            )))
        }
    }
    Ok(())
}

// KEEPFILTERS(table_or_expr) ------------------------------------------------

pub fn keepfilters_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "KEEPFILTERS requires exactly 1 argument".into(),
        ));
    }
    eval(
        args.into_iter()
            .next()
            .expect("args.len() == 1 guarantees one element"),
        fc,
        rc,
    )
}
