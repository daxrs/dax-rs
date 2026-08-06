use polars::prelude::{BooleanChunked, Series};

use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::row_context::RowContext;
use crate::engine::table_col::TableCol;

// IF(condition, true_result[, false_result]) --------------------------------

pub fn if_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 2 || args.len() > 3 {
        return Err(DaxError::InvalidArgument(
            "IF requires 2 or 3 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let condition_expr = it.next().expect("args.len() is checked above to be 2 or 3");
    let true_expr = it.next().expect("args.len() is checked above to be 2 or 3");
    let false_expr = it.next();

    let condition = match eval(condition_expr, fc, rc)? {
        Value::Boolean(b) => b,
        other => {
            return Err(DaxError::Type(format!(
                "IF: condition must be boolean, got {other:?}"
            )))
        }
    };

    if condition {
        eval(true_expr, fc, rc)
    } else {
        match false_expr {
            Some(expr) => eval(expr, fc, rc),
            None => Ok(Value::Blank),
        }
    }
}

// COALESCE(value1, value2[, value3, ...]) ------------------------------------

pub fn coalesce_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "COALESCE requires at least 2 arguments".into(),
        ));
    }
    for arg in args {
        match eval(arg, fc, rc)? {
            Value::Blank => continue,
            other => return Ok(other),
        }
    }
    Ok(Value::Blank)
}

// SELECTEDVALUE(column[, alternateResult]) ----------------------------------

pub fn selectedvalue_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.is_empty() || args.len() > 2 {
        return Err(DaxError::InvalidArgument(
            "SELECTEDVALUE requires 1 or 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let column_expr = it.next().expect("args.len() is either 1 or 2");
    let alt_expr = it.next();

    let series = match eval(column_expr, fc, rc)? {
        Value::Series(s) => s,
        other => {
            return Err(DaxError::Type(format!(
                "SELECTEDVALUE: first argument must be a column, got {other:?}"
            )))
        }
    };

    if series
        .n_unique()
        .map_err(|e| DaxError::Eval(format!("SELECTEDVALUE: n_unique failed: {e}")))?
        == 1
    {
        Value::try_from(
            series
                .get(0)
                .expect("n_unique() == 1 guarantees at least one element"),
        )
    } else {
        match alt_expr {
            Some(expr) => eval(expr, fc, rc),
            None => Ok(Value::Blank),
        }
    }
}

// SWITCH(expr, val1, result1[, val2, result2, ...][, else]) -----------------

pub fn switch_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 {
        return Err(DaxError::InvalidArgument(
            "SWITCH requires at least 3 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let switch_val = eval(
        it.next()
            .expect("args.len() >= 3 guarantees a first element"),
        fc,
        rc,
    )?;
    let rest: Vec<BoundExprNode> = it.collect();

    let mut i = 0;
    while i + 1 < rest.len() {
        let candidate = eval(rest[i].clone(), fc, rc)?;
        if switch_val == candidate {
            return eval(rest[i + 1].clone(), fc, rc);
        }
        i += 2;
    }

    if rest.len() % 2 == 1 {
        eval(
            rest.last()
                .expect("odd rest.len() guarantees at least one element")
                .clone(),
            fc,
            rc,
        )
    } else {
        Ok(Value::Blank)
    }
}

// ISINSCOPE(column) ---------------------------------------------------------

pub fn isinscope_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "ISINSCOPE requires exactly 1 argument".into(),
        ));
    }
    let (table, column) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ISINSCOPE: argument must be a column reference, got {other:?}"
            )))
        }
    };
    Ok(Value::Boolean(fc.scoped_columns.contains(&(table, column))))
}

// ISFILTERED(column) --------------------------------------------------------

pub fn isfiltered_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    _rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "ISFILTERED requires exactly 1 argument".into(),
        ));
    }
    let (table, column) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ISFILTERED: argument must be a column reference, got {other:?}"
            )))
        }
    };
    let filtered = fc.direct_filters.contains(&(table.clone(), column))
        || fc.table_overrides.contains_key(&table);
    Ok(Value::Boolean(filtered))
}

// EARLIER(column [, levels]) -------------------------------------------------

pub fn earlier_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.is_empty() || args.len() > 2 {
        return Err(DaxError::InvalidArgument(
            "EARLIER requires 1 or 2 arguments".into(),
        ));
    }
    let (table, column) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "EARLIER: first argument must be a column reference, got {other:?}"
            )))
        }
    };
    let levels = match args.get(1) {
        Some(expr) => match eval(expr.clone(), fc, rc)? {
            Value::Integer(i) if i > 0 => i as usize,
            Value::Number(n) if n > 0.0 => n as usize,
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "EARLIER: levels argument must be a positive number, got {other:?}"
                )))
            }
        },
        None => 1,
    };

    rc.earlier(&table, &column, levels)
        .map(|v| v.clone().into())
        .ok_or_else(|| {
            DaxError::Eval(format!(
                "EARLIER: no row context {levels} level(s) out for '{table}[{column}]'"
            ))
        })
}

// EARLIEST(column) ------------------------------------------------------------

