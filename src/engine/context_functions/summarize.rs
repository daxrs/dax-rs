use std::collections::HashMap;

use polars::prelude::{
    col, BooleanChunked, Column, DataFrame, DataFrameJoinOps, DataType, IntoLazy, JoinArgs,
    JoinType, NamedFrom, Scalar, Series,
};

use crate::engine::context::{ExecutionContext, FilterContext, FilterPredicate};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::ir::operator::{BoundSummarize, BoundSummarizeColumns, SummarizeExtensions};
use crate::engine::row_context::{RowContext, ScalarValue};
use crate::engine::table_col::TableCol;

use super::relationship::find_join_path;
use super::select_unique;

type RollupColumnRefs = Vec<(String, String)>;
type RollupGroups = Vec<Vec<(RollupColumnRefs, Option<String>)>>;
type RollupRefs = Vec<((String, String), Option<String>)>;

// Shared private helpers ----------------------------------------------------

fn make_key_predicate(
    ctx: &ExecutionContext,
    col_name: &str,
    owning_table: &str,
    av: polars::prelude::AnyValue<'_>,
    fn_name: &str,
) -> DaxResult<FilterPredicate> {
    let key_single = ScalarValue::try_from(av)?.to_series(col_name);
    let dtype = ctx
        .catalog
        .columns
        .get(&(owning_table.to_string(), col_name.to_string()))
        .map(|m| m.dtype.clone())
        .unwrap_or_else(|| key_single.dtype().clone());
    Ok(FilterPredicate::In(key_single.cast(&dtype).map_err(
        |e| DaxError::Eval(format!("{fn_name}: cast failed: {e}")),
    )?))
}

fn append_subtotal_rows(
    result: &mut DataFrame,
    columns: Vec<Series>,
    fn_name: &str,
) -> DaxResult<()> {
    let columns: Vec<Series> = columns
        .into_iter()
        .map(|s| {
            if let Ok(existing) = result.column(s.name().as_str()) {
                let target = existing.dtype();
                if s.dtype() != target {
                    return s.cast(target).unwrap_or(s);
                }
            }
            s
        })
        .collect();
    let sub_df =
        DataFrame::new_infer_height(columns.into_iter().map(|s| s.into()).collect::<Vec<_>>())
            .map_err(|e| {
                DaxError::Eval(format!("{fn_name}: DataFrame construction failed: {e}"))
            })?;
    result
        .vstack_mut(&sub_df)
        .map(|_| ())
        .map_err(|e| DaxError::Eval(format!("{fn_name}: vstack failed: {e}")))
}

fn enrich_with_foreign_cols(
    df: DataFrame,
    base_table: &str,
    foreign_cols: &HashMap<String, Vec<String>>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    fn_name: &str,
) -> DaxResult<DataFrame> {
    let mut enriched = df;
    for (foreign_table, cols_needed) in foreign_cols {
        let path = find_join_path(ctx, rc, base_table, foreign_table)?;
        for (i, step) in path.iter().enumerate() {
            let is_last = i == path.len() - 1;
            let right_df = ctx.get_filtered_df(&step.right_table, fc, rc)?;

            let mut to_select = vec![step.right_col.clone()];
            if is_last {
                for c in cols_needed {
                    if !to_select.contains(c) {
                        to_select.push(c.clone());
                    }
                }
            } else {
                let next_left = &path[i + 1].left_col;
                if !to_select.contains(next_left) {
                    to_select.push(next_left.clone());
                }
            }

            let right_slim = right_df
                .select(to_select)
                .map_err(|e| DaxError::Eval(format!("{fn_name}: select failed: {e}")))?;

            let left_col_resolved = if enriched.column(&step.left_col).is_ok() {
                step.left_col.clone()
            } else {
                enriched
                    .get_column_names()
                    .into_iter()
                    .find(|n| {
                        TableCol::try_parse(n.as_str())
                            .map(|tc| tc.col == step.left_col)
                            .unwrap_or(false)
                    })
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| step.left_col.clone())
            };

            enriched = enriched
                .join(
                    &right_slim,
                    [left_col_resolved.as_str()],
                    [step.right_col.as_str()],
                    JoinType::Left.into(),
                    None,
                )
                .map_err(|e| DaxError::Eval(format!("{fn_name}: join failed: {e}")))?;

            if !is_last {
                let next_left = &path[i + 1].left_col;
                let col_names = enriched.get_column_names();
                if next_left == &step.right_col
                    && &step.left_col != next_left
                    && !col_names.iter().any(|n| n.as_str() == next_left.as_str())
                {
                    let alias = enriched
                        .column(&step.left_col)
                        .map_err(|_| {
                            DaxError::Eval(format!(
                                "{fn_name}: column '{}' not found",
                                step.left_col
                            ))
                        })?
                        .as_materialized_series()
                        .clone()
                        .with_name(next_left.as_str().into());
                    enriched.with_column(alias.into()).map_err(|e| {
                        DaxError::Eval(format!("{fn_name}: with_column failed: {e}"))
                    })?;
                }
            }
        }
    }
    Ok(enriched)
}

