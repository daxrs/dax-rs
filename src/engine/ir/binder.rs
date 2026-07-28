use polars::prelude::DataType;
use std::collections::HashSet;

use crate::engine::context::ExecutionContext;
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::functions::REGISTRY;
use crate::engine::ir::expr_node::*;
use crate::engine::ir::operator::{
    infer_binary_dtype, BoundBinaryOp, BoundCalculate, BoundColumn, BoundFunction, BoundLiteral,
    BoundMeasure, BoundSummarize, BoundSummarizeColumns, BoundTable, BoundUnaryOp, BoundVar,
    CalculateNode, ColumnRef, FunctionCall, UnaryOperator,
};

pub fn bind(expr: ExprNode, ctx: &ExecutionContext) -> DaxResult<BoundExprNode> {
    bind_inner(expr, ctx, &HashSet::new())
}

/// Bind an expression with a set of names already in scope (query-level
/// `DEFINE VAR`/`DEFINE MEASURE` names). References to these names bind to
/// `VarRef`, resolved at eval time via row-context lookup — see
/// `RowContext::with_var`. The names themselves carry no bound tree here;
/// each is evaluated exactly once by the caller (see `Engine::evaluate_query`).
pub fn bind_with_vars(
    expr: ExprNode,
    ctx: &ExecutionContext,
    vars: &HashSet<String>,
) -> DaxResult<BoundExprNode> {
    bind_inner(expr, ctx, vars)
}

fn bind_inner(
    expr: ExprNode,
    ctx: &ExecutionContext,
    vars: &HashSet<String>,
) -> DaxResult<BoundExprNode> {
    match expr {
        ExprNode::Literal(l) => {
            let dtype = l.dtype();
            Ok(BoundExprNode::Literal(BoundLiteral { value: l, dtype }))
        }

        ExprNode::Identifier(name) => bind_name(name, ctx, vars),
        ExprNode::MeasureRef(name) => {
            if vars.contains(&name) {
                return Ok(BoundExprNode::VarRef(name));
            }
            bind_measure_ref(name)
        }

        ExprNode::Column(col) => bind_column(col, ctx),

        ExprNode::Function(func) => bind_function(func, ctx, vars),

        ExprNode::UnaryOp(op) => {
            let expr = Box::new(bind_inner(*op.expr, ctx, vars)?);
            let dtype = match &op.op {
                UnaryOperator::Not => Some(DataType::Boolean),
                UnaryOperator::Negate => expr.dtype(),
            };
            Ok(BoundExprNode::UnaryOp(BoundUnaryOp {
                op: op.op,
                expr,
                dtype,
            }))
        }

        ExprNode::BinaryOp(op) => {
            let left = Box::new(bind_inner(*op.left, ctx, vars)?);
            let right = Box::new(bind_inner(*op.right, ctx, vars)?);
            let dtype = match (left.dtype(), right.dtype()) {
                (Some(l), Some(r)) => Some(infer_binary_dtype(&op.op, &l, &r)),
                _ => None,
            };
            Ok(BoundExprNode::BinaryOp(BoundBinaryOp {
                left,
                right,
                op: op.op,
                dtype,
            }))
        }

        ExprNode::Calculate(calc) => bind_calculate(calc, ctx, vars),

        ExprNode::Summarize(s) => {
            let table = Box::new(bind_inner(*s.table, ctx, vars)?);
            let group_by = s
                .group_by
                .into_iter()
                .map(|e| bind_inner(e, ctx, vars))
                .collect::<DaxResult<_>>()?;
            let rollup_cols = s
                .rollup_cols
                .into_iter()
                .map(|(e, flag)| Ok((bind_inner(e, ctx, vars)?, flag)))
                .collect::<DaxResult<_>>()?;
            let extensions = s
                .extensions
                .into_iter()
                .map(|(name, expr)| Ok((name, bind_inner(expr, ctx, vars)?)))
                .collect::<DaxResult<_>>()?;
            Ok(BoundExprNode::Summarize(BoundSummarize {
                table,
                group_by,
                rollup_cols,
                extensions,
            }))
        }

        ExprNode::SummarizeColumns(sc) => {
            let group_by_cols = sc
                .group_by_cols
                .into_iter()
                .map(|e| bind_inner(e, ctx, vars))
                .collect::<DaxResult<_>>()?;
            let rollup_groups = sc
                .rollup_groups
                .into_iter()
                .map(|axis| {
                    axis.into_iter()
                        .map(|(cols, flag)| {
                            Ok((
                                cols.into_iter()
                                    .map(|e| bind_inner(e, ctx, vars))
                                    .collect::<DaxResult<_>>()?,
                                flag,
                            ))
                        })
                        .collect::<DaxResult<_>>()
                })
                .collect::<DaxResult<_>>()?;
            let filters = sc
                .filters
                .into_iter()
                .map(|e| bind_inner(e, ctx, vars))
                .collect::<DaxResult<_>>()?;
            let extensions = sc
                .extensions
                .into_iter()
                .map(|(name, expr, ig)| Ok((name, bind_inner(expr, ctx, vars)?, ig)))
                .collect::<DaxResult<_>>()?;
            Ok(BoundExprNode::SummarizeColumns(BoundSummarizeColumns {
                group_by_cols,
                rollup_groups,
                filters,
                extensions,
            }))
        }

        ExprNode::TableConstructor(rows) => {
            let bound = rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|e| bind_inner(e, ctx, vars))
                        .collect::<DaxResult<_>>()
                })
                .collect::<DaxResult<_>>()?;
            Ok(BoundExprNode::TableConstructor(bound))
        }

        // VAR/RETURN: bind each variable sequentially, extending the scope.
        // Each binding's bound tree is kept in the Var node itself (evaluated
        // once, at eval time — see Evaluator::eval) instead of being cloned
        // into every reference site.
        ExprNode::Var(v) => {
            let mut scope = vars.clone();
            let mut bindings: Vec<(String, BoundExprNode)> = Vec::with_capacity(v.bindings.len());
            for (name, expr) in v.bindings {
                let bound = bind_inner(expr, ctx, &scope)?;
                scope.insert(name.clone());
                bindings.push((name, bound));
            }
            let result = Box::new(bind_inner(*v.result, ctx, &scope)?);
            Ok(BoundExprNode::Var(BoundVar { bindings, result }))
        }
    }
}

