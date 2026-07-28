use crate::engine::dax::ast::{DaxExpr, Literal};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::ir::expr_node::*;
use crate::engine::ir::operator::{self, *};

pub fn build_expression(ast: DaxExpr) -> DaxResult<ExprNode> {
    match ast {
        DaxExpr::Literal(lit) => Ok(ExprNode::Literal(match lit {
            crate::engine::dax::ast::Literal::Number(n) => {
                if n.fract() == 0.0 && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                    LiteralValue::Integer(n as i64)
                } else {
                    LiteralValue::Number(n)
                }
            }
            crate::engine::dax::ast::Literal::String(s) => LiteralValue::String(s),
            crate::engine::dax::ast::Literal::DateTime(ms) => LiteralValue::DateTime(ms),
        })),

        DaxExpr::ColumnRef { table, column } => Ok(ExprNode::Column(ColumnRef { table, column })),

        DaxExpr::Identifier(name) => {
            if name.eq_ignore_ascii_case("TRUE") {
                return Ok(ExprNode::Literal(LiteralValue::Boolean(true)));
            }
            if name.eq_ignore_ascii_case("FALSE") {
                return Ok(ExprNode::Literal(LiteralValue::Boolean(false)));
            }
            if name.eq_ignore_ascii_case("ASC") {
                return Ok(ExprNode::Literal(LiteralValue::Boolean(true)));
            }
            if name.eq_ignore_ascii_case("DESC") {
                return Ok(ExprNode::Literal(LiteralValue::Boolean(false)));
            }
            if name.eq_ignore_ascii_case("NONE") {
                return Ok(ExprNode::Literal(LiteralValue::CrossFilterDirection(
                    operator::CrossFilterDirection::None,
                )));
            }
            if name.eq_ignore_ascii_case("ONEWAY") {
                return Ok(ExprNode::Literal(LiteralValue::CrossFilterDirection(
                    operator::CrossFilterDirection::OneWay,
                )));
            }
            if name.eq_ignore_ascii_case("BOTH") {
                return Ok(ExprNode::Literal(LiteralValue::CrossFilterDirection(
                    operator::CrossFilterDirection::Both,
                )));
            }
            Ok(ExprNode::Identifier(name))
        }

        DaxExpr::BinaryOp { lhs, rhs, op } => Ok(ExprNode::BinaryOp(BinaryOpNode {
            left: Box::new(build_expression(*lhs)?),
            right: Box::new(build_expression(*rhs)?),
            op: map_binary_op(op)?,
        })),

        DaxExpr::UnaryOp { op, expr } => {
            let operator = match op.as_str() {
                "-" => crate::engine::ir::operator::UnaryOperator::Negate,
                "NOT" => crate::engine::ir::operator::UnaryOperator::Not,
                other => {
                    return Err(DaxError::InvalidArgument(format!(
                        "Unknown unary operator: {other}"
                    )))
                }
            };
            Ok(ExprNode::UnaryOp(
                crate::engine::ir::operator::UnaryOpNode {
                    op: operator,
                    expr: Box::new(build_expression(*expr)?),
                },
            ))
        }

        DaxExpr::MeasureRef(name) => Ok(ExprNode::MeasureRef(name)),

        DaxExpr::VarExpr { bindings, result } => {
            let bindings = bindings
                .into_iter()
                .map(|(name, expr)| Ok((name, build_expression(*expr)?)))
                .collect::<DaxResult<_>>()?;
            let result = Box::new(build_expression(*result)?);
            Ok(ExprNode::Var(VarNode { bindings, result }))
        }

        DaxExpr::TableConstructor(rows) => {
            let built = rows
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(build_expression)
                        .collect::<DaxResult<_>>()
                })
                .collect::<DaxResult<_>>()?;
            Ok(ExprNode::TableConstructor(built))
        }

        DaxExpr::FunctionCall { name, args } => {
            if name.eq_ignore_ascii_case("CALCULATE") || name.eq_ignore_ascii_case("CALCULATETABLE")
            {
                return build_calculate(args);
            }
            if name.eq_ignore_ascii_case("SUMMARIZE") {
                return build_summarize(args);
            }
            if name.eq_ignore_ascii_case("SUMMARIZECOLUMNS") {
                return build_summarize_columns(args);
            }

            Ok(ExprNode::Function(FunctionCall {
                name,
                args: args
                    .into_iter()
                    .map(build_expression)
                    .collect::<DaxResult<_>>()?,
            }))
        }
    }
}

