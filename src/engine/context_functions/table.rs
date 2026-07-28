use polars::prelude::{
    Column, DataFrame, DataFrameJoinOps, DataType, JoinArgs, JoinType, NamedFrom, PlSmallStr,
    Scalar, Series, SortMultipleOptions,
};

use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::row_context::RowContext;
use crate::engine::table_col::TableCol;

// SELECTCOLUMNS(table, name1, expr1, name2, expr2, ...) ---------------------

pub fn selectcolumns_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 || !(args.len() - 1).is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "SELECTCOLUMNS requires a table then one or more (name, expr) pairs".into(),
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
                "SELECTCOLUMNS: first argument must be a table, got {other:?}"
            )))
        }
    };

    let rc_scoped = rc.with_table_scope(table_name.clone(), df.clone());
    let mut columns: Vec<Column> = Vec::new();

    while let Some(name_expr) = it.next() {
        let col_expr = it
            .next()
            .expect("guard ensures even remainder — name always has a paired expr");

        let raw_name = match eval(name_expr, fc, rc)? {
            Value::String(s) => s,
            other => {
                return Err(DaxError::Type(format!(
                    "SELECTCOLUMNS: column name must be a string, got {other:?}"
                )))
            }
        };
        let col_name = TableCol::try_parse(&raw_name)
            .map(|tc| tc.to_string())
            .unwrap_or(raw_name);

        let column: Column = match eval(col_expr, fc, &rc_scoped)? {
            Value::Series(s) => s.with_name(col_name.as_str().into()).into(),
            Value::String(s) => Column::new_scalar(
                col_name.as_str().into(),
                Scalar::from(PlSmallStr::from(s)),
                df.height(),
            ),
            Value::Number(n) => {
                Column::new_scalar(col_name.as_str().into(), Scalar::from(n), df.height())
            }
            Value::Integer(i) => {
                Column::new_scalar(col_name.as_str().into(), Scalar::from(i), df.height())
            }
            Value::Boolean(b) => {
                Column::new_scalar(col_name.as_str().into(), Scalar::from(b), df.height())
            }
            Value::Blank => Column::new_scalar(
                col_name.as_str().into(),
                Scalar::null(DataType::Null),
                df.height(),
            ),
            other => {
                return Err(DaxError::Type(format!(
                    "SELECTCOLUMNS: column expression returned unexpected type {other:?}"
                )))
            }
        };
        columns.push(column);
    }

    let result = DataFrame::new_infer_height(columns)
        .map_err(|e| DaxError::Eval(format!("SELECTCOLUMNS: failed to build DataFrame: {e}")))?;
    Ok(Value::Table(table_name, result))
}

// TOPN(n, table, orderBy_expr [, order, ...]) -------------------------------

pub fn topn_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 {
        return Err(DaxError::InvalidArgument(
            "TOPN requires at least 3 arguments: TOPN(n, table, orderBy_expr [, order, ...])"
                .into(),
        ));
    }
    let remaining = args.len() - 2;
    if remaining > 1 && !remaining.is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "TOPN: each orderBy expression after the first must be paired with an order argument"
                .into(),
        ));
    }

    let mut it = args.into_iter();

    let n: usize = match eval(
        it.next()
            .expect("args.len() >= 3 guarantees a first element"),
        fc,
        rc,
    )? {
        Value::Integer(i) if i >= 0 => i as usize,
        Value::Number(n) if n >= 0.0 => n as usize,
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "TOPN: first argument must be a non-negative integer, got {other:?}"
            )))
        }
    };

    let (table_name, mut df) = match eval(
        it.next()
            .expect("args.len() >= 3 guarantees a second element"),
        fc,
        rc,
    )? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "TOPN: second argument must be a table, got {other:?}"
            )))
        }
    };

    let mut temp_col_names: Vec<String> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();
    let mut pair_index = 0usize;

    while let Some(expr_node) = it.next() {
        let is_desc = match it.next() {
            None => true,
            Some(order_node) => match eval(order_node, fc, rc)? {
                Value::Boolean(b) => !b,
                Value::Integer(0) => true,
                Value::Integer(1) => false,
                Value::Number(0.0) => true,
                Value::Number(1.0) => false,
                other => {
                    return Err(DaxError::InvalidArgument(format!(
                        "TOPN: order argument must be 0 (DESC) or 1 (ASC), got {other:?}"
                    )))
                }
            },
        };

        let direct_col: Option<Series> = if let BoundExprNode::Column(c) = &expr_node {
            let qualified = TableCol::new(&c.table, &c.column).to_string();
            df.column(&qualified)
                .or_else(|_| df.column(&c.column))
                .ok()
                .map(|s| s.as_materialized_series().clone())
        } else {
            None
        };

        let rc_scoped = rc.with_table_scope(table_name.clone(), df.clone());
        let column: Column = if let Some(s) = direct_col {
            s.into()
        } else {
            match eval(expr_node, fc, &rc_scoped)? {
                Value::Series(s) => s.into(),
                Value::Integer(i) => Column::new_scalar("_".into(), Scalar::from(i), df.height()),
                Value::Number(n) => Column::new_scalar("_".into(), Scalar::from(n), df.height()),
                Value::Boolean(b) => Column::new_scalar("_".into(), Scalar::from(b), df.height()),
                Value::String(s) => {
                    Column::new_scalar("_".into(), Scalar::from(PlSmallStr::from(s)), df.height())
                }
                other => {
                    return Err(DaxError::Type(format!(
                        "TOPN: orderBy expression must be scalar or column, got {other:?}"
                    )))
                }
            }
        };

        let col_name = format!("__topn_{pair_index}__");
        let renamed = column.with_name(col_name.as_str().into());
        df.with_column(renamed)
            .map_err(|e| DaxError::Eval(format!("TOPN: failed to add sort column: {e}")))?;

        temp_col_names.push(col_name);
        descending.push(is_desc);
        pair_index += 1;
    }

    let sorted = df
        .sort(
            temp_col_names.clone(),
            SortMultipleOptions::new().with_order_descending_multi(descending),
        )
        .map_err(|e| DaxError::Eval(format!("TOPN: sort failed: {e}")))?;

    let topped = sorted.head(Some(n));
    let result = topped.drop_many(temp_col_names);

    Ok(Value::Table(table_name, result))
}

