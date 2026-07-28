use polars::prelude::{BooleanChunked, Column, DataFrame, IntoLazy, NamedFrom, Scalar};

use crate::engine::context::{build_mask, ExecutionContext, FilterContext, FilterPredicate};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::ir::operator::LiteralValue;
use crate::engine::row_context::{RowContext, RowFrameCursor, ScalarValue};
use crate::engine::table_col::TableCol;

fn table_prefix(name: &str) -> &str {
    if let Some(i) = name.find('[') {
        &name[..i]
    } else {
        name
    }
}

fn qualify_col(col: &str, prefix: &str) -> String {
    if TableCol::try_parse(col).is_some() {
        col.to_string()
    } else {
        TableCol::new(prefix, col).to_string()
    }
}

// Generic X-function iterator (SUMX / MAXX / MINX / COUNTX share the loop) --

pub fn sumx_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    x_fn(args, ctx, fc, rc, eval, |acc, v| acc + v, 0.0)
}

pub fn maxx_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    x_fn(args, ctx, fc, rc, eval, f64::max, f64::NEG_INFINITY)
}

pub fn minx_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    x_fn(args, ctx, fc, rc, eval, f64::min, f64::INFINITY)
}

fn x_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
    combine: fn(f64, f64) -> f64,
    identity: f64,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "X function requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table_expr = it.next().expect("args.len() == 2 guarantees two elements");
    let value_expr = it.next().expect("args.len() == 2 guarantees two elements");

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "X function: first argument must be a table, got {other:?}"
            )))
        }
    };

    let mut acc = identity;
    let mut found = false;

    let mut cursor = RowFrameCursor::new(&table_name, &df, &[&value_expr], ctx)?;
    for _ in 0..df.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);

        match eval(value_expr.clone(), fc, &rc_row)? {
            Value::Number(n) => {
                acc = combine(acc, n);
                found = true;
            }
            Value::Integer(i) => {
                acc = combine(acc, i as f64);
                found = true;
            }
            Value::Blank => {}
            other => {
                return Err(DaxError::Type(format!(
                    "X function expression must return a number, got {other:?}"
                )))
            }
        }
    }

    Ok(if found {
        Value::Number(acc)
    } else {
        Value::Blank
    })
}

pub fn averagex_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "AVERAGEX requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table_expr = it.next().expect("args.len() == 2 guarantees two elements");
    let value_expr = it.next().expect("args.len() == 2 guarantees two elements");

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "AVERAGEX: first argument must be a table, got {other:?}"
            )))
        }
    };

    let height = df.height();
    if height == 0 {
        return Ok(Value::Blank);
    }

    let mut sum = 0.0f64;
    let mut count = 0usize;

    let mut cursor = RowFrameCursor::new(&table_name, &df, &[&value_expr], ctx)?;
    for _ in 0..height {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);

        match eval(value_expr.clone(), fc, &rc_row)? {
            Value::Number(n) => {
                sum += n;
                count += 1;
            }
            Value::Integer(i) => {
                sum += i as f64;
                count += 1;
            }
            Value::Blank => {}
            other => {
                return Err(DaxError::Type(format!(
                    "AVERAGEX expression must return a number, got {other:?}"
                )))
            }
        }
    }

    Ok(if count == 0 {
        Value::Blank
    } else {
        Value::Number(sum / count as f64)
    })
}

pub fn countx_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "COUNTX requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table_expr = it.next().expect("args.len() == 2 checked above");
    let value_expr = it.next().expect("args.len() == 2 checked above");

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "COUNTX: first argument must be a table, got {other:?}"
            )))
        }
    };

    let mut count = 0usize;

    let mut cursor = RowFrameCursor::new(&table_name, &df, &[&value_expr], ctx)?;
    for _ in 0..df.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);

        match eval(value_expr.clone(), fc, &rc_row)? {
            Value::Blank => {}
            _ => count += 1,
        }
    }

    Ok(if count == 0 {
        Value::Blank
    } else {
        Value::Number(count as f64)
    })
}

// A-variant iterator helper (COUNTAX) ---------------------------------------

fn coerce_a_value(v: Value) -> Option<f64> {
    match v {
        Value::Blank => None,
        Value::Number(n) => Some(n),
        Value::Integer(i) => Some(i as f64),
        Value::Boolean(b) => Some(if b { 1.0 } else { 0.0 }),
        Value::String(ref s) if s.is_empty() => None,
        Value::String(_) => Some(0.0),
        _ => None,
    }
}

pub fn countax_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "COUNTAX requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table_expr = it.next().expect("length checked");
    let value_expr = it.next().expect("length checked");

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "COUNTAX: first argument must be a table, got {other:?}"
            )))
        }
    };

    let mut count = 0usize;
    let mut cursor = RowFrameCursor::new(&table_name, &df, &[&value_expr], ctx)?;
    for _ in 0..df.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);
        if coerce_a_value(eval(value_expr.clone(), fc, &rc_row)?).is_some() {
            count += 1;
        }
    }
    Ok(if count == 0 {
        Value::Blank
    } else {
        Value::Number(count as f64)
    })
}