pub fn earliest_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "EARLIEST requires exactly 1 argument".into(),
        ));
    }
    let (table, column) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "EARLIEST: argument must be a column reference, got {other:?}"
            )))
        }
    };

    rc.earliest(&table, &column)
        .map(|v| v.clone().into())
        .ok_or_else(|| {
            DaxError::Eval(format!(
                "EARLIEST: no outer row context for '{table}[{column}]'"
            ))
        })
}

// CONTAINS(table, column, value, ...) ---------------------------------------

pub fn contains_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 || args.len().is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "CONTAINS requires a table followed by one or more column/value pairs".into(),
        ));
    }
    let mut it = args.into_iter();
    let (table_name, df) = match eval(
        it.next()
            .expect("args.len() >= 3 guarantees a first element"),
        fc,
        rc,
    )? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "CONTAINS: first argument must be a table, got {other:?}"
            )))
        }
    };

    let rc_scoped = rc.with_table_scope(table_name, df.clone());
    let pairs: Vec<BoundExprNode> = it.collect();

    let mut mask: Option<BooleanChunked> = None;
    for chunk in pairs.chunks(2) {
        let col_series = match eval(chunk[0].clone(), fc, &rc_scoped)? {
            Value::Series(s) => s,
            other => {
                return Err(DaxError::Type(format!(
                    "CONTAINS: column argument must be a series, got {other:?}"
                )))
            }
        };
        let val = eval(chunk[1].clone(), fc, rc)?;
        let col_mask = series_eq_scalar(&col_series, val)?;
        mask = Some(match mask {
            None => col_mask,
            Some(m) => m & col_mask,
        });
    }

    let filtered = df
        .filter(&mask.expect("loop always executes at least once: pairs is non-empty by guard"))
        .map_err(|e| DaxError::Eval(format!("CONTAINS: filter failed: {e}")))?;
    Ok(Value::Boolean(filtered.height() > 0))
}

// LOOKUPVALUE(result_col, search_col1, value1 [, search_col2, value2, ...] [, alternate])

pub fn lookupvalue_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 {
        return Err(DaxError::InvalidArgument(
            "LOOKUPVALUE requires at least 3 arguments: result_column, search_column, search_value"
                .into(),
        ));
    }

    let (result_table, result_col) = match &args[0] {
        BoundExprNode::Column(c) => (c.table.clone(), c.column.clone()),
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "LOOKUPVALUE: first argument must be a column reference, got {other:?}"
            )))
        }
    };

    let remaining = &args[1..];
    let has_alternate = remaining.len() % 2 == 1;
    let pairs_end = if has_alternate {
        remaining.len() - 1
    } else {
        remaining.len()
    };

    let mut search_pairs: Vec<(String, BoundExprNode)> = Vec::new();
    for chunk in remaining[..pairs_end].chunks(2) {
        let col_name = match &chunk[0] {
            BoundExprNode::Column(c) => {
                if c.table != result_table {
                    return Err(DaxError::InvalidArgument(format!(
                        "LOOKUPVALUE: search column '{}.{}' must be in the same table as the result column '{}'",
                        c.table, c.column, result_table,
                    )));
                }
                c.column.clone()
            }
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "LOOKUPVALUE: search column must be a column reference, got {other:?}"
                )))
            }
        };
        search_pairs.push((col_name, chunk[1].clone()));
    }

    let df = ctx.get_filtered_df(&result_table, fc, rc)?;

    let mut search: Vec<(Series, Value)> = Vec::with_capacity(search_pairs.len());
    for (search_col, value_expr) in &search_pairs {
        let val = eval(value_expr.clone(), fc, rc)?;
        let resolved = {
            let qualified = TableCol::new(&result_table, search_col).to_string();
            if df.column(&qualified).is_ok() {
                qualified
            } else {
                search_col.clone()
            }
        };
        let col_series = df
            .column(&resolved)
            .map_err(|_| {
                DaxError::UnknownName(format!(
                    "LOOKUPVALUE: column '{result_table}[{search_col}]' not found"
                ))
            })?
            .as_materialized_series()
            .clone();
        search.push((col_series, val));
    }

    let alternate = |e: &[BoundExprNode]| -> DaxResult<Value> {
        if has_alternate {
            eval(
                e.last()
                    .expect("has_alternate guarantees remaining is non-empty")
                    .clone(),
                fc,
                rc,
            )
        } else {
            Ok(Value::Blank)
        }
    };

    // Direct scan instead of building a boolean mask + DataFrame::filter():
    // LOOKUPVALUE targets are dimension-shaped tables by DAX convention (a
    // handful to a few thousand rows), and this runs once per outer row when
    // called from SUMX/ADDCOLUMNS/etc. — the mask+filter approach allocates a
    // new BooleanChunked and a new filtered DataFrame on every such call,
    // which dominates when the caller iterates a large fact table.
    let mut matched_rows: Vec<usize> = Vec::new();
    'rows: for row_idx in 0..df.height() {
        for (col_series, val) in &search {
            if !value_eq_at(col_series, row_idx, val)? {
                continue 'rows;
            }
        }
        matched_rows.push(row_idx);
    }

    if matched_rows.is_empty() {
        return alternate(remaining);
    }

    let result_resolved = {
        let qualified = TableCol::new(&result_table, &result_col).to_string();
        if df.column(&qualified).is_ok() {
            qualified
        } else {
            result_col.clone()
        }
    };
    let result_series = df
        .column(&result_resolved)
        .map_err(|_| {
            DaxError::UnknownName(format!(
                "LOOKUPVALUE: result column '{result_table}[{result_col}]' not found"
            ))
        })?
        .as_materialized_series();

    let first_av = result_series
        .get(matched_rows[0])
        .map_err(|e| DaxError::Eval(format!("LOOKUPVALUE: {e}")))?;
    for &row_idx in &matched_rows[1..] {
        let av = result_series
            .get(row_idx)
            .map_err(|e| DaxError::Eval(format!("LOOKUPVALUE: {e}")))?;
        if av != first_av {
            return Err(DaxError::Eval(format!(
                "LOOKUPVALUE: multiple rows found with different values for '{result_table}[{result_col}]'"
            )));
        }
    }
    Value::try_from(first_av)
}