// ISSUBTOTAL(column) --------------------------------------------------------

pub fn issubtotal_fn(
    args: Vec<BoundExprNode>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    rc: &RowContext,
    _eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    if args.len() != 1 {
        return Err(DaxError::InvalidArgument(
            "ISSUBTOTAL requires exactly 1 argument".into(),
        ));
    }
    let col = match &args[0] {
        BoundExprNode::Column(c) => c,
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "ISSUBTOTAL: expected column reference, got {other:?}"
            )))
        }
    };
    Ok(Value::Boolean(rc.is_subtotal(&col.table, &col.column)))
}

// SUMMARIZE -----------------------------------------------------------------

enum NativeAggKind {
    Sum,
    Min,
    Max,
    Average,
}

struct NativeAgg {
    kind: NativeAggKind,
    column: String,
}

fn native_aggregate_kind(expr: &BoundExprNode, table_name: &str) -> Option<NativeAgg> {
    let BoundExprNode::Function(f) = expr else {
        return None;
    };
    if f.args.len() != 1 {
        return None;
    }
    let BoundExprNode::Column(c) = &f.args[0] else {
        return None;
    };
    if c.table != table_name {
        return None;
    }
    let kind = match f.name.to_ascii_uppercase().as_str() {
        "SUM" => NativeAggKind::Sum,
        "MIN" => NativeAggKind::Min,
        "MAX" => NativeAggKind::Max,
        "AVERAGE" => NativeAggKind::Average,
        _ => return None,
    };
    Some(NativeAgg { kind, column: c.column.clone() })
}

/// Computes every native-compatible extension in `native_exts` with a single
/// `group_by(resolved_col_names).agg(...)` pass over `enriched`, then joins
/// the result into `result` on the group-by columns (matching nulls, since
/// blank-member rows have NULL group keys). Casts SUM/AVERAGE outputs to
/// match `functions/aggregation.rs`'s own widening rule (Int64/Int32 stay
/// integral, everything else promotes to Float64) so a native-computed
/// column is indistinguishable from one computed via the per-group path.
fn compute_native_extensions(
    mut result: DataFrame,
    enriched: &DataFrame,
    resolved_col_names: &[String],
    table_name: &str,
    native_exts: &[(String, BoundExprNode)],
) -> DaxResult<DataFrame> {
    if native_exts.is_empty() {
        return Ok(result);
    }

    let group_by_exprs: Vec<polars::prelude::Expr> =
        resolved_col_names.iter().map(|c| col(c.as_str())).collect();

    let agg_exprs: Vec<polars::prelude::Expr> = native_exts
        .iter()
        .map(|(name, expr)| {
            let agg = native_aggregate_kind(expr, table_name)
                .expect("caller only passes extensions already classified as native");
            let base = col(agg.column.as_str());
            let reduced = match agg.kind {
                NativeAggKind::Sum => base.sum(),
                NativeAggKind::Min => base.min(),
                NativeAggKind::Max => base.max(),
                NativeAggKind::Average => base.mean(),
            };
            reduced.alias(name.as_str())
        })
        .collect();

    let native_df = enriched
        .clone()
        .lazy()
        .group_by(group_by_exprs)
        .agg(agg_exprs)
        .collect()
        .map_err(|e| DaxError::Eval(format!("SUMMARIZE: native aggregation failed: {e}")))?;

    let mut join_args = JoinArgs::new(JoinType::Left);
    join_args.nulls_equal = true;
    result = result
        .join(
            &native_df,
            resolved_col_names,
            resolved_col_names,
            join_args,
            None,
        )
        .map_err(|e| DaxError::Eval(format!("SUMMARIZE: native aggregation join failed: {e}")))?;

    for (name, expr) in native_exts {
        let agg = native_aggregate_kind(expr, table_name)
            .expect("caller only passes extensions already classified as native");
        if matches!(agg.kind, NativeAggKind::Sum | NativeAggKind::Average) {
            let source_dtype = enriched
                .column(&agg.column)
                .map_err(|e| DaxError::Eval(format!("SUMMARIZE: {e}")))?
                .dtype()
                .clone();
            if !matches!(source_dtype, DataType::Int64 | DataType::Int32) {
                let series = result
                    .column(name)
                    .map_err(|e| DaxError::Eval(format!("SUMMARIZE: {e}")))?
                    .as_materialized_series()
                    .cast(&DataType::Float64)
                    .map_err(|e| DaxError::Eval(format!("SUMMARIZE: cast failed: {e}")))?;
                result
                    .with_column(series.into())
                    .map_err(|e| DaxError::Eval(format!("SUMMARIZE: with_column failed: {e}")))?;
            }
        }
    }

    Ok(result)
}