fn bind_name(
    name: String,
    ctx: &ExecutionContext,
    vars: &HashSet<String>,
) -> DaxResult<BoundExprNode> {
    if vars.contains(&name) {
        return Ok(BoundExprNode::VarRef(name));
    }

    if ctx.tables.contains_key(&name) {
        return Ok(BoundExprNode::Table(BoundTable { name }));
    }

    Err(DaxError::UnknownName(format!(
        "Unknown identifier '{}' — measure references require bracket notation [{}]",
        name, name
    )))
}

fn bind_measure_ref(name: String) -> DaxResult<BoundExprNode> {
    Ok(BoundExprNode::Measure(BoundMeasure { name }))
}

fn bind_function(
    func: FunctionCall,
    ctx: &ExecutionContext,
    vars: &HashSet<String>,
) -> DaxResult<BoundExprNode> {
    let args: Vec<BoundExprNode> = func
        .args
        .into_iter()
        .map(|arg| bind_inner(arg, ctx, vars))
        .collect::<DaxResult<_>>()?;

    let arg_dtypes: Vec<Option<DataType>> = args.iter().map(|a| a.dtype()).collect();
    let entry = REGISTRY
        .get(&func.name)
        .ok_or_else(|| DaxError::UnknownName(format!("Unknown function: '{}'", func.name)))?;
    let dtype = entry.return_type().to_dtype(&arg_dtypes);

    Ok(BoundExprNode::Function(BoundFunction {
        name: func.name,
        args,
        dtype,
    }))
}

fn bind_column(col: ColumnRef, ctx: &ExecutionContext) -> DaxResult<BoundExprNode> {
    let dtype = match ctx
        .catalog
        .columns
        .get(&(col.table.clone(), col.column.clone()))
    {
        Some(meta) => meta.dtype.clone(),
        None if ctx.resolved_measures.contains_key(&col.column) => {
            return Ok(BoundExprNode::Measure(BoundMeasure { name: col.column }));
        }
        None if ctx.tables.contains_key(&col.table) => {
            return Err(DaxError::InvalidArgument(format!(
                "Column '{}' not found in table '{}'; extension columns must be referenced as [{}]",
                col.column, col.table, col.column
            )));
        }
        None => {
            return Err(DaxError::UnknownName(format!(
                "Unknown column: {}.{}",
                col.table, col.column
            )))
        }
    };

    Ok(BoundExprNode::Column(BoundColumn {
        table: col.table,
        column: col.column,
        dtype,
    }))
}

fn bind_calculate(
    calc: CalculateNode,
    ctx: &ExecutionContext,
    vars: &HashSet<String>,
) -> DaxResult<BoundExprNode> {
    let expression = Box::new(bind_inner(*calc.expression, ctx, vars)?);
    let dtype = expression.dtype();
    let filters = calc
        .filters
        .into_iter()
        .map(|f| bind_inner(f, ctx, vars))
        .collect::<DaxResult<_>>()?;
    Ok(BoundExprNode::Calculate(BoundCalculate {
        expression,
        filters,
        dtype,
    }))
}