// SAMPLE(n, table, orderBy_expr [, order, ...]) -----------------------------

pub fn sample_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 {
        return Err(DaxError::InvalidArgument(
            "SAMPLE requires at least 3 arguments: SAMPLE(n, table, orderBy_expr [, order, ...])"
                .into(),
        ));
    }
    let remaining = args.len() - 2;
    if remaining > 1 && !remaining.is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "SAMPLE: each orderBy expression after the first must be paired with an order argument"
                .into(),
        ));
    }

    let mut it = args.into_iter();

    let n: usize = match eval(
        it.next()
            .expect("args.len() >= 3 guarantees a first element"),
        fc,
        rc,
    )? {
        Value::Integer(i) if i >= 0 => i as usize,
        Value::Number(f) if f >= 0.0 => f as usize,
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "SAMPLE: first argument must be a non-negative integer, got {other:?}"
            )))
        }
    };

    let (table_name, mut df) = match eval(
        it.next()
            .expect("args.len() >= 3 guarantees a second element"),
        fc,
        rc,
    )? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "SAMPLE: second argument must be a table, got {other:?}"
            )))
        }
    };

    if n == 0 || df.height() == 0 {
        return Ok(Value::Table(table_name, df.head(Some(0))));
    }

    let mut temp_col_names: Vec<String> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();
    let mut pair_index = 0usize;

    while let Some(expr_node) = it.next() {
        let is_desc = match it.next() {
            None => true,
            Some(order_node) => match eval(order_node, fc, rc)? {
                Value::Boolean(b) => !b,
                Value::Integer(0) => true,
                Value::Integer(1) => false,
                Value::Number(0.0) => true,
                Value::Number(1.0) => false,
                other => {
                    return Err(DaxError::InvalidArgument(format!(
                        "SAMPLE: order argument must be 0 (DESC) or 1 (ASC), got {other:?}"
                    )))
                }
            },
        };

        let direct_col: Option<Series> = if let BoundExprNode::Column(c) = &expr_node {
            let qualified = TableCol::new(&c.table, &c.column).to_string();
            df.column(&qualified)
                .or_else(|_| df.column(&c.column))
                .ok()
                .map(|s| s.as_materialized_series().clone())
        } else {
            None
        };

        let rc_scoped = rc.with_table_scope(table_name.clone(), df.clone());
        let column: Column = if let Some(s) = direct_col {
            s.into()
        } else {
            match eval(expr_node, fc, &rc_scoped)? {
                Value::Series(s) => s.into(),
                Value::Integer(i) => Column::new_scalar("_".into(), Scalar::from(i), df.height()),
                Value::Number(f) => Column::new_scalar("_".into(), Scalar::from(f), df.height()),
                Value::Boolean(b) => Column::new_scalar("_".into(), Scalar::from(b), df.height()),
                Value::String(s) => {
                    Column::new_scalar("_".into(), Scalar::from(PlSmallStr::from(s)), df.height())
                }
                other => {
                    return Err(DaxError::Type(format!(
                        "SAMPLE: orderBy expression must be scalar or column, got {other:?}"
                    )))
                }
            }
        };

        let col_name = format!("__sample_{pair_index}__");
        let renamed = column.with_name(col_name.as_str().into());
        df.with_column(renamed)
            .map_err(|e| DaxError::Eval(format!("SAMPLE: failed to add sort column: {e}")))?;

        temp_col_names.push(col_name);
        descending.push(is_desc);
        pair_index += 1;
    }

    let sorted = df
        .sort(
            temp_col_names.clone(),
            SortMultipleOptions::new().with_order_descending_multi(descending),
        )
        .map_err(|e| DaxError::Eval(format!("SAMPLE: sort failed: {e}")))?;

    let height = sorted.height();

    // Select n rows evenly spaced across [0, height-1], always including first and last.
    let sample_indices: Vec<u32> = if n >= height {
        (0..height as u32).collect()
    } else if n == 1 {
        vec![0u32]
    } else {
        (0..n)
            .map(|i| ((i * (height - 1)) / (n - 1)) as u32)
            .collect()
    };

    let idx = polars::prelude::IdxCa::from_vec("".into(), sample_indices);
    let sampled = sorted
        .take(&idx)
        .map_err(|e| DaxError::Eval(format!("SAMPLE: take failed: {e}")))?;

    let result = sampled.drop_many(temp_col_names);
    Ok(Value::Table(table_name, result))
}