fn build_summarize(args: Vec<DaxExpr>) -> DaxResult<ExprNode> {
    if args.len() < 2 {
        return Err(DaxError::InvalidArgument(
            "SUMMARIZE requires at least 2 arguments".into(),
        ));
    }
    let mut it = args.into_iter();
    let table = Box::new(build_expression(it.next().expect("just checked len >= 2"))?);

    let mut group_by = Vec::new();
    let mut rollup_cols = Vec::new();
    let mut extensions = Vec::new();
    let mut remaining: Vec<DaxExpr> = it.collect();

    while !remaining.is_empty() {
        match &remaining[0] {
            DaxExpr::Literal(Literal::String(_)) => break,
            DaxExpr::FunctionCall { name, .. } if name.eq_ignore_ascii_case("ROLLUP") => {
                if let DaxExpr::FunctionCall { args: rollup_args, .. } = remaining.remove(0) {
                    for arg in rollup_args {
                        let built = build_expression(arg)?;
                        rollup_cols.push((built.clone(), None));
                        group_by.push(built);
                    }
                }
            }
            DaxExpr::FunctionCall { name, .. }
                if name.eq_ignore_ascii_case("ROLLUPADDISSUBTOTAL") =>
            {
                return Err(DaxError::InvalidArgument(
                    "ROLLUPADDISSUBTOTAL is only valid inside SUMMARIZECOLUMNS, not SUMMARIZE"
                        .into(),
                ));
            }
            _ => group_by.push(build_expression(remaining.remove(0))?),
        }
    }

    while remaining.len() >= 2 {
        let name = match remaining.remove(0) {
            DaxExpr::Literal(Literal::String(s)) => s,
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "SUMMARIZE: expected extension column name (string literal), got {other:?}"
                )))
            }
        };
        let expr = build_expression(remaining.remove(0))?;
        extensions.push((name, expr));
    }
    if !remaining.is_empty() {
        return Err(DaxError::InvalidArgument(
            "SUMMARIZE: odd number of extension arguments".into(),
        ));
    }

    Ok(ExprNode::Summarize(SummarizeNode {
        table,
        group_by,
        rollup_cols,
        extensions,
    }))
}

fn build_summarize_columns(args: Vec<DaxExpr>) -> DaxResult<ExprNode> {
    let mut group_by_cols = Vec::new();
    let mut rollup_groups = Vec::new();
    let mut filters = Vec::new();
    let mut extensions = Vec::new();
    let mut remaining: Vec<DaxExpr> = args;

    while !remaining.is_empty() {
        match &remaining[0] {
            DaxExpr::Literal(Literal::String(_)) => {
                if remaining.len() < 2 {
                    return Err(DaxError::InvalidArgument(
                        "SUMMARIZECOLUMNS: extension name has no expression".into(),
                    ));
                }
                let name = match remaining.remove(0) {
                    DaxExpr::Literal(Literal::String(s)) => s,
                    _ => unreachable!(),
                };
                let raw = remaining.remove(0);
                let (is_ignore, expr) = match raw {
                    DaxExpr::FunctionCall { ref name, .. }
                        if name.eq_ignore_ascii_case("IGNORE") =>
                    {
                        if let DaxExpr::FunctionCall { args, .. } = raw {
                            if args.len() != 1 {
                                return Err(DaxError::InvalidArgument(
                                    "IGNORE requires exactly one argument".into(),
                                ));
                            }
                            (
                                true,
                                build_expression(
                                    args.into_iter().next().expect("just checked len == 1"),
                                )?,
                            )
                        } else {
                            unreachable!()
                        }
                    }
                    other => (false, build_expression(other)?),
                };
                extensions.push((name, expr, is_ignore));
            }
            DaxExpr::FunctionCall { name, .. }
                if name.eq_ignore_ascii_case("ROLLUPADDISSUBTOTAL") =>
            {
                if let DaxExpr::FunctionCall { args: rais_args, .. } = remaining.remove(0) {
                    rollup_groups.push(parse_rollupaddissubtotal(rais_args)?);
                }
            }
            DaxExpr::ColumnRef { .. } => {
                group_by_cols.push(build_expression(remaining.remove(0))?);
            }
            _ => {
                filters.push(build_expression(remaining.remove(0))?);
            }
        }
    }

    Ok(ExprNode::SummarizeColumns(SummarizeColumnsNode {
        group_by_cols,
        rollup_groups,
        filters,
        extensions,
    }))
}

