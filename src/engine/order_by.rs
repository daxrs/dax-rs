use crate::engine::dax::ast::{DaxExpr, Literal, SortDir};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::table_col::TableCol;
use polars::prelude::{
    BooleanChunked, ChunkCompareEq, ChunkCompareIneq, DataFrame, DataType, SortMultipleOptions,
};

pub(crate) fn apply_order_by(
    value: Value,
    order_by: Vec<(DaxExpr, SortDir)>,
    start_at: Vec<DaxExpr>,
) -> DaxResult<Value> {
    let Value::Table(name, df) = value else {
        return Ok(value);
    };
    let available: Vec<String> = df
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut col_names: Vec<String> = Vec::new();
    let mut descending: Vec<bool> = Vec::new();
    for (expr, dir) in &order_by {
        let col = resolve_order_by_column(expr, &available)?;
        col_names.push(col);
        descending.push(matches!(dir, SortDir::Desc));
    }
    let df = if start_at.is_empty() {
        df
    } else {
        apply_start_at_filter(df, &col_names, &descending, &start_at)?
    };
    let sorted = df
        .sort(
            col_names,
            SortMultipleOptions::new().with_order_descending_multi(descending),
        )
        .map_err(|e| DaxError::Eval(format!("ORDER BY: {e}")))?;
    Ok(Value::Table(name, sorted))
}

fn apply_start_at_filter(
    df: DataFrame,
    col_names: &[String],
    descending: &[bool],
    start_at: &[DaxExpr],
) -> DaxResult<DataFrame> {
    let n = start_at.len().min(col_names.len());
    let height = df.height();

    let mut include_mask: BooleanChunked = std::iter::repeat_n(false, height).collect();
    let mut eq_prefix: BooleanChunked = std::iter::repeat_n(true, height).collect();

    for i in 0..n {
        let series = df
            .column(&col_names[i])
            .map_err(|_| {
                DaxError::Eval(format!(
                    "START AT: column '{}' not found in result",
                    col_names[i]
                ))
            })?
            .as_materialized_series();

        let is_last = i == n - 1;
        let (progress, eq) = start_at_masks(series, &start_at[i], descending[i], is_last)?;

        let condition = &eq_prefix & &progress;
        include_mask = &include_mask | &condition;
        if !is_last {
            eq_prefix = &eq_prefix & &eq;
        }
    }

    df.filter(&include_mask)
        .map_err(|e| DaxError::Eval(format!("START AT: filter failed: {e}")))
}

fn start_at_masks(
    series: &polars::prelude::Series,
    val: &DaxExpr,
    descending: bool,
    is_last: bool,
) -> DaxResult<(BooleanChunked, BooleanChunked)> {
    match val {
        DaxExpr::Literal(Literal::Number(n)) => {
            let f = *n;
            let col_f = series.cast(&DataType::Float64).map_err(|e| {
                DaxError::Eval(format!("START AT: cannot cast column to float: {e}"))
            })?;
            let ca = col_f.f64().map_err(|e| DaxError::Eval(e.to_string()))?;
            let progress = if descending {
                if is_last {
                    ca.lt_eq(f)
                } else {
                    ca.lt(f)
                }
            } else {
                if is_last {
                    ca.gt_eq(f)
                } else {
                    ca.gt(f)
                }
            };
            let eq = ca.equal(f);
            Ok((progress, eq))
        }
        DaxExpr::Literal(Literal::String(s)) => {
            let ca = series.str().map_err(|_| {
                DaxError::Eval(format!(
                    "START AT: expected string column, got {:?}",
                    series.dtype()
                ))
            })?;
            let sv = s.as_str();
            let progress: BooleanChunked = ca
                .no_null_iter()
                .map(|v| {
                    if descending {
                        if is_last {
                            v <= sv
                        } else {
                            v < sv
                        }
                    } else {
                        if is_last {
                            v >= sv
                        } else {
                            v > sv
                        }
                    }
                })
                .collect();
            let eq: BooleanChunked = ca.no_null_iter().map(|v| v == sv).collect();
            Ok((progress, eq))
        }
        other => Err(DaxError::Eval(format!(
            "START AT: unsupported value expression: {other:?}"
        ))),
    }
}

fn resolve_order_by_column(expr: &DaxExpr, available: &[String]) -> DaxResult<String> {
    let candidates: Vec<String> = match expr {
        DaxExpr::ColumnRef { table, column } => {
            vec![column.clone(), TableCol::new(table, column).to_string()]
        }
        DaxExpr::Identifier(name) => vec![name.clone()],
        DaxExpr::MeasureRef(name) => vec![name.clone()],
        _ => {
            return Err(DaxError::InvalidArgument(
                "ORDER BY: unsupported expression (only column refs and identifiers)".into(),
            ))
        }
    };
    for candidate in &candidates {
        if let Some(found) = available.iter().find(|c| c.eq_ignore_ascii_case(candidate)) {
            return Ok(found.clone());
        }
    }
    Err(DaxError::UnknownName(format!(
        "ORDER BY: column '{}' not found in result set",
        candidates[0]
    )))
}