pub fn eval_summarize(
    node: BoundSummarize,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    let rollup_refs: RollupRefs = node
        .rollup_cols
        .iter()
        .map(|(e, flag)| {
            let tc = match e {
                BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
                other => Err(DaxError::InvalidArgument(format!(
                    "ROLLUP: expected column reference, got {other:?}"
                ))),
            }?;
            Ok((tc, flag.clone()))
        })
        .collect::<DaxResult<_>>()?;
    let extensions_for_subtotals: Vec<(String, BoundExprNode)> = node
        .extensions
        .iter()
        .map(|(n, e)| (n.clone(), e.clone()))
        .collect();

    let (table_name, df) = match eval(*node.table, fc, rc)? {
        Value::Table(name, df) => (name, df),
        other => {
            return Err(DaxError::Type(format!(
                "SUMMARIZE: first argument must be a table, got {other:?}"
            )))
        }
    };

    let group_refs: Vec<(String, String)> = node
        .group_by
        .iter()
        .map(|e| match e {
            BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
            BoundExprNode::Measure(m) if df.column(&m.name).is_ok() => {
                Ok((table_name.clone(), m.name.clone()))
            }
            other => Err(DaxError::InvalidArgument(format!(
                "SUMMARIZE: group-by must be a column reference, got {other:?}"
            ))),
        })
        .collect::<DaxResult<_>>()?;

    let mut foreign_cols: HashMap<String, Vec<String>> = HashMap::new();
    for (tbl, col) in &group_refs {
        if tbl != &table_name {
            let qualified = TableCol::new(tbl, col).to_string();
            if df.column(&qualified).is_err() && df.column(col.as_str()).is_err() {
                foreign_cols
                    .entry(tbl.clone())
                    .or_default()
                    .push(col.clone());
            }
        }
    }

    let enriched =
        enrich_with_foreign_cols(df, &table_name, &foreign_cols, ctx, fc, rc, "SUMMARIZE")?;

    let resolved_col_names: Vec<String> = group_refs
        .iter()
        .map(|(t, c)| {
            let qualified = TableCol::new(t, c).to_string();
            if enriched.column(&qualified).is_ok() {
                qualified
            } else {
                c.clone()
            }
        })
        .collect();

    let mut result = select_unique(&enriched, &resolved_col_names, "SUMMARIZE")?;

    if let Some(any_not_null) = result
        .columns()
        .iter()
        .map(|s| s.is_not_null())
        .reduce(|a, b| a | b)
    {
        result = result
            .filter(&any_not_null)
            .map_err(|e| DaxError::Eval(format!("SUMMARIZE: blank member filter failed: {e}")))?;
    }

    let (native_exts, non_native_exts): (Vec<_>, Vec<_>) = node
        .extensions
        .into_iter()
        .partition(|(_, expr)| native_aggregate_kind(expr, &table_name).is_some());

    result = compute_native_extensions(
        result,
        &enriched,
        &resolved_col_names,
        &table_name,
        &native_exts,
    )?;

    for (ext_name, ext_expr) in non_native_exts {
        let mut values: Vec<Value> = Vec::with_capacity(result.height());

        for row_idx in 0..result.height() {
            let mut group_fc = fc.clone();
            for ((owning_table, col_name), resolved) in
                group_refs.iter().zip(resolved_col_names.iter())
            {
                let key_series = result
                    .column(resolved)
                    .expect("resolved column names were successfully selected into result")
                    .as_materialized_series();
                let key_val = key_series
                    .get(row_idx)
                    .expect("row_idx is within bounds of result.height()");
                let predicate =
                    make_key_predicate(ctx, col_name, owning_table, key_val, "SUMMARIZE")?;
                group_fc
                    .filters
                    .entry((owning_table.clone(), col_name.clone()))
                    .or_default()
                    .push(predicate);
            }
            let expanded = ctx.expanded_filter_context(&group_fc, rc)?;
            values.push(eval(ext_expr.clone(), &expanded, rc)?);
        }

        result
            .with_column(Value::to_series(&values, &ext_name)?.into())
            .map_err(|e| DaxError::Eval(format!("SUMMARIZE: with_column failed: {e}")))?;
    }
    if !rollup_refs.is_empty() {
        result = generate_rollup_subtotals(
            result,
            &enriched,
            &group_refs,
            &rollup_refs,
            &extensions_for_subtotals,
            ctx,
            fc,
            rc,
            eval,
        )?;
    }

    for ((table, col), resolved) in group_refs.iter().zip(resolved_col_names.iter()) {
        // Only add the table prefix when the column is a real catalog column of
        // that table. Virtual columns (ROLLUPADDISSUBTOTAL flags, ADDCOLUMNS
        // additions, etc.) have no catalog entry and must keep their bare name
        // so that ORDER BY and downstream SUMMARIZE calls can find them.
        if TableCol::try_parse(resolved).is_none()
            && ctx
                .catalog
                .columns
                .contains_key(&(table.clone(), col.clone()))
        {
            let qualified = TableCol::new(table, col).to_string();
            result.rename(resolved, qualified.as_str().into()).ok();
        }
    }

    Ok(Value::Table(table_name, result))
}