/// Parse the argument list of ROLLUPADDISSUBTOTAL into rollup group entries.
/// Each pair is (col_or_ROLLUPGROUP, "flag_name").
/// ROLLUPGROUP(c1, c2) expands to a Vec of columns treated as one rollup unit.
fn parse_rollupaddissubtotal(
    mut args: Vec<DaxExpr>,
) -> DaxResult<Vec<(Vec<ExprNode>, Option<String>)>> {
    let mut groups = Vec::new();
    while args.len() >= 2 {
        let col_arg = args.remove(0);
        let flag_name = match args.remove(0) {
            DaxExpr::Literal(Literal::String(s)) => s,
            other => {
                return Err(DaxError::InvalidArgument(format!(
                    "ROLLUPADDISSUBTOTAL: expected string flag name, got {other:?}"
                )))
            }
        };
        let cols = match col_arg {
            DaxExpr::FunctionCall { ref name, .. } if name.eq_ignore_ascii_case("ROLLUPGROUP") => {
                if let DaxExpr::FunctionCall { args: rg_args, .. } = col_arg {
                    if rg_args.is_empty() {
                        return Err(DaxError::InvalidArgument(
                            "ROLLUPGROUP requires at least one column".into(),
                        ));
                    }
                    rg_args
                        .into_iter()
                        .map(build_expression)
                        .collect::<DaxResult<_>>()?
                } else {
                    unreachable!()
                }
            }
            other => vec![build_expression(other)?],
        };
        groups.push((cols, Some(flag_name)));
    }
    if !args.is_empty() {
        return Err(DaxError::InvalidArgument(
            "ROLLUPADDISSUBTOTAL: odd number of arguments".into(),
        ));
    }
    Ok(groups)
}

fn build_calculate(args: Vec<DaxExpr>) -> DaxResult<ExprNode> {
    if args.is_empty() {
        return Err(DaxError::InvalidArgument(
            "CALCULATE requires at least one argument".into(),
        ));
    }

    let expression = Box::new(build_expression(args[0].clone())?);
    let mut filters = Vec::new();

    for arg in args.into_iter().skip(1) {
        filters.push(build_expression(arg)?);
    }

    Ok(ExprNode::Calculate(CalculateNode { expression, filters }))
}

fn map_binary_op(op: String) -> DaxResult<BinaryOperator> {
    match op.as_str() {
        "+" => Ok(BinaryOperator::Add),
        "-" => Ok(BinaryOperator::Sub),
        "*" => Ok(BinaryOperator::Mul),
        "/" => Ok(BinaryOperator::Div),
        "=" => Ok(BinaryOperator::Eq),
        "!=" | "<>" => Ok(BinaryOperator::Neq),
        ">" => Ok(BinaryOperator::Gt),
        "<" => Ok(BinaryOperator::Lt),
        ">=" => Ok(BinaryOperator::Gte),
        "<=" => Ok(BinaryOperator::Lte),
        "&&" => Ok(BinaryOperator::And),
        "||" => Ok(BinaryOperator::Or),
        "&" => Ok(BinaryOperator::Concat),
        "IN" => Ok(BinaryOperator::In),
        "NOT IN" => Err(DaxError::InvalidArgument(
            "NOT IN is not supported; use NOT(x IN {...}) instead".into(),
        )),
        _ => Err(DaxError::InvalidArgument(format!("Unknown operator: {op}"))),
    }
}