// Cartesian product helper --------------------------------------------------

fn df_cross_join(left: &DataFrame, right: &DataFrame) -> DaxResult<DataFrame> {
    left.clone()
        .lazy()
        .cross_join(right.clone().lazy(), None)
        .collect()
        .map_err(|e| DaxError::Eval(format!("cross_join: {e}")))
}

// CROSSJOIN(table1, table2, ...) --------------------------------------------

pub fn crossjoin_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "CROSSJOIN requires at least 2 arguments".into(),
        ));
    }
    let mut tables = args
        .into_iter()
        .map(|a| match eval(a, fc, rc)? {
            Value::Table(n, df) => Ok((n, df)),
            other => Err(DaxError::Type(format!(
                "CROSSJOIN: all arguments must be tables, got {other:?}"
            ))),
        })
        .collect::<DaxResult<Vec<_>>>()?;

    let mut seen_cols: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (_, df) in &tables {
        for col in df.get_column_names() {
            if !seen_cols.insert(col.to_string()) {
                return Err(DaxError::InvalidArgument(format!(
                    "CROSSJOIN does not allow two columns with the same name '{col}'"
                )));
            }
        }
    }

    let (first_name, mut acc) = tables.remove(0);
    for (_, df) in tables {
        acc = df_cross_join(&acc, &df)?;
    }
    Ok(Value::Table(first_name, acc))
}

// GENERATE(table1, table2) --------------------------------------------------

pub fn generate_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "GENERATE requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let t1_expr = it.next().expect("args.len() == 2 guarantees two elements");
    let t2_expr = it.next().expect("args.len() == 2 guarantees two elements");

    let (name1, df1) = match eval(t1_expr, fc, rc)? {
        Value::Table(n, df) => (n, df),
        other => {
            return Err(DaxError::Type(format!(
                "GENERATE: first argument must be a table, got {other:?}"
            )))
        }
    };
    let frame_table = name1.find('[').map_or(name1.as_str(), |i| &name1[..i]);

    let mut chunks: Vec<DataFrame> = Vec::new();
    let mut schema_checked = false;
    let mut cursor = RowFrameCursor::new(frame_table, &df1, &[&t2_expr], ctx)?;
    for row_idx in 0..df1.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);
        let (t2_name, df2) = match eval(t2_expr.clone(), fc, &rc_row)? {
            Value::Table(n, df) => (n, df),
            other => {
                return Err(DaxError::Type(format!(
                    "GENERATE: second argument must be a table, got {other:?}"
                )))
            }
        };
        if !schema_checked {
            schema_checked = true;
            let df1_prefix = table_prefix(&name1);
            let df1_qualified: std::collections::HashSet<String> = df1
                .get_column_names()
                .iter()
                .map(|c| qualify_col(c.as_str(), df1_prefix))
                .collect();
            let df2_prefix = table_prefix(&t2_name);
            for col in df2.get_column_names() {
                if df1_qualified.contains(&qualify_col(col.as_str(), df2_prefix)) {
                    return Err(DaxError::InvalidArgument(format!(
                        "GENERATE does not allow two columns with the same name '{col}'"
                    )));
                }
            }
        }
        if df2.height() == 0 {
            continue;
        }
        let single = df1.slice(row_idx as i64, 1);
        chunks.push(df_cross_join(&single, &df2)?);
    }

    if chunks.is_empty() {
        return Ok(Value::Table(name1, DataFrame::default()));
    }
    let mut result = chunks.remove(0);
    for chunk in chunks {
        result = result
            .vstack(&chunk)
            .map_err(|e| DaxError::Eval(format!("GENERATE vstack: {e}")))?;
    }
    Ok(Value::Table(name1, result))
}

// GENERATEALL(table1, table2) -----------------------------------------------

