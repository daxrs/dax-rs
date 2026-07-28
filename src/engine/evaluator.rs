use crate::engine::context::{ExecutionContext, FilterContext, FilterPredicate};
use crate::engine::context_functions::{
    apply_all_modifier, apply_allexcept_modifier, apply_allselected_modifier,
    apply_crossfilter_modifier, apply_treatas_modifier, apply_userelationship_modifier,
    eval_summarize, eval_summarize_columns,
};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::functions::{FunctionEntry, REGISTRY};
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::ir::operator::{BinaryOperator, BoundBinaryOp, LiteralValue, UnaryOperator};
use crate::engine::row_context::RowContext;
use crate::engine::table_col::TableCol;
use polars::prelude::{
    BooleanChunked, ChunkCompareEq, ChunkCompareIneq, DataFrame, DataFrameJoinOps, DataType,
    IntoSeries, JoinType, NamedFrom, Series,
};

pub struct Evaluator;

impl Evaluator {
    pub fn eval(
        expr: BoundExprNode,
        ctx: &ExecutionContext,
        filter_ctx: &FilterContext,
        row_ctx: &RowContext,
    ) -> DaxResult<Value> {
        match expr {
            BoundExprNode::Literal(bound) => Ok(match bound.value {
                LiteralValue::Integer(i) => Value::Integer(i),
                LiteralValue::Number(n) => Value::Number(n),
                LiteralValue::String(s) => Value::String(s),
                LiteralValue::Boolean(b) => Value::Boolean(b),
                LiteralValue::DateTime(ms) => Value::DateTime(ms),
                LiteralValue::CrossFilterDirection(_) => {
                    unreachable!("CrossFilterDirection literal is only valid inside CROSSFILTER()")
                }
                LiteralValue::Blank => {
                    unreachable!("Blank literal is only ever produced at runtime by eval_to_literal, never parsed")
                }
            }),

            BoundExprNode::BinaryOp(op) => {
                let l = Self::eval(*op.left, ctx, filter_ctx, row_ctx)?;
                let r = Self::eval(*op.right, ctx, filter_ctx, row_ctx)?;
                Self::eval_binary(l, r, op.op)
            }

            BoundExprNode::UnaryOp(op) => {
                let val = Self::eval(*op.expr, ctx, filter_ctx, row_ctx)?;
                match op.op {
                    UnaryOperator::Not => match val {
                        Value::Boolean(b) => Ok(Value::Boolean(!b)),
                        Value::Blank => Ok(Value::Boolean(true)),
                        Value::Series(s) => {
                            let ca = s.bool().map_err(|_| {
                                DaxError::Type("NOT: expected boolean series".into())
                            })?;
                            Ok(Value::Series((!ca).into_series()))
                        }
                        other => Err(DaxError::Type(format!(
                            "NOT: expected a boolean value, got {other:?}"
                        ))),
                    },
                    UnaryOperator::Negate => match val {
                        Value::Integer(i) => Ok(Value::Integer(-i)),
                        Value::Number(n) => Ok(Value::Number(-n)),
                        Value::Blank => Ok(Value::Blank),
                        other => Err(DaxError::Type(format!(
                            "Negate: expected a numeric value, got {other:?}"
                        ))),
                    },
                }
            }

            BoundExprNode::Column(c) => {
                // 1. Per-row scalar binding (SUMX iteration)
                if let Some(scalar) = row_ctx.lookup(&c.table, &c.column) {
                    return Ok(Value::from(scalar.clone()));
                }

                // 2. Vectorised table scope (FILTER condition evaluation).
                // If the exact table name isn't in scope (e.g. SUMMARIZECOLUMNS merged
                // columns from multiple tables into one DataFrame), fall back to any
                // scoped table that has the column by unqualified name.
                let qualified = TableCol::new(&c.table, &c.column).to_string();
                let scope_df_opt = row_ctx.table_scope.get(&c.table).or_else(|| {
                    row_ctx
                        .table_scope
                        .values()
                        .find(|df| df.column(&c.column).is_ok() || df.column(&qualified).is_ok())
                });
                if let Some(scope_df) = scope_df_opt {
                    let raw = scope_df
                        .column(&c.column)
                        .or_else(|_| scope_df.column(&qualified))
                        .expect("column existence was confirmed by the is_ok() check above")
                        .as_materialized_series()
                        .clone();
                    return Ok(Value::Series(raw.with_name(qualified.as_str().into())));
                }

                // 3. Global table with filter context applied
                let expanded = ctx.expanded_filter_context(filter_ctx, row_ctx)?;
                let df = ctx.get_filtered_df(&c.table, &expanded, row_ctx)?;
                let raw = df
                    .column(&c.column)
                    .map_err(|_| {
                        DaxError::UnknownName(format!(
                            "Column '{}' not found in table '{}'",
                            c.column, c.table
                        ))
                    })?
                    .as_materialized_series()
                    .clone();
                Ok(Value::Series(raw.with_name(qualified.as_str().into())))
            }

            BoundExprNode::Measure(m) => {
                if let Some(tree) = ctx.resolved_measures.get(&m.name) {
                    return Self::eval(tree.clone(), ctx, filter_ctx, row_ctx);
                }
                // Fallback: treat as a dynamic column in the current table scope.
                // Covers ROLLUPADDISSUBTOTAL flag columns (e.g. [IsGrandTotalRowTotal])
                // referenced by bracket-only syntax in TOPN sort keys or ORDER BY.
                // Also handles qualified names like "Product[Color]" from SUMMARIZE output
                // when referenced as bare [Color].
                let suffix = format!("[{}]", m.name);
                for scope_df in row_ctx.table_scope.values() {
                    if let Ok(col) = scope_df.column(&m.name) {
                        return Ok(Value::Series(col.as_materialized_series().clone()));
                    }
                    if let Some(qname) = scope_df
                        .get_column_names()
                        .iter()
                        .find(|n| n.as_str().ends_with(&suffix))
                    {
                        if let Ok(col) = scope_df.column(qname.as_str()) {
                            return Ok(Value::Series(col.as_materialized_series().clone()));
                        }
                    }
                }
                if let Some(val) = row_ctx.lookup_measure(&m.name) {
                    return Ok(Value::from(val.clone()));
                }
                Err(DaxError::UnknownName(format!(
                    "Measure '{}' not found",
                    m.name
                )))
            }

            BoundExprNode::Var(v) => {
                let mut scoped_rc = row_ctx.clone();
                for (name, bound) in v.bindings {
                    let value = Self::eval(bound, ctx, filter_ctx, &scoped_rc)?;
                    scoped_rc = scoped_rc.with_var(name, value);
                }
                Self::eval(*v.result, ctx, filter_ctx, &scoped_rc)
            }

            BoundExprNode::VarRef(name) => row_ctx
                .get_var(&name)
                .cloned()
                .ok_or_else(|| DaxError::UnknownName(format!("Unknown variable '{name}'"))),

            BoundExprNode::Table(t) => {
                if !ctx.tables.contains_key(&t.name) {
                    return Err(DaxError::UnknownName(format!("Unknown table: {}", t.name)));
                }
                let expanded = ctx.expanded_filter_context(filter_ctx, row_ctx)?;
                let mut df = ctx.get_filtered_df(&t.name, &expanded, row_ctx)?;

                if ctx
                    .catalog
                    .relationships
                    .iter()
                    .any(|r| r.to_table == t.name)
                {
                    if let Some(any_not_null) = df
                        .columns()
                        .iter()
                        .map(|c| c.as_materialized_series().is_not_null())
                        .reduce(|a, b| a | b)
                    {
                        df = df.filter(&any_not_null).map_err(|e| {
                            DaxError::Eval(format!(
                                "blank member filter failed for '{}': {e}",
                                t.name
                            ))
                        })?;
                    }
                }

                Ok(Value::Table(t.name, df))
            }

            BoundExprNode::Function(f) => {
                let entry = REGISTRY.get(&f.name).ok_or_else(|| {
                    DaxError::UnknownName(format!("Unknown function: '{}'", f.name))
                })?;
                match entry {
                    FunctionEntry::CallByValue(func, _) => {
                        let evaluated: Vec<Value> = f
                            .args
                            .into_iter()
                            .map(|arg| Self::eval(arg, ctx, filter_ctx, row_ctx))
                            .collect::<DaxResult<_>>()?;
                        func(evaluated, ctx, filter_ctx, row_ctx)
                    }
                    FunctionEntry::Context(func, _) => {
                        let eval_fn = |expr: BoundExprNode, fc: &FilterContext, rc: &RowContext| {
                            Self::eval(expr, ctx, fc, rc)
                        };
                        func(f.args, ctx, filter_ctx, row_ctx, &eval_fn)
                    }
                }
            }

            BoundExprNode::TableConstructor(rows) => {
                use polars::prelude::DataFrame;
                let num_cols = rows.first().map(|r| r.len()).unwrap_or(1).max(1);
                let mut col_values: Vec<Vec<Value>> = (0..num_cols).map(|_| Vec::new()).collect();
                for row in rows {
                    for (i, expr) in row.into_iter().enumerate() {
                        col_values[i].push(Self::eval(expr, ctx, filter_ctx, row_ctx)?);
                    }
                }
                let col_name = |i: usize| format!("Value{}", i + 1);
                let series: Vec<Series> = col_values
                    .into_iter()
                    .enumerate()
                    .map(|(i, vals)| Value::to_series(&vals, &col_name(i)))
                    .collect::<DaxResult<_>>()?;
                let df = DataFrame::new_infer_height(
                    series.into_iter().map(|s| s.into()).collect::<Vec<_>>(),
                )
                .map_err(|e| DaxError::Eval(format!("Table constructor: {e}")))?;
                Ok(Value::Table("__ctor__".to_string(), df))
            }

            BoundExprNode::SummarizeColumns(sc) => {
                let eval_fn = |expr: BoundExprNode, fc: &FilterContext, rc: &RowContext| {
                    Self::eval(expr, ctx, fc, rc)
                };
                eval_summarize_columns(sc, ctx, filter_ctx, row_ctx, &eval_fn)
            }

            BoundExprNode::Summarize(s) => {
                let eval_fn = |expr: BoundExprNode, fc: &FilterContext, rc: &RowContext| {
                    Self::eval(expr, ctx, fc, rc)
                };
                eval_summarize(s, ctx, filter_ctx, row_ctx, &eval_fn)
            }

            BoundExprNode::Calculate(calc) => {
                let mut new_fc = filter_ctx.clone();
                new_fc.outer_fc = Some(Box::new(filter_ctx.clone()));
                // Track which columns have had their outer filter cleared in this
                // CALCULATE call. First predicate on a column clears the outer
                // filter; subsequent predicates on the same column AND together.
                let mut replaced_in_call = std::collections::HashSet::<(String, String)>::new();

                // DAX semantics: ALL-type modifiers are always evaluated before
                // column predicate filters, regardless of argument order. Collect
                // predicates and defer them until after all modifiers are applied.
                let mut deferred_preds: Vec<BoundBinaryOp> = Vec::new();

                for filter_arg in calc.filters {
                    match filter_arg {
                        BoundExprNode::BinaryOp(op) => {
                            deferred_preds.push(op);
                        }

                        // ALL / REMOVEFILTERS / ALLEXCEPT / TREATAS / table-returning modifier
                        BoundExprNode::Function(func) => {
                            match func.name.to_ascii_uppercase().as_str() {
                                "ALL" | "REMOVEFILTERS" => {
                                    apply_all_modifier(&func, &mut new_fc, ctx)?;
                                }
                                "ALLEXCEPT" => {
                                    apply_allexcept_modifier(&func, &mut new_fc)?;
                                }
                                "TREATAS" => {
                                    let eval_fn = |expr: BoundExprNode,
                                                   fc: &FilterContext,
                                                   rc: &RowContext| {
                                        Self::eval(expr, ctx, fc, rc)
                                    };
                                    let current_fc = new_fc.clone();
                                    apply_treatas_modifier(func.args, &mut new_fc, ctx, &current_fc, row_ctx, &eval_fn)?;
                                }
                                "USERELATIONSHIP" => {
                                    apply_userelationship_modifier(func.args, ctx, &mut new_fc)?;
                                }
                                "CROSSFILTER" => {
                                    apply_crossfilter_modifier(func.args, ctx, &mut new_fc)?;
                                }
                                "ALLSELECTED" => {
                                    apply_allselected_modifier(&func, filter_ctx, &mut new_fc)?;
                                }
                                "KEEPFILTERS" => {
                                    let mut kf_args = func.args;
                                    if kf_args.len() != 1 {
                                        return Err(DaxError::InvalidArgument(
                                            "KEEPFILTERS requires exactly 1 argument".into(),
                                        ));
                                    }
                                    let inner = kf_args.remove(0);
                                    match inner {
                                        BoundExprNode::BinaryOp(op) => {
                                            Self::apply_predicate_filter_keep(
                                                op, &mut new_fc, ctx, filter_ctx, row_ctx,
                                            )?;
                                        }
                                        other => {
                                            let eval_fn = |expr: BoundExprNode,
                                                           fc: &FilterContext,
                                                           rc: &RowContext| {
                                                Self::eval(expr, ctx, fc, rc)
                                            };
                                            match eval_fn(other, &new_fc, row_ctx)? {
                                                Value::Table(tname, df) => {
                                                    Self::apply_keepfilters_table_override(
                                                        ctx, &tname, df, &mut new_fc, row_ctx,
                                                    )?;
                                                }
                                                other => return Err(DaxError::Type(format!(
                                                    "KEEPFILTERS: expected predicate or table, got {other:?}"
                                                ))),
                                            }
                                        }
                                    }
                                }
                                _ => {
                                    // Table-returning modifier (e.g. FILTER)
                                    let eval_fn = |expr: BoundExprNode,
                                                   fc: &FilterContext,
                                                   rc: &RowContext| {
                                        Self::eval(expr, ctx, fc, rc)
                                    };
                                    let entry = REGISTRY.get(&func.name)
                                        .ok_or_else(|| DaxError::UnknownName(format!(
                                            "CALCULATE: unknown function modifier '{}'", func.name
                                        )))?;
                                    let FunctionEntry::Context(func_ptr, _) = entry else {
                                        return Err(DaxError::InvalidArgument(format!(
                                            "CALCULATE: function modifier '{}' must be a context function",
                                            func.name
                                        )));
                                    };
                                    let val = func_ptr(func.args, ctx, &new_fc, row_ctx, &eval_fn)?;
                                    match val {
                                        Value::Table(tname, df) => {
                                            new_fc.table_overrides.insert(tname, df);
                                        }
                                        other => return Err(DaxError::Type(format!(
                                            "CALCULATE: function modifier must return a table, got {other:?}"
                                        ))),
                                    }
                                }
                            }
                        }

                        BoundExprNode::UnaryOp(uop) if matches!(uop.op, UnaryOperator::Not) => {
                            match *uop.expr {
                                BoundExprNode::BinaryOp(inner) if matches!(inner.op, BinaryOperator::In) => {
                                    deferred_preds.push(BoundBinaryOp {
                                        left: inner.left,
                                        right: inner.right,
                                        op: BinaryOperator::NotIn,
                                        dtype: inner.dtype,
                                    });
                                }
                                other => return Err(DaxError::InvalidArgument(format!(
                                    "Unsupported CALCULATE filter argument: NOT applied to {other:?}"
                                ))),
                            }
                        }

                        other => return Err(DaxError::InvalidArgument(format!(
                            "Unsupported CALCULATE filter argument: {other:?}"
                        ))),
                    }
                }

                // Pass 2: apply deferred column predicate filters
                for op in deferred_preds {
                    Self::apply_predicate_filter(
                        op,
                        &mut new_fc,
                        ctx,
                        &mut replaced_in_call,
                        filter_ctx,
                        row_ctx,
                    )?;
                }

                let expanded = ctx.expanded_filter_context(&new_fc, row_ctx)?;
                Self::eval(*calc.expression, ctx, &expanded, row_ctx)
            }
        }
    }