// SUMMARIZECOLUMNS ----------------------------------------------------------

pub fn eval_summarize_columns(
    node: BoundSummarizeColumns,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value> {
    let mut base_fc = fc.clone();
    for filter_expr in node.filters {
        match eval(filter_expr, fc, rc)? {
            Value::Table(name, filter_df) => {
                let current = base_fc
                    .table_overrides
                    .remove(&name)
                    .or_else(|| ctx.tables.get(&name).cloned())
                    .ok_or_else(|| {
                        DaxError::UnknownName(format!(
                            "SUMMARIZECOLUMNS: unknown filter table '{name}'"
                        ))
                    })?;
                let join_cols: Vec<String> = filter_df
                    .get_column_names()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let restricted = current
                    .join(
                        &filter_df,
                        join_cols.clone(),
                        join_cols,
                        JoinType::Semi.into(),
                        None,
                    )
                    .map_err(|e| DaxError::Eval(format!("SUMMARIZECOLUMNS filter: {e}")))?;
                base_fc.table_overrides.insert(name, restricted);
            }
            other => {
                return Err(DaxError::Type(format!(
                    "SUMMARIZECOLUMNS: filter argument must return a table, got {other:?}"
                )))
            }
        }
    }

    let fixed_refs: Vec<(String, String)> = node
        .group_by_cols
        .iter()
        .map(|e| match e {
            BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
            other => Err(DaxError::InvalidArgument(format!(
                "SUMMARIZECOLUMNS: group-by must be a column reference, got {other:?}"
            ))),
        })
        .collect::<DaxResult<_>>()?;

    let rollup_groups: RollupGroups = node
        .rollup_groups
        .iter()
        .map(|axis| {
            axis.iter()
                .map(|(cols, flag)| {
                    let col_refs = cols
                        .iter()
                        .map(|e| match e {
                            BoundExprNode::Column(c) => Ok((c.table.clone(), c.column.clone())),
                            other => Err(DaxError::InvalidArgument(format!(
                                "SUMMARIZECOLUMNS: rollup column must be a column reference, got {other:?}"
                            ))),
                        })
                        .collect::<DaxResult<_>>()?;
                    Ok((col_refs, flag.clone()))
                })
                .collect::<DaxResult<_>>()
        })
        .collect::<DaxResult<_>>()?;

    let all_group_refs: Vec<(String, String)> = fixed_refs
        .iter()
        .cloned()
        .chain(
            rollup_groups
                .iter()
                .flatten()
                .flat_map(|(cols, _)| cols.iter().cloned()),
        )
        .collect();

    if all_group_refs.is_empty() {
        let mut eval_fc = base_fc.clone();
        eval_fc.outer_fc = Some(Box::new(fc.clone()));
        let expanded = ctx.expanded_filter_context(&eval_fc, rc)?;
        let has_non_ignore = node.extensions.iter().any(|(_, _, ig)| !ig);
        let mut all_non_ignore_blank = true;
        let mut result = DataFrame::default();
        for (ext_name, ext_expr, is_ignore) in node.extensions {
            let val = eval(ext_expr, &expanded, rc)?;
            if !is_ignore && !matches!(val, Value::Blank) {
                all_non_ignore_blank = false;
            }
            result
                .with_column(Value::to_series(&[val], &ext_name)?.into())
                .map_err(|e| {
                    DaxError::Eval(format!("SUMMARIZECOLUMNS: with_column failed: {e}"))
                })?;
        }
        if has_non_ignore && all_non_ignore_blank {
            result = result.clear();
        }
        return Ok(Value::Table(String::new(), result));
    }

    let base_table = {
        let mut seen = std::collections::HashSet::new();
        let mut best: Option<(&str, usize)> = None;
        for (tbl, _) in &all_group_refs {
            if seen.insert(tbl.as_str()) {
                let count = ctx
                    .catalog
                    .relationships
                    .iter()
                    .filter(|r| r.active && r.from_table == tbl.as_str())
                    .count();
                if best.as_ref().map(|(_, c)| count > *c).unwrap_or(true) {
                    best = Some((tbl.as_str(), count));
                }
            }
        }
        best.map(|(t, _)| t.to_string())
            .unwrap_or_else(|| all_group_refs[0].0.clone())
    };
    let df = ctx.get_filtered_df(&base_table, &base_fc, rc)?;

    let mut foreign_cols: HashMap<String, Vec<String>> = HashMap::new();
    for (tbl, col) in &all_group_refs {
        if tbl != &base_table {
            foreign_cols
                .entry(tbl.clone())
                .or_default()
                .push(col.clone());
        }
    }

    let enriched = enrich_with_foreign_cols(
        df,
        &base_table,
        &foreign_cols,
        ctx,
        &base_fc,
        rc,
        "SUMMARIZECOLUMNS",
    )?;

    let col_names: Vec<String> = all_group_refs.iter().map(|(_, c)| c.clone()).collect();
    let mut result = select_unique(&enriched, &col_names, "SUMMARIZECOLUMNS")?;

    let num_rows = result.height();
    let has_extensions = !node.extensions.is_empty();
    let has_non_ignore = node.extensions.iter().any(|(_, _, ig)| !ig);
    let mut all_non_ignore_blank: Vec<bool> = vec![true; num_rows];

    let extensions_for_subtotals: SummarizeExtensions = node
        .extensions
        .iter()
        .map(|(n, e, ig)| (n.clone(), e.clone(), *ig))
        .collect();

    for (ext_name, ext_expr, is_ignore) in node.extensions {
        let is_boolean_typed = !is_ignore && ext_expr.dtype() == Some(DataType::Boolean);
        let mut values: Vec<Value> = Vec::with_capacity(num_rows);

        for (row_idx, non_ignore_blank) in all_non_ignore_blank.iter_mut().enumerate() {
            let mut group_fc = base_fc.clone();
            group_fc.outer_fc = Some(Box::new(fc.clone()));
            group_fc.scoped_columns = all_group_refs.iter().cloned().collect();
            for (owning_table, col_name) in &all_group_refs {
                let key_series = result
                    .column(col_name)
                    .expect("all_group_refs columns were successfully selected into result")
                    .as_materialized_series();
                let key_val = key_series
                    .get(row_idx)
                    .expect("row_idx is within bounds of result.height()");
                let predicate =
                    make_key_predicate(ctx, col_name, owning_table, key_val, "SUMMARIZECOLUMNS")?;

                if let Some(override_df) = group_fc.table_overrides.get_mut(owning_table) {
                    let series = override_df
                        .column(col_name)
                        .map_err(|_| {
                            DaxError::Eval(format!(
                                "SUMMARIZECOLUMNS: column '{col_name}' not found in table override"
                            ))
                        })?
                        .as_materialized_series();
                    let mask = crate::engine::context::build_mask(
                        series,
                        std::slice::from_ref(&predicate),
                    )?;
                    *override_df = override_df.filter(&mask).map_err(|e| {
                        DaxError::Eval(format!("SUMMARIZECOLUMNS: filter failed: {e}"))
                    })?;
                } else {
                    group_fc
                        .filters
                        .entry((owning_table.clone(), col_name.clone()))
                        .or_default()
                        .push(predicate);
                }
            }
            let expanded = ctx.expanded_filter_context(&group_fc, rc)?;
            let val = eval(ext_expr.clone(), &expanded, rc)?;
            if !is_ignore && (is_boolean_typed || !matches!(val, Value::Blank)) {
                *non_ignore_blank = false;
            }
            values.push(val);
        }

        result
            .with_column(Value::to_series(&values, &ext_name)?.into())
            .map_err(|e| DaxError::Eval(format!("SUMMARIZECOLUMNS: with_column failed: {e}")))?;
    }
    if has_extensions {
        let blank_vec = if has_non_ignore {
            &all_non_ignore_blank
        } else {
            &vec![false; num_rows]
        };
        let keep: Vec<bool> = blank_vec.iter().map(|&b| !b).collect();
        let mask = BooleanChunked::new("keep".into(), keep);
        result = result
            .filter(&mask)
            .map_err(|e| DaxError::Eval(format!("SUMMARIZECOLUMNS: filter failed: {e}")))?;
    }

    if !rollup_groups.is_empty() {
        result = generate_rollup_subtotals_sc(
            result,
            &enriched,
            &fixed_refs,
            &rollup_groups,
            &extensions_for_subtotals,
            ctx,
            &base_fc,
            fc,
            rc,
            eval,
        )?;
    }

    for (table, col) in &all_group_refs {
        let qualified = TableCol::new(table, col).to_string();
        result.rename(col, qualified.as_str().into()).ok();
    }

    Ok(Value::Table(base_table, result))
}

fn rollup_cutoff_combinations(axis_lens: &[usize]) -> Vec<Vec<usize>> {
    let mut combos: Vec<Vec<usize>> = vec![vec![]];
    for &len in axis_lens {
        combos = combos
            .into_iter()
            .flat_map(|combo| {
                (0..=len).map(move |k| {
                    let mut next = combo.clone();
                    next.push(k);
                    next
                })
            })
            .collect();
    }
    combos.retain(|combo| combo.iter().zip(axis_lens).any(|(k, len)| k != len));
    combos
}

#[allow(clippy::too_many_arguments)]
fn generate_rollup_subtotals_sc(
    mut result: DataFrame,
    enriched: &DataFrame,
    fixed_refs: &[(String, String)],
    rollup_groups: &RollupGroups,
    extensions: &SummarizeExtensions,
    ctx: &ExecutionContext,
    base_fc: &FilterContext,
    outer_fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<DataFrame> {
    use std::collections::HashSet;

    for axis in rollup_groups {
        for (_, flag_opt) in axis {
            if let Some(flag_name) = flag_opt {
                let flag_col = Column::new_scalar(
                    flag_name.as_str().into(),
                    Scalar::from(false),
                    result.height(),
                );
                result.with_column(flag_col).map_err(|e| {
                    DaxError::Eval(format!("SUMMARIZECOLUMNS ROLLUP: with_column failed: {e}"))
                })?;
            }
        }
    }

    let all_rollup_cols: Vec<(String, String)> = rollup_groups
        .iter()
        .flatten()
        .flat_map(|(cols, _)| cols.iter().cloned())
        .collect();

    let axis_lens: Vec<usize> = rollup_groups.iter().map(|axis| axis.len()).collect();

    for combo in rollup_cutoff_combinations(&axis_lens) {
        let effective_group_refs: Vec<(String, String)> =
            fixed_refs
                .iter()
                .cloned()
                .chain(rollup_groups.iter().zip(&combo).flat_map(|(axis, &k)| {
                    axis[..k].iter().flat_map(|(cols, _)| cols.iter().cloned())
                }))
                .collect();

        let nulled_set: HashSet<(String, String)> = rollup_groups
            .iter()
            .zip(&combo)
            .flat_map(|(axis, &k)| axis[k..].iter().flat_map(|(cols, _)| cols.iter().cloned()))
            .collect();
        let rc_subtotals = rc.with_subtotal_cols(nulled_set.clone());

        let flag_values: std::collections::HashMap<&str, bool> = rollup_groups
            .iter()
            .zip(&combo)
            .flat_map(|(axis, &k)| {
                axis.iter()
                    .enumerate()
                    .filter_map(move |(j, (_, flag_opt))| {
                        flag_opt.as_deref().map(|flag_name| (flag_name, j >= k))
                    })
            })
            .collect();

        let effective_col_names: Vec<String> = effective_group_refs
            .iter()
            .map(|(_, c)| c.clone())
            .collect();

        let (sub_unique, sub_height) = if effective_col_names.is_empty() {
            (None, 1usize)
        } else {
            let df = select_unique(enriched, &effective_col_names, "SUMMARIZECOLUMNS ROLLUP")?;
            let h = df.height();
            (Some(df), h)
        };

        let has_non_ignore_ext = extensions.iter().any(|(_, _, ig)| !ig);
        let mut row_non_ignore_all_blank: Vec<bool> = vec![true; sub_height];
        let mut ext_series_map: Vec<(String, Series)> = Vec::new();
        for (ext_name, ext_expr, is_ignore) in extensions {
            let is_boolean_typed = !is_ignore && ext_expr.dtype() == Some(DataType::Boolean);
            let mut ext_values: Vec<Value> = Vec::with_capacity(sub_height);
            for (row_idx, non_ignore_blank) in row_non_ignore_all_blank.iter_mut().enumerate() {
                let mut group_fc = base_fc.clone();
                group_fc.outer_fc = Some(Box::new(outer_fc.clone()));
                group_fc.scoped_columns = effective_group_refs.iter().cloned().collect();
                for (owning_table, col_name) in &effective_group_refs {
                    let key_series = sub_unique
                        .as_ref()
                        .expect("effective_group_refs non-empty means sub_unique is Some")
                        .column(col_name)
                        .expect("col_name was selected into sub_unique")
                        .as_materialized_series();
                    let key_val = key_series
                        .get(row_idx)
                        .expect("row_idx is within bounds of sub_height");
                    let predicate = make_key_predicate(
                        ctx,
                        col_name,
                        owning_table,
                        key_val,
                        "SUMMARIZECOLUMNS ROLLUP",
                    )?;

                    if let Some(override_df) = group_fc.table_overrides.get_mut(owning_table) {
                        let series = override_df
                            .column(col_name)
                            .map_err(|_| DaxError::Eval(format!("SUMMARIZECOLUMNS ROLLUP: column '{col_name}' not found in table override")))?
                            .as_materialized_series();
                        let mask = crate::engine::context::build_mask(
                            series,
                            std::slice::from_ref(&predicate),
                        )?;
                        *override_df = override_df.filter(&mask).map_err(|e| {
                            DaxError::Eval(format!("SUMMARIZECOLUMNS ROLLUP: filter failed: {e}"))
                        })?;
                    } else {
                        group_fc
                            .filters
                            .entry((owning_table.clone(), col_name.clone()))
                            .or_default()
                            .push(predicate);
                    }
                }
                let expanded = ctx.expanded_filter_context(&group_fc, rc)?;
                let val = eval(ext_expr.clone(), &expanded, &rc_subtotals)?;
                if !is_ignore && (is_boolean_typed || !matches!(val, Value::Blank)) {
                    *non_ignore_blank = false;
                }
                ext_values.push(val);
            }
            ext_series_map.push((ext_name.clone(), Value::to_series(&ext_values, ext_name)?));
        }

        let (sub_unique, sub_height) = if has_non_ignore_ext
            && row_non_ignore_all_blank.iter().any(|&b| b)
        {
            let keep_ca = BooleanChunked::new(
                "keep".into(),
                row_non_ignore_all_blank
                    .iter()
                    .map(|&b| !b)
                    .collect::<Vec<bool>>(),
            );
            let filtered_unique = sub_unique
                .map(|df| {
                    df.filter(&keep_ca)
                        .map_err(|e| DaxError::Eval(format!("SUMMARIZECOLUMNS ROLLUP filter: {e}")))
                })
                .transpose()?;
            let filtered_height = keep_ca.sum().unwrap_or(0) as usize;
            ext_series_map = ext_series_map
                .into_iter()
                .map(|(n, s)| {
                    s.filter(&keep_ca).map(|fs| (n, fs)).map_err(|e| {
                        DaxError::Eval(format!("SUMMARIZECOLUMNS ROLLUP filter series: {e}"))
                    })
                })
                .collect::<DaxResult<_>>()?;
            (filtered_unique, filtered_height)
        } else {
            (sub_unique, sub_height)
        };

        let result_col_names: Vec<String> = result
            .get_column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut columns: Vec<Series> = Vec::new();
        for col_name in &result_col_names {
            if effective_col_names.contains(col_name) {
                let s = sub_unique
                    .as_ref()
                    .expect("effective_col_names non-empty means sub_unique is Some")
                    .column(col_name)
                    .expect("col_name was selected into sub_unique")
                    .as_materialized_series()
                    .clone()
                    .with_name(col_name.as_str().into());
                columns.push(s);
            } else if all_rollup_cols.iter().any(|(_, c)| c == col_name)
                && nulled_set.iter().any(|(_, c)| c == col_name)
            {
                let dtype = result
                    .column(col_name)
                    .expect("col_name comes from result.get_column_names()")
                    .dtype()
                    .clone();
                let null_series = Series::new_null(col_name.as_str().into(), sub_height)
                    .cast(&dtype)
                    .map_err(|e| {
                        DaxError::Eval(format!(
                            "SUMMARIZECOLUMNS ROLLUP: cast to null series dtype failed: {e}"
                        ))
                    })?;
                columns.push(null_series);
            } else if let Some(&is_true) = flag_values.get(col_name.as_str()) {
                columns.push(Series::new(
                    col_name.as_str().into(),
                    vec![is_true; sub_height],
                ));
            } else {
                let s = ext_series_map
                    .iter()
                    .find(|(name, _)| name == col_name)
                    .map(|(_, s)| s.clone())
                    .ok_or_else(|| {
                        DaxError::Eval(format!(
                            "SUMMARIZECOLUMNS ROLLUP: column '{col_name}' not found"
                        ))
                    })?;
                columns.push(s);
            }
        }

        append_subtotal_rows(&mut result, columns, "SUMMARIZECOLUMNS ROLLUP")?;
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn generate_rollup_subtotals(
    mut result: DataFrame,
    enriched: &DataFrame,
    group_refs: &[(String, String)],
    rollup_refs: &RollupRefs,
    extensions: &[(String, BoundExprNode)],
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<DataFrame> {
    use std::collections::HashSet;

    for (_, flag_opt) in rollup_refs {
        if let Some(flag_name) = flag_opt {
            let flag_col = Column::new_scalar(
                flag_name.as_str().into(),
                Scalar::from(false),
                result.height(),
            );
            result.with_column(flag_col).map_err(|e| {
                DaxError::Eval(format!("SUMMARIZE ROLLUP: with_column failed: {e}"))
            })?;
        }
    }

    let non_rollup_refs: Vec<(String, String)> = group_refs
        .iter()
        .filter(|r| !rollup_refs.iter().any(|((t, c), _)| (t, c) == (&r.0, &r.1)))
        .cloned()
        .collect();

    for k in (0..rollup_refs.len()).rev() {
        let active_rollup = &rollup_refs[0..k];
        let nulled_rollup = &rollup_refs[k..];

        let effective_group_refs: Vec<(String, String)> = non_rollup_refs
            .iter()
            .cloned()
            .chain(
                active_rollup
                    .iter()
                    .map(|((t, c), _)| (t.clone(), c.clone())),
            )
            .collect();

        let nulled_set: HashSet<(String, String)> = nulled_rollup
            .iter()
            .map(|((t, c), _)| (t.clone(), c.clone()))
            .collect();
        let rc_subtotals = rc.with_subtotal_cols(nulled_set);

        let flag_values: std::collections::HashMap<&str, bool> = rollup_refs
            .iter()
            .filter_map(|((_, _), flag_opt)| flag_opt.as_deref())
            .enumerate()
            .map(|(i, flag_name)| (flag_name, i >= k))
            .collect();

        let effective_col_names: Vec<String> = effective_group_refs
            .iter()
            .map(|(_, c)| c.clone())
            .collect();

        let (sub_unique, sub_height) = if effective_col_names.is_empty() {
            (None, 1usize)
        } else {
            let df = select_unique(enriched, &effective_col_names, "SUMMARIZE ROLLUP")?;
            let h = df.height();
            (Some(df), h)
        };

        let mut ext_series_map: Vec<(String, Series)> = Vec::new();
        for (ext_name, ext_expr) in extensions {
            let mut ext_values: Vec<Value> = Vec::with_capacity(sub_height);
            for row_idx in 0..sub_height {
                let mut group_fc = fc.clone();
                for (owning_table, col_name) in &effective_group_refs {
                    let key_series = sub_unique
                        .as_ref()
                        .expect("effective_group_refs non-empty means sub_unique is Some")
                        .column(col_name)
                        .expect("col_name was selected into sub_unique")
                        .as_materialized_series();
                    let key_val = key_series
                        .get(row_idx)
                        .expect("row_idx is within bounds of sub_height");
                    let predicate = make_key_predicate(
                        ctx,
                        col_name,
                        owning_table,
                        key_val,
                        "SUMMARIZE ROLLUP",
                    )?;
                    group_fc
                        .filters
                        .entry((owning_table.clone(), col_name.clone()))
                        .or_default()
                        .push(predicate);
                }
                let expanded = ctx.expanded_filter_context(&group_fc, rc)?;
                ext_values.push(eval(ext_expr.clone(), &expanded, &rc_subtotals)?);
            }
            ext_series_map.push((ext_name.clone(), Value::to_series(&ext_values, ext_name)?));
        }

        let result_col_names: Vec<String> = result
            .get_column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mut columns: Vec<Series> = Vec::new();
        for col_name in &result_col_names {
            if effective_col_names.contains(col_name) {
                let s = sub_unique
                    .as_ref()
                    .expect("effective_col_names non-empty means sub_unique is Some")
                    .column(col_name)
                    .expect("col_name was selected into sub_unique")
                    .as_materialized_series()
                    .clone()
                    .with_name(col_name.as_str().into());
                columns.push(s);
            } else if nulled_rollup.iter().any(|((_, c), _)| c == col_name) {
                let dtype = result
                    .column(col_name)
                    .expect("col_name comes from result.get_column_names()")
                    .dtype()
                    .clone();
                let null_series = Series::new_null(col_name.as_str().into(), sub_height)
                    .cast(&dtype)
                    .map_err(|e| {
                        DaxError::Eval(format!(
                            "SUMMARIZE ROLLUP: cast to null series dtype failed: {e}"
                        ))
                    })?;
                columns.push(null_series);
            } else if let Some(&is_true) = flag_values.get(col_name.as_str()) {
                columns.push(Series::new(
                    col_name.as_str().into(),
                    vec![is_true; sub_height],
                ));
            } else {
                let s = ext_series_map
                    .iter()
                    .find(|(name, _)| name == col_name)
                    .map(|(_, s)| s.clone())
                    .ok_or_else(|| {
                        DaxError::Eval(format!("SUMMARIZE ROLLUP: column '{col_name}' not found"))
                    })?;
                columns.push(s);
            }
        }

        append_subtotal_rows(&mut result, columns, "SUMMARIZE ROLLUP")?;
    }

    Ok(result)
}