pub fn generateall_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 2 {
        return Err(DaxError::InvalidArgument(
            "GENERATEALL requires exactly 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let t1_expr = it.next().expect("args.len() == 2 guarantees two elements");
    let t2_expr = it.next().expect("args.len() == 2 guarantees two elements");

    let (name1, df1) = match eval(t1_expr, fc, rc)? {
        Value::Table(n, df) => (n, df),
        other => {
            return Err(DaxError::Type(format!(
                "GENERATEALL: first argument must be a table, got {other:?}"
            )))
        }
    };
    let frame_table = name1.find('[').map_or(name1.as_str(), |i| &name1[..i]);

    let mut chunks: Vec<DataFrame> = Vec::new();
    let mut empty_singles: Vec<DataFrame> = Vec::new();
    let mut t2_schema: Option<Vec<(String, polars::prelude::DataType)>> = None;

    let mut cursor = RowFrameCursor::new(frame_table, &df1, &[&t2_expr], ctx)?;
    for row_idx in 0..df1.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);
        let df2 = match eval(t2_expr.clone(), fc, &rc_row)? {
            Value::Table(_, df) => df,
            other => {
                return Err(DaxError::Type(format!(
                    "GENERATEALL: second argument must be a table, got {other:?}"
                )))
            }
        };
        let single = df1.slice(row_idx as i64, 1);
        if df2.height() > 0 {
            if t2_schema.is_none() {
                t2_schema = Some(
                    df2.columns()
                        .iter()
                        .map(|s| (s.name().to_string(), s.dtype().clone()))
                        .collect(),
                );
            }
            chunks.push(df_cross_join(&single, &df2)?);
        } else {
            empty_singles.push(single);
        }
    }
    drop(cursor);

    if let Some(schema) = &t2_schema {
        for single in empty_singles {
            let null_cols: Vec<Column> = schema
                .iter()
                .map(|(name, dtype)| {
                    Column::new_scalar(name.as_str().into(), Scalar::null(dtype.clone()), 1)
                })
                .collect();
            let null_df = DataFrame::new_infer_height(null_cols)
                .map_err(|e| DaxError::Eval(format!("GENERATEALL null row: {e}")))?;
            chunks.push(df_cross_join(&single, &null_df)?);
        }
    } else {
        return Ok(Value::Table(name1, df1));
    }

    if chunks.is_empty() {
        return Ok(Value::Table(name1, DataFrame::default()));
    }
    let mut result = chunks.remove(0);
    for chunk in chunks {
        result = result
            .vstack(&chunk)
            .map_err(|e| DaxError::Eval(format!("GENERATEALL vstack: {e}")))?;
    }
    Ok(Value::Table(name1, result))
}

// CURRENTGROUP() ------------------------------------------------------------

pub fn currentgroup_fn(
    _args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    match &rc.current_group {
        Some((name, df)) => Ok(Value::Table(name.clone(), df.clone())),
        None => Err(DaxError::InvalidArgument(
            "CURRENTGROUP() can only be used inside GROUPBY()".into(),
        )),
    }
}

// GROUPBY(table, col, ..., "Name", expr, ...) -------------------------------

pub fn groupby_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 {
        return Err(DaxError::InvalidArgument(
            "GROUPBY requires a table, at least one grouping column, and one name/expr pair".into(),
        ));
    }

    let mut it = args.into_iter();
    let table_expr = it
        .next()
        .expect("args.len() >= 3 guarantees a first element");
    let rest: Vec<BoundExprNode> = it.collect();

    let mut group_exprs: Vec<BoundExprNode> = Vec::new();
    let mut extensions: Vec<(String, BoundExprNode)> = Vec::new();

    let mut i = 0;
    while i < rest.len() {
        match &rest[i] {
            BoundExprNode::Column(_) if extensions.is_empty() => {
                group_exprs.push(rest[i].clone());
                i += 1;
            }
            BoundExprNode::Literal(lit) if matches!(&lit.value, LiteralValue::String(_)) => {
                let name = match &lit.value {
                    LiteralValue::String(s) => s.clone(),
                    _ => unreachable!(),
                };
                i += 1;
                let expr = rest
                    .get(i)
                    .ok_or_else(|| {
                        DaxError::InvalidArgument(
                            "GROUPBY: extension name must be followed by an expression".into(),
                        )
                    })?
                    .clone();
                extensions.push((name, expr));
                i += 1;
            }
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "GROUPBY: unexpected argument: {other:?}"
                )))
            }
        }
    }

    if group_exprs.is_empty() {
        return Err(DaxError::InvalidArgument(
            "GROUPBY requires at least one grouping column".into(),
        ));
    }
    if extensions.is_empty() {
        return Err(DaxError::InvalidArgument(
            "GROUPBY requires at least one name/expression pair".into(),
        ));
    }

    let (table_name, df) = match eval(table_expr, fc, rc)? {
        Value::Table(n, df) => (n, df),
        other => {
            return Err(DaxError::Type(format!(
                "GROUPBY: first argument must be a table, got {other:?}"
            )))
        }
    };

    let group_refs: Vec<(String, String)> = group_exprs
        .iter()
        .map(|e| match e {
            BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
            other => Err(DaxError::InvalidArgument(format!(
                "GROUPBY: grouping argument must be a column reference, got {other:?}"
            ))),
        })
        .collect::<DaxResult<_>>()?;

    let resolved_names: Vec<String> = group_refs
        .iter()
        .map(|(t, c)| {
            let qualified = TableCol::new(t, c).to_string();
            if df.column(&qualified).is_ok() {
                qualified
            } else {
                c.clone()
            }
        })
        .collect();

    let group_only = df
        .select(resolved_names.clone())
        .map_err(|e| DaxError::Eval(format!("GROUPBY: column select failed: {e}")))?;
    let mut result = group_only
        .unique_stable(
            Some(&resolved_names),
            polars::prelude::UniqueKeepStrategy::First,
            None,
        )
        .map_err(|e| DaxError::Eval(format!("GROUPBY: unique failed: {e}")))?;

    for (ext_name, ext_expr) in &extensions {
        let mut values: Vec<Value> = Vec::with_capacity(result.height());

        for row_idx in 0..result.height() {
            let mut mask = BooleanChunked::new("__groupby_mask__".into(), &vec![true; df.height()]);
            for (resolved, (_, col)) in resolved_names.iter().zip(group_refs.iter()) {
                let key_av = result
                    .column(resolved)
                    .expect("resolved was selected into result")
                    .as_materialized_series()
                    .get(row_idx)
                    .expect("row_idx is within bounds of result.height()");
                let key_single = ScalarValue::try_from(key_av)?.to_series(col);
                let col_series = df
                    .column(resolved)
                    .expect("resolved was validated by prior df.select")
                    .as_materialized_series();
                let key_casted = key_single
                    .cast(col_series.dtype())
                    .map_err(|e| DaxError::Eval(format!("GROUPBY: cast failed: {e}")))?;
                let col_mask = build_mask(col_series, &[FilterPredicate::In(key_casted)])?;
                mask = mask & col_mask;
            }
            let group_df = df
                .filter(&mask)
                .map_err(|e| DaxError::Eval(format!("GROUPBY: filter failed: {e}")))?;
            let rc_group = rc.with_current_group(table_name.clone(), group_df);
            values.push(eval(ext_expr.clone(), fc, &rc_group)?);
        }

        result
            .with_column(Value::to_series(&values, ext_name)?.into())
            .map_err(|e| DaxError::Eval(format!("GROUPBY: with_column failed: {e}")))?;
    }

    for ((t, c), resolved) in group_refs.iter().zip(resolved_names.iter()) {
        if TableCol::try_parse(resolved).is_none() {
            let qualified = TableCol::new(t, c).to_string();
            result.rename(resolved, qualified.as_str().into()).ok();
        }
    }

    Ok(Value::Table(table_name, result))
}