    fn resolve_predicate(
        op: BoundBinaryOp,
        ctx: &ExecutionContext,
        fc: &FilterContext,
        rc: &RowContext,
    ) -> DaxResult<(String, String, FilterPredicate)> {
        if matches!(op.op, BinaryOperator::In | BinaryOperator::NotIn) {
            let negate = matches!(op.op, BinaryOperator::NotIn);
            if let BoundExprNode::Column(col) = *op.left {
                let dtype = ctx
                    .catalog
                    .columns
                    .get(&(col.table.clone(), col.column.clone()))
                    .ok_or_else(|| {
                        DaxError::UnknownName(format!(
                            "Unknown column '{}.{}' in CALCULATE filter",
                            col.table, col.column
                        ))
                    })?
                    .dtype
                    .clone();
                let rhs_val = Self::eval(*op.right, ctx, fc, rc)?;
                let series = match rhs_val {
                    Value::Table(_, df) => df
                        .columns()
                        .first()
                        .ok_or_else(|| {
                            DaxError::Eval("CALCULATE IN: table constructor has no columns".into())
                        })?
                        .as_materialized_series()
                        .cast(&dtype)
                        .map_err(|e| DaxError::Eval(format!("CALCULATE IN: cast failed: {e}")))?,
                    other => {
                        return Err(DaxError::Type(format!(
                            "CALCULATE IN: right-hand side must be a table, got {other:?}"
                        )))
                    }
                };
                let predicate = if negate {
                    FilterPredicate::NotIn(series)
                } else {
                    FilterPredicate::In(series)
                };
                return Ok((col.table, col.column, predicate));
            }
        }

        let (col, lit, operator) = match (*op.left, *op.right) {
            (BoundExprNode::Column(col), BoundExprNode::Literal(bound)) => {
                (col, bound.value, op.op)
            }
            (BoundExprNode::Literal(bound), BoundExprNode::Column(col)) => {
                (col, bound.value, op.op.flip())
            }
            (BoundExprNode::Column(col), rhs) => {
                let lit = Self::eval_to_literal(rhs, ctx, fc, rc)?;
                (col, lit, op.op)
            }
            (lhs, BoundExprNode::Column(col)) => {
                let lit = Self::eval_to_literal(lhs, ctx, fc, rc)?;
                (col, lit, op.op.flip())
            }
            _ => {
                return Err(DaxError::InvalidArgument(
                    "CALCULATE filter must be 'Column op expr'".into(),
                ))
            }
        };

        let dtype = ctx
            .catalog
            .columns
            .get(&(col.table.clone(), col.column.clone()))
            .ok_or_else(|| {
                DaxError::UnknownName(format!(
                    "Unknown column '{}.{}' in CALCULATE filter",
                    col.table, col.column
                ))
            })?
            .dtype
            .clone();

        let predicate = match operator {
            BinaryOperator::Eq => FilterPredicate::In(
                Self::lit_to_series(lit)
                    .cast(&dtype)
                    .map_err(|e| DaxError::Eval(format!("CALCULATE filter: cast failed: {e}")))?,
            ),
            BinaryOperator::Neq => FilterPredicate::NotIn(
                Self::lit_to_series(lit)
                    .cast(&dtype)
                    .map_err(|e| DaxError::Eval(format!("CALCULATE filter: cast failed: {e}")))?,
            ),
            BinaryOperator::Gt => FilterPredicate::Gt(lit),
            BinaryOperator::Lt => FilterPredicate::Lt(lit),
            BinaryOperator::Gte => FilterPredicate::Gte(lit),
            BinaryOperator::Lte => FilterPredicate::Lte(lit),
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "Unsupported operator {other:?} in CALCULATE filter"
                )))
            }
        };

        Ok((col.table, col.column, predicate))
    }

    fn apply_predicate_filter(
        op: BoundBinaryOp,
        fc: &mut FilterContext,
        ctx: &ExecutionContext,
        replaced: &mut std::collections::HashSet<(String, String)>,
        outer_fc: &FilterContext,
        outer_rc: &RowContext,
    ) -> DaxResult<()> {
        let (table, column, predicate) = Self::resolve_predicate(op, ctx, outer_fc, outer_rc)?;
        let key = (table, column);
        if replaced.insert(key.clone()) {
            fc.filters.remove(&key);
        }
        fc.direct_filters.insert(key.clone());
        fc.filters.entry(key).or_default().push(predicate);
        Ok(())
    }

    fn apply_predicate_filter_keep(
        op: BoundBinaryOp,
        fc: &mut FilterContext,
        ctx: &ExecutionContext,
        outer_fc: &FilterContext,
        outer_rc: &RowContext,
    ) -> DaxResult<()> {
        let (table, column, predicate) = Self::resolve_predicate(op, ctx, outer_fc, outer_rc)?;
        fc.direct_filters.insert((table.clone(), column.clone()));
        fc.filters
            .entry((table, column))
            .or_default()
            .push(predicate);
        Ok(())
    }

    fn apply_keepfilters_table_override(
        ctx: &ExecutionContext,
        tname: &str,
        new_df: DataFrame,
        fc: &mut FilterContext,
        rc: &RowContext,
    ) -> DaxResult<()> {
        let existing_df = ctx.get_filtered_df(tname, fc, rc)?;
        let cols: Vec<String> = new_df
            .get_column_names()
            .iter()
            .map(|s| s.to_string())
            .collect();
        let intersected = existing_df
            .join(&new_df, &cols, &cols, JoinType::Semi.into(), None)
            .unwrap_or(existing_df);
        fc.table_overrides.insert(tname.to_string(), intersected);
        Ok(())
    }

    fn eval_to_literal(
        expr: BoundExprNode,
        ctx: &ExecutionContext,
        fc: &FilterContext,
        rc: &RowContext,
    ) -> DaxResult<LiteralValue> {
        let value = Self::eval(expr, ctx, fc, rc)?;
        match value {
            Value::Integer(i) => Ok(LiteralValue::Integer(i)),
            Value::Number(n) => {
                if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                    Ok(LiteralValue::Integer(n as i64))
                } else {
                    Ok(LiteralValue::Number(n))
                }
            }
            Value::String(s) => Ok(LiteralValue::String(s)),
            Value::Boolean(b) => Ok(LiteralValue::Boolean(b)),
            Value::DateTime(ms) => Ok(LiteralValue::DateTime(ms)),
            Value::Blank => Ok(LiteralValue::Blank),
            other => Err(DaxError::Type(format!(
                "CALCULATE filter RHS evaluated to non-scalar: {other:?}"
            ))),
        }
    }

    fn lit_to_series(lit: LiteralValue) -> Series {
        use polars::prelude::{DatetimeChunked, Int64Chunked, TimeUnit};
        match lit {
            LiteralValue::Integer(i) => Series::new("filter".into(), &[i]),
            LiteralValue::Number(n) => Series::new("filter".into(), &[n]),
            LiteralValue::String(s) => Series::new("filter".into(), &[s.as_str()]),
            LiteralValue::Boolean(b) => Series::new("filter".into(), &[b]),
            LiteralValue::DateTime(ms) => {
                let ca: DatetimeChunked = Int64Chunked::new("filter".into(), &[ms])
                    .into_datetime(TimeUnit::Milliseconds, None);
                ca.into_series()
            }
            LiteralValue::CrossFilterDirection(_) => {
                unreachable!("CrossFilterDirection literal is only valid inside CROSSFILTER()")
            }
            LiteralValue::Blank => Series::new_null("filter".into(), 1),
        }
    }

    fn eval_binary(lhs: Value, rhs: Value, op: BinaryOperator) -> DaxResult<Value> {
        match (lhs, rhs) {
            (Value::Integer(a), Value::Integer(b)) => match op {
                BinaryOperator::Add => Ok(Value::Integer(a + b)),
                BinaryOperator::Sub => Ok(Value::Integer(a - b)),
                BinaryOperator::Mul => Ok(Value::Integer(a * b)),
                BinaryOperator::Div => Ok(if b == 0 {
                    Value::Blank
                } else {
                    Value::Number(a as f64 / b as f64)
                }),
                BinaryOperator::Eq => Ok(Value::Boolean(a == b)),
                BinaryOperator::Neq => Ok(Value::Boolean(a != b)),
                BinaryOperator::Gt => Ok(Value::Boolean(a > b)),
                BinaryOperator::Lt => Ok(Value::Boolean(a < b)),
                BinaryOperator::Gte => Ok(Value::Boolean(a >= b)),
                BinaryOperator::Lte => Ok(Value::Boolean(a <= b)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for Integer operands"
                ))),
            },

            // Integer ↔ Number: promote integer to float
            (Value::Integer(a), Value::Number(b)) => {
                Self::eval_binary(Value::Number(a as f64), Value::Number(b), op)
            }
            (Value::Number(a), Value::Integer(b)) => {
                Self::eval_binary(Value::Number(a), Value::Number(b as f64), op)
            }

            (Value::Boolean(a), Value::Boolean(b)) => match op {
                BinaryOperator::And => Ok(Value::Boolean(a && b)),
                BinaryOperator::Or => Ok(Value::Boolean(a || b)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for Boolean operands"
                ))),
            },

            (Value::Number(a), Value::Number(b)) => match op {
                BinaryOperator::Add => Ok(Value::Number(a + b)),
                BinaryOperator::Sub => Ok(Value::Number(a - b)),
                BinaryOperator::Mul => Ok(Value::Number(a * b)),
                BinaryOperator::Div => Ok(if b == 0.0 {
                    Value::Blank
                } else {
                    Value::Number(a / b)
                }),
                BinaryOperator::Eq => Ok(Value::Boolean(a == b)),
                BinaryOperator::Neq => Ok(Value::Boolean(a != b)),
                BinaryOperator::Gt => Ok(Value::Boolean(a > b)),
                BinaryOperator::Lt => Ok(Value::Boolean(a < b)),
                BinaryOperator::Gte => Ok(Value::Boolean(a >= b)),
                BinaryOperator::Lte => Ok(Value::Boolean(a <= b)),
                BinaryOperator::And | BinaryOperator::Or => Err(DaxError::Type(
                    "&& / || require boolean operands, got Number".into(),
                )),
                BinaryOperator::Concat => Err(DaxError::Type(
                    "& requires string operands, got Number".into(),
                )),
                BinaryOperator::In | BinaryOperator::NotIn => Err(DaxError::Type(
                    "IN/NOT IN requires a table constructor on the right".into(),
                )),
            },

            (Value::Series(a), Value::Series(b)) => {
                let result = match op {
                    BinaryOperator::Add => {
                        (&a + &b).map_err(|e| DaxError::Eval(format!("Series +: {e}")))?
                    }
                    BinaryOperator::Sub => {
                        (&a - &b).map_err(|e| DaxError::Eval(format!("Series -: {e}")))?
                    }
                    BinaryOperator::Mul => {
                        (&a * &b).map_err(|e| DaxError::Eval(format!("Series *: {e}")))?
                    }
                    BinaryOperator::Div => {
                        (&a / &b).map_err(|e| DaxError::Eval(format!("Series /: {e}")))?
                    }
                    BinaryOperator::Eq => a
                        .equal(&b)
                        .map_err(|e| DaxError::Eval(format!("Series =: {e}")))?
                        .into_series(),
                    BinaryOperator::Neq => a
                        .not_equal(&b)
                        .map_err(|e| DaxError::Eval(format!("Series <>: {e}")))?
                        .into_series(),
                    BinaryOperator::Gt => a
                        .gt(&b)
                        .map_err(|e| DaxError::Eval(format!("Series >: {e}")))?
                        .into_series(),
                    BinaryOperator::Lt => a
                        .lt(&b)
                        .map_err(|e| DaxError::Eval(format!("Series <: {e}")))?
                        .into_series(),
                    BinaryOperator::Gte => a
                        .gt_eq(&b)
                        .map_err(|e| DaxError::Eval(format!("Series >=: {e}")))?
                        .into_series(),
                    BinaryOperator::Lte => a
                        .lt_eq(&b)
                        .map_err(|e| DaxError::Eval(format!("Series <=: {e}")))?
                        .into_series(),
                    BinaryOperator::And => {
                        let ca = a.bool().map_err(|_| {
                            DaxError::Type("&&: expected boolean series on left".into())
                        })?;
                        let cb = b.bool().map_err(|_| {
                            DaxError::Type("&&: expected boolean series on right".into())
                        })?;
                        (ca & cb).into_series()
                    }
                    BinaryOperator::Or => {
                        let ca = a.bool().map_err(|_| {
                            DaxError::Type("||: expected boolean series on left".into())
                        })?;
                        let cb = b.bool().map_err(|_| {
                            DaxError::Type("||: expected boolean series on right".into())
                        })?;
                        (ca | cb).into_series()
                    }
                    BinaryOperator::Concat => {
                        let sa = a.str().map_err(|_| {
                            DaxError::Type("&: expected string series on left".into())
                        })?;
                        let sb = b.str().map_err(|_| {
                            DaxError::Type("&: expected string series on right".into())
                        })?;
                        let result: polars::prelude::StringChunked = sa
                            .iter()
                            .zip(sb.iter())
                            .map(|(a, b)| match (a, b) {
                                (Some(a), Some(b)) => Some(format!("{}{}", a, b)),
                                _ => None,
                            })
                            .collect();
                        result.into_series()
                    }
                    BinaryOperator::In | BinaryOperator::NotIn => {
                        return Err(DaxError::Type(
                            "IN/NOT IN requires a table constructor on the right".into(),
                        ))
                    }
                };
                Ok(Value::Series(result))
            }

            (Value::Series(s), Value::Boolean(b)) => {
                let ca = s.bool().map_err(|_| {
                    DaxError::Type("Boolean comparison: expected boolean series".into())
                })?;
                let result: BooleanChunked = match op {
                    BinaryOperator::Eq => ca.no_null_iter().map(|v| v == b).collect(),
                    BinaryOperator::Neq => ca.no_null_iter().map(|v| v != b).collect(),
                    BinaryOperator::And => ca.no_null_iter().map(|v| v && b).collect(),
                    BinaryOperator::Or => ca.no_null_iter().map(|v| v || b).collect(),
                    other => {
                        return Err(DaxError::Type(format!(
                            "Operator {other:?} not supported for (Series<bool>, Boolean)"
                        )))
                    }
                };
                Ok(Value::Series(result.into_series()))
            }
            (Value::Boolean(b), Value::Series(s)) => {
                let ca = s.bool().map_err(|_| {
                    DaxError::Type("Boolean comparison: expected boolean series".into())
                })?;
                let result: BooleanChunked = match op {
                    BinaryOperator::Eq => ca.no_null_iter().map(|v| v == b).collect(),
                    BinaryOperator::Neq => ca.no_null_iter().map(|v| v != b).collect(),
                    BinaryOperator::And => ca.no_null_iter().map(|v| b && v).collect(),
                    BinaryOperator::Or => ca.no_null_iter().map(|v| b || v).collect(),
                    other => {
                        return Err(DaxError::Type(format!(
                            "Operator {other:?} not supported for (Boolean, Series<bool>)"
                        )))
                    }
                };
                Ok(Value::Series(result.into_series()))
            }

            (Value::Series(s), Value::Number(n)) => Self::series_scalar_cmp(&s, n, op),
            (Value::Number(n), Value::Series(s)) => Self::series_scalar_cmp(&s, n, op.flip()),
            (Value::Series(s), Value::Integer(i)) => Self::series_scalar_cmp(&s, i as f64, op),
            (Value::Integer(i), Value::Series(s)) => {
                Self::series_scalar_cmp(&s, i as f64, op.flip())
            }

            (Value::Series(s), Value::String(str_val)) => Self::series_string_cmp(&s, &str_val, op),
            (Value::String(str_val), Value::Series(s)) => Self::series_string_cmp(&s, &str_val, op),

            (Value::String(a), Value::String(b)) => match op {
                BinaryOperator::Eq => Ok(Value::Boolean(a == b)),
                BinaryOperator::Neq => Ok(Value::Boolean(a != b)),
                BinaryOperator::Concat => Ok(Value::String(a + &b)),
                other => Err(DaxError::Type(format!(
                    "Only Eq/Neq/Concat supported for string scalars, got {other:?}"
                ))),
            },

            (Value::DateTime(a), Value::DateTime(b)) => match op {
                BinaryOperator::Eq => Ok(Value::Boolean(a == b)),
                BinaryOperator::Neq => Ok(Value::Boolean(a != b)),
                BinaryOperator::Gt => Ok(Value::Boolean(a > b)),
                BinaryOperator::Lt => Ok(Value::Boolean(a < b)),
                BinaryOperator::Gte => Ok(Value::Boolean(a >= b)),
                BinaryOperator::Lte => Ok(Value::Boolean(a <= b)),
                BinaryOperator::Sub => Ok(Value::Number(((a - b) / 86_400_000) as f64)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for (DateTime, DateTime)"
                ))),
            },

            (Value::DateTime(dt), Value::Number(n)) => match op {
                BinaryOperator::Add => Ok(Value::DateTime(dt + (n as i64) * 86_400_000)),
                BinaryOperator::Sub => Ok(Value::DateTime(dt - (n as i64) * 86_400_000)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for (DateTime, Number)"
                ))),
            },
            (Value::Number(n), Value::DateTime(dt)) => match op {
                BinaryOperator::Add => Ok(Value::DateTime(dt + (n as i64) * 86_400_000)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for (Number, DateTime)"
                ))),
            },
            (Value::DateTime(dt), Value::Integer(n)) => match op {
                BinaryOperator::Add => Ok(Value::DateTime(dt + n * 86_400_000)),
                BinaryOperator::Sub => Ok(Value::DateTime(dt - n * 86_400_000)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for (DateTime, Integer)"
                ))),
            },
            (Value::Integer(n), Value::DateTime(dt)) => match op {
                BinaryOperator::Add => Ok(Value::DateTime(dt + n * 86_400_000)),
                other => Err(DaxError::Type(format!(
                    "Operator {other:?} not supported for (Integer, DateTime)"
                ))),
            },

            (Value::Series(s), Value::DateTime(ms)) => Self::series_datetime_cmp(&s, ms, op),
            (Value::DateTime(ms), Value::Series(s)) => Self::series_datetime_cmp(&s, ms, op.flip()),

            (lhs, Value::Table(_, df))
                if matches!(op, BinaryOperator::In | BinaryOperator::NotIn) =>
            {
                let negate = matches!(op, BinaryOperator::NotIn);
                let col = df.columns().first().ok_or_else(|| {
                    DaxError::Eval("IN: table constructor produced no columns".into())
                })?;
                let col_series = col.as_materialized_series();
                match lhs {
                    Value::Series(s) => {
                        let values_cast = col_series
                            .cast(s.dtype())
                            .map_err(|e| DaxError::Eval(format!("IN: {e}")))?;
                        let mask =
                            crate::engine::context::membership_mask(&s, &values_cast, !negate)?;
                        Ok(Value::Series(mask.into_series()))
                    }
                    scalar => {
                        let scalar_series = match &scalar {
                            Value::Integer(i) => Series::new("v".into(), &[*i]),
                            Value::Number(n) => Series::new("v".into(), &[*n]),
                            Value::String(s) => Series::new("v".into(), &[s.as_str()]),
                            Value::Boolean(b) => Series::new("v".into(), &[*b]),
                            Value::Blank => return Ok(Value::Boolean(false)),
                            other => {
                                return Err(DaxError::Type(format!(
                                    "IN: unsupported scalar type {other:?}"
                                )))
                            }
                        };
                        let values_cast = col_series
                            .cast(scalar_series.dtype())
                            .map_err(|e| DaxError::Eval(format!("IN: {e}")))?;
                        let mask = crate::engine::context::membership_mask(
                            &scalar_series,
                            &values_cast,
                            !negate,
                        )?;
                        let found = mask.get(0).unwrap_or(false);
                        Ok(Value::Boolean(found))
                    }
                }
            }

            (Value::Blank, Value::Integer(b)) => {
                Self::eval_binary(Value::Integer(0), Value::Integer(b), op)
            }
            (Value::Blank, Value::Number(b)) => {
                Self::eval_binary(Value::Number(0.0), Value::Number(b), op)
            }
            (Value::Integer(a), Value::Blank) => {
                Self::eval_binary(Value::Integer(a), Value::Integer(0), op)
            }
            (Value::Number(a), Value::Blank) => {
                Self::eval_binary(Value::Number(a), Value::Number(0.0), op)
            }
            (Value::Blank, Value::String(b)) => {
                Self::eval_binary(Value::String(String::new()), Value::String(b), op)
            }
            (Value::String(a), Value::Blank) => {
                Self::eval_binary(Value::String(a), Value::String(String::new()), op)
            }
            (Value::Blank, Value::Boolean(b)) => {
                Self::eval_binary(Value::Boolean(false), Value::Boolean(b), op)
            }
            (Value::Boolean(a), Value::Blank) => {
                Self::eval_binary(Value::Boolean(a), Value::Boolean(false), op)
            }
            (Value::Blank, Value::Blank) => match op {
                BinaryOperator::Eq | BinaryOperator::Gte | BinaryOperator::Lte => {
                    Ok(Value::Boolean(true))
                }
                BinaryOperator::Neq | BinaryOperator::Gt | BinaryOperator::Lt => {
                    Ok(Value::Boolean(false))
                }
                _ => Ok(Value::Blank),
            },

            (Value::Series(s), Value::Blank) => match op {
                BinaryOperator::Eq => Ok(Value::Series(s.is_null().into_series())),
                BinaryOperator::Neq => Ok(Value::Series(s.is_not_null().into_series())),
                _ => Ok(Value::Blank),
            },
            (Value::Blank, Value::Series(s)) => match op {
                BinaryOperator::Eq => Ok(Value::Series(s.is_null().into_series())),
                BinaryOperator::Neq => Ok(Value::Series(s.is_not_null().into_series())),
                _ => Ok(Value::Blank),
            },

            (lhs, rhs) => Err(DaxError::Type(format!(
                "Type mismatch in binary operation: {lhs:?} {op:?} {rhs:?}"
            ))),
        }
    }

    fn series_scalar_cmp(s: &Series, scalar: f64, op: BinaryOperator) -> DaxResult<Value> {
        let bool_ca = match s.dtype() {
            DataType::Float64 => apply_cmp(
                s.f64()
                    .expect("dtype matched as Float64")
                    .into_no_null_iter(),
                scalar,
                op,
            )?,
            DataType::Int64 => apply_cmp(
                s.i64().expect("dtype matched as Int64").into_no_null_iter(),
                scalar as i64,
                op,
            )?,
            DataType::Int32 => apply_cmp(
                s.i32().expect("dtype matched as Int32").into_no_null_iter(),
                scalar as i32,
                op,
            )?,
            DataType::Int8 => apply_cmp(
                s.i8().expect("dtype matched as Int8").into_no_null_iter(),
                scalar as i8,
                op,
            )?,
            other => {
                return Err(DaxError::Type(format!(
                    "Unsupported dtype in series_scalar_cmp: {other:?}"
                )))
            }
        };
        Ok(Value::Series(bool_ca.into_series()))
    }

    fn series_string_cmp(s: &Series, scalar: &str, op: BinaryOperator) -> DaxResult<Value> {
        let ca = s.str().map_err(|_| {
            DaxError::Type(format!(
                "series_string_cmp: expected String series, got {:?}",
                s.dtype()
            ))
        })?;
        Ok(Value::Series(
            apply_cmp(ca.no_null_iter(), scalar, op)?.into_series(),
        ))
    }

    fn series_datetime_cmp(s: &Series, scalar: i64, op: BinaryOperator) -> DaxResult<Value> {
        let ca = s.datetime().map_err(|_| {
            DaxError::Type(format!(
                "series_datetime_cmp: expected Datetime series, got {:?}",
                s.dtype()
            ))
        })?;
        Ok(Value::Series(
            apply_cmp(ca.phys.into_no_null_iter(), scalar, op)?.into_series(),
        ))
    }
}

fn apply_cmp<T: PartialOrd + Copy>(
    iter: impl Iterator<Item = T>,
    scalar: T,
    op: BinaryOperator,
) -> DaxResult<BooleanChunked> {
    use crate::engine::error::DaxError;
    let cmp: fn(T, T) -> bool = match op {
        BinaryOperator::Gt => |a, b| a > b,
        BinaryOperator::Lt => |a, b| a < b,
        BinaryOperator::Gte => |a, b| a >= b,
        BinaryOperator::Lte => |a, b| a <= b,
        BinaryOperator::Eq => |a, b| a == b,
        BinaryOperator::Neq => |a, b| a != b,
        other => {
            return Err(DaxError::Type(format!(
                "Non-comparison operator {other:?} in series_scalar_cmp"
            )))
        }
    };
    Ok(iter.map(|v| cmp(v, scalar)).collect())
}