// SUBSTITUTEWITHINDEX(table, newColName, indexTable, col, order [, col, order, ...])

pub fn substitutewithindex_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 5 || !(args.len() - 3).is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "SUBSTITUTEWITHINDEX requires (table, name, indexTable, col, order [, col, order, ...])".into(),
        ));
    }
    let mut it = args.into_iter();

    let source_node = it.next().expect("arity checked above: at least 5 args");
    let name_node = it.next().expect("arity checked above: at least 5 args");
    let index_node = it.next().expect("arity checked above: at least 5 args");

    let (source_name, source_df) = match eval(source_node, fc, rc)? {
        Value::Table(n, df) => (n, df),
        other => {
            return Err(DaxError::Type(format!(
                "SUBSTITUTEWITHINDEX: first argument must be a table, got {other:?}"
            )))
        }
    };

    let new_col_name = match eval(name_node, fc, rc)? {
        Value::String(s) => s,
        other => {
            return Err(DaxError::Type(format!(
                "SUBSTITUTEWITHINDEX: second argument must be a string, got {other:?}"
            )))
        }
    };

    let (_, index_df) = match eval(index_node, fc, rc)? {
        Value::Table(n, df) => (n, df),
        other => {
            return Err(DaxError::Type(format!(
                "SUBSTITUTEWITHINDEX: third argument must be a table, got {other:?}"
            )))
        }
    };

    let resolve_col = |df: &DataFrame, qcol: &str| -> DaxResult<String> {
        if df.column(qcol).is_ok() {
            return Ok(qcol.to_string());
        }
        let bare = qcol.split('[').nth(1).unwrap_or(qcol).trim_end_matches(']');
        if df.column(bare).is_ok() {
            return Ok(bare.to_string());
        }
        Err(DaxError::InvalidArgument(format!(
            "SUBSTITUTEWITHINDEX: column '{qcol}' not found in table (columns: {:?})",
            df.get_column_names()
        )))
    };

    let mut idx_cols: Vec<String> = Vec::new();
    let mut src_cols: Vec<String> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();

    while let (Some(col_node), Some(ord_node)) = (it.next(), it.next()) {
        let qcol = match &col_node {
            BoundExprNode::Column(c) => TableCol::new(&c.table, &c.column).to_string(),
            BoundExprNode::Measure(m) => m.name.clone(),
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "SUBSTITUTEWITHINDEX: sort column must be a column or measure reference, got {other:?}"
                )))
            }
        };
        let ascending = match eval(ord_node, fc, rc)? {
            Value::Boolean(b) => b,
            Value::Integer(1) => true,
            Value::Integer(0) => false,
            _ => true,
        };
        idx_cols.push(resolve_col(&index_df, &qcol)?);
        src_cols.push(resolve_col(&source_df, &qcol)?);
        descending.push(!ascending);
    }

    let index_sorted = index_df
        .sort(
            idx_cols.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            SortMultipleOptions::new().with_order_descending_multi(descending),
        )
        .map_err(|e| DaxError::Eval(format!("SUBSTITUTEWITHINDEX: sort failed: {e}")))?;

    let index_values: Vec<i64> = (0i64..index_sorted.height() as i64).collect();
    let index_series = Series::new(new_col_name.as_str().into(), index_values);
    let mut mapping_cols: Vec<polars::prelude::Column> = idx_cols
        .iter()
        .map(|col| {
            index_sorted
                .column(col)
                .expect("resolve_col validated this column exists in index_df above")
                .as_materialized_series()
                .clone()
                .into()
        })
        .collect();
    mapping_cols.push(index_series.into());
    let mapping_df = DataFrame::new_infer_height(mapping_cols).map_err(|e| {
        DaxError::Eval(format!("SUBSTITUTEWITHINDEX: failed to build mapping: {e}"))
    })?;

    let mut join_args = JoinArgs::new(JoinType::Inner);
    join_args.nulls_equal = true;
    let joined = source_df
        .join(
            &mapping_df,
            src_cols.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            idx_cols.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            join_args,
            None,
        )
        .map_err(|e| DaxError::Eval(format!("SUBSTITUTEWITHINDEX: join failed: {e}")))?;

    let result = joined.drop_many(src_cols.iter().map(|s| s.as_str()).collect::<Vec<_>>());

    Ok(Value::Table(source_name, result))
}