// ADDCOLUMNS(table, "Name", expr, ...) --------------------------------------

pub fn addcolumns_fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() < 3 || !(args.len() - 1).is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "ADDCOLUMNS requires a table followed by name/expression pairs".into(),
        ));
    }

    let mut it = args.into_iter();
    let table_expr = it.next().expect("args.len() >= 3");

    let mut pairs: Vec<(String, BoundExprNode)> = Vec::new();
    loop {
        match it.next() {
            None => break,
            Some(BoundExprNode::Literal(lit)) => match lit.value {
                LiteralValue::String(s) => {
                    let expr = it.next().expect("length validated above");
                    pairs.push((s, expr));
                }
                _ => {
                    return Err(DaxError::InvalidArgument(
                        "ADDCOLUMNS: column name must be a string literal".into(),
                    ))
                }
            },
            _ => {
                return Err(DaxError::InvalidArgument(
                    "ADDCOLUMNS: expected string literal for column name".into(),
                ))
            }
        }
    }

    let (table_name, mut df) = match eval(table_expr, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "ADDCOLUMNS: first argument must be a table, got {other:?}"
            )))
        }
    };

    let frame_table = table_name
        .find('[')
        .map_or(table_name.as_str(), |i| &table_name[..i]);

    // Single row-major pass so the sequential RowFrameCursor (forward-only)
    // can be shared across all pairs, instead of rebuilding a full row frame
    // once per (pair, row) as a column-major loop would require.
    let exprs: Vec<&BoundExprNode> = pairs.iter().map(|(_, e)| e).collect();
    let mut cursor = RowFrameCursor::new(frame_table, &df, &exprs, ctx)?;
    let mut pair_values: Vec<Vec<Value>> = pairs
        .iter()
        .map(|_| Vec::with_capacity(df.height()))
        .collect();
    for _ in 0..df.height() {
        let frame = cursor.next_frame();
        let rc_row = rc.with_frame(frame);
        for (values, (_, expr)) in pair_values.iter_mut().zip(pairs.iter()) {
            values.push(eval(expr.clone(), fc, &rc_row)?);
        }
    }
    drop(cursor);

    for ((col_name, _), values) in pairs.iter().zip(pair_values.iter()) {
        df.with_column(Value::to_series(values, col_name)?.into())
            .map_err(|e| DaxError::Eval(format!("ADDCOLUMNS: with_column failed: {e}")))?;
    }

    Ok(Value::Table(table_name, df))
}