fn value_eq_at(series: &polars::prelude::Series, row_idx: usize, val: &Value) -> DaxResult<bool> {
    use polars::prelude::DataType;
    Ok(match val {
        Value::Integer(i) => match series.dtype() {
            DataType::Int64 => {
                series.i64().expect("dtype matched as Int64").get(row_idx) == Some(*i)
            }
            DataType::Int32 => {
                series.i32().expect("dtype matched as Int32").get(row_idx) == Some(*i as i32)
            }
            DataType::Float64 => {
                series.f64().expect("dtype matched as Float64").get(row_idx) == Some(*i as f64)
            }
            other @ (DataType::Boolean
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Datetime(_, _)
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => {
                return Err(DaxError::Type(format!(
                    "LOOKUPVALUE: unsupported numeric dtype {other:?}"
                )))
            }
        },
        Value::Number(n) => match series.dtype() {
            DataType::Float64 => {
                series.f64().expect("dtype matched as Float64").get(row_idx) == Some(*n)
            }
            DataType::Int64 => {
                series.i64().expect("dtype matched as Int64").get(row_idx) == Some(*n as i64)
            }
            DataType::Int32 => {
                series.i32().expect("dtype matched as Int32").get(row_idx) == Some(*n as i32)
            }
            other @ (DataType::Boolean
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Datetime(_, _)
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => {
                return Err(DaxError::Type(format!(
                    "LOOKUPVALUE: unsupported numeric dtype {other:?}"
                )))
            }
        },
        Value::String(s) => {
            series.str().expect("dtype matched as String").get(row_idx) == Some(s.as_str())
        }
        Value::Boolean(b) => {
            series
                .bool()
                .expect("dtype matched as Boolean")
                .get(row_idx)
                == Some(*b)
        }
        other @ (Value::Blank | Value::DateTime(_) | Value::Table(_, _) | Value::Series(_)) => {
            return Err(DaxError::Type(format!(
                "LOOKUPVALUE: unsupported value type {other:?}"
            )))
        }
    })
}

pub(crate) fn series_eq_scalar(
    series: &polars::prelude::Series,
    val: Value,
) -> DaxResult<BooleanChunked> {
    use polars::prelude::{ChunkCompareEq, DataType};
    match val {
        Value::Integer(i) => match series.dtype() {
            DataType::Int64 => Ok(series.i64().expect("dtype matched as Int64").equal(i)),
            DataType::Int32 => Ok(series
                .i32()
                .expect("dtype matched as Int32")
                .equal(i as i32)),
            DataType::Float64 => Ok(series
                .f64()
                .expect("dtype matched as Float64")
                .equal(i as f64)),
            other @ (DataType::Boolean
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Datetime(_, _)
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => Err(DaxError::Type(format!(
                "CONTAINS: unsupported numeric dtype {other:?}"
            ))),
        },
        Value::Number(n) => match series.dtype() {
            DataType::Float64 => Ok(series.f64().expect("dtype matched as Float64").equal(n)),
            DataType::Int64 => Ok(series
                .i64()
                .expect("dtype matched as Int64")
                .equal(n as i64)),
            DataType::Int32 => Ok(series
                .i32()
                .expect("dtype matched as Int32")
                .equal(n as i32)),
            other @ (DataType::Boolean
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Datetime(_, _)
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => Err(DaxError::Type(format!(
                "CONTAINS: unsupported numeric dtype {other:?}"
            ))),
        },
        Value::String(s) => Ok(series
            .str()
            .expect("dtype matched as String")
            .equal(s.as_str())),
        Value::Boolean(b) => Ok(series
            .bool()
            .expect("dtype matched as Boolean")
            .no_null_iter()
            .map(|v| v == b)
            .collect()),
        other @ (Value::Blank | Value::DateTime(_) | Value::Table(_, _) | Value::Series(_)) => Err(
            DaxError::Type(format!("CONTAINS: unsupported value type {other:?}")),
        ),
    }
}
