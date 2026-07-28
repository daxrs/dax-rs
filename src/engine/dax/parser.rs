use super::ast::{DaxExpr, DaxQuery, Definition, EvaluateStatement, Literal, SortDir};
use crate::engine::error::{DaxError, DaxResult};
use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar = "engine/dax/grammar.pest"]
pub struct DaxParser;

pub fn parse_query(input: &str) -> DaxResult<DaxQuery> {
    let mut pairs =
        DaxParser::parse(Rule::query, input).map_err(|e| DaxError::Parse(e.to_string()))?;
    let query_pair = pairs.next().expect("Expected query rule");
    let mut define = Vec::new();
    let mut statements = Vec::new();
    for pair in query_pair.into_inner() {
        match pair.as_rule() {
            Rule::define_block => define = parse_define_block(pair)?,
            Rule::evaluate_statement => statements.push(parse_evaluate_statement(pair)?),
            Rule::EOI => {}
            _ => unreachable!("Unexpected rule in query: {:?}", pair.as_rule()),
        }
    }
    Ok(DaxQuery { define, statements })
}

fn parse_define_block(pair: Pair<Rule>) -> DaxResult<Vec<Definition>> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::definition)
        .map(|p| {
            let inner = p.into_inner().next().expect("definition: missing child");
            parse_definition(inner)
        })
        .collect()
}

fn parse_definition(pair: Pair<Rule>) -> DaxResult<Definition> {
    match pair.as_rule() {
        Rule::var_definition => {
            let mut inner = pair.into_inner();
            let name = inner
                .next()
                .expect("var_definition: missing name")
                .as_str()
                .to_string();
            let expr = build_ast(inner.next().expect("var_definition: missing expression"))?;
            Ok(Definition::Var { name, expr: Box::new(expr) })
        }
        Rule::measure_definition => {
            let mut inner = pair.into_inner();
            let table = inner
                .next()
                .expect("measure_definition: missing table")
                .as_str()
                .to_string();
            let name = inner
                .next()
                .expect("measure_definition: missing name")
                .as_str()
                .to_string();
            let expr = build_ast(
                inner
                    .next()
                    .expect("measure_definition: missing expression"),
            )?;
            Ok(Definition::Measure { table, name, expr: Box::new(expr) })
        }
        _ => unreachable!("Unexpected definition rule: {:?}", pair.as_rule()),
    }
}

fn parse_evaluate_statement(pair: Pair<Rule>) -> DaxResult<EvaluateStatement> {
    let mut expr = None;
    let mut order_by = Vec::new();
    let mut start_at = Vec::new();
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::expr => expr = Some(Box::new(build_ast(child)?)),
            Rule::order_by_clause => order_by = parse_order_by_clause(child)?,
            Rule::start_at_clause => start_at = parse_start_at_clause(child)?,
            _ => unreachable!(
                "Unexpected child in evaluate_statement: {:?}",
                child.as_rule()
            ),
        }
    }
    Ok(EvaluateStatement {
        expr: expr.expect("EVALUATE statement missing expression"),
        order_by,
        start_at,
    })
}

fn parse_start_at_clause(pair: Pair<Rule>) -> DaxResult<Vec<DaxExpr>> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::start_at_value)
        .map(|p| {
            let inner = p
                .into_inner()
                .next()
                .expect("start_at_value: missing literal");
            build_ast(inner)
        })
        .collect()
}

fn parse_order_by_clause(pair: Pair<Rule>) -> DaxResult<Vec<(DaxExpr, SortDir)>> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::order_item)
        .map(parse_order_item)
        .collect()
}

fn parse_order_item(pair: Pair<Rule>) -> DaxResult<(DaxExpr, SortDir)> {
    let mut expr = None;
    let mut dir = SortDir::Asc;
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::expr => expr = Some(build_ast(child)?),
            Rule::sort_dir => {
                dir = if child.as_str().eq_ignore_ascii_case("desc") {
                    SortDir::Desc
                } else {
                    SortDir::Asc
                };
            }
            _ => unreachable!("Unexpected child in order_item: {:?}", child.as_rule()),
        }
    }
    Ok((expr.expect("ORDER BY item missing expression"), dir))
}

pub fn parse_expression(input: &str) -> DaxResult<DaxExpr> {
    let mut pairs =
        DaxParser::parse(Rule::program, input).map_err(|e| DaxError::Parse(e.to_string()))?;
    let program_pair = pairs.next().expect("Expected program rule");
    let expr_pair = program_pair
        .into_inner()
        .next()
        .expect("Expected expression inside program");
    build_ast(expr_pair)
}

fn build_ast(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    match pair.as_rule() {
        Rule::expr
        | Rule::logical_or
        | Rule::logical_and
        | Rule::comparison
        | Rule::concatenation
        | Rule::additive
        | Rule::multiplicative => parse_binary(pair),
        Rule::unary => parse_unary(pair),
        Rule::primary => {
            let inner = pair
                .into_inner()
                .next()
                .expect("Primary should have one child");
            build_ast(inner)
        }
        Rule::literal => Ok(parse_literal(pair)),
        Rule::column_ref => parse_column_ref(pair),
        Rule::identifier => Ok(DaxExpr::Identifier(pair.as_str().to_string())),
        Rule::function_call => parse_function_call(pair),
        Rule::var_expr => parse_var_expr(pair),
        Rule::measure_ref => {
            let inner = pair
                .into_inner()
                .next()
                .expect("measure_ref: missing dax_name");
            Ok(DaxExpr::MeasureRef(inner.as_str().trim().to_string()))
        }
        Rule::quoted_name => {
            let raw = pair.as_str();
            Ok(DaxExpr::Identifier(raw[1..raw.len() - 1].to_string()))
        }
        Rule::table_constructor => {
            let rows = pair
                .into_inner()
                .filter(|p| p.as_rule() == Rule::table_ctor_row)
                .map(|row| {
                    row.into_inner()
                        .filter(|p| p.as_rule() == Rule::expr)
                        .map(build_ast)
                        .collect::<DaxResult<_>>()
                })
                .collect::<DaxResult<_>>()?;
            Ok(DaxExpr::TableConstructor(rows))
        }
        _ => unreachable!("Unexpected rule: {:?}", pair.as_rule()),
    }
}

fn parse_binary(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    let mut inner = pair.into_inner();
    let mut lhs = build_ast(inner.next().expect("Missing LHS for binary expression"))?;

    while let Some(op_pair) = inner.next() {
        let rhs_pair = inner.next().expect("Missing RHS for binary expression");
        let rhs = build_ast(rhs_pair)?;
        let op = match op_pair.as_rule() {
            Rule::in_op => "IN".to_string(),
            Rule::not_in_op => "NOT IN".to_string(),
            _ => op_pair.as_str().to_string(),
        };
        lhs = DaxExpr::BinaryOp { lhs: Box::new(lhs), rhs: Box::new(rhs), op };
    }

    Ok(lhs)
}

fn parse_column_ref(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    let s = pair.as_str();
    if let Some(start) = s.find('[') {
        let table_raw = s[..start].trim();
        let table = if table_raw.starts_with('\'') && table_raw.ends_with('\'') {
            table_raw[1..table_raw.len() - 1].to_string()
        } else {
            table_raw.to_string()
        };
        let column = s[start + 1..s.len() - 1].trim().to_string();
        Ok(DaxExpr::ColumnRef { table, column })
    } else {
        Err(DaxError::Parse(format!("invalid column reference: '{s}'")))
    }
}

fn parse_function_call(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    let mut inner = pair.into_inner();
    let name = inner
        .next()
        .expect("Function name missing")
        .as_str()
        .to_string();
    let mut args = Vec::new();
    if let Some(next) = inner.next() {
        match next.as_rule() {
            Rule::expr_list => {
                args = next.into_inner().map(build_ast).collect::<DaxResult<_>>()?;
            }
            _ => {
                args.push(build_ast(next)?);
            }
        }
    }
    Ok(DaxExpr::FunctionCall { name, args })
}

fn parse_var_expr(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    let mut bindings = Vec::new();
    let mut result = None;
    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::var_binding => {
                let mut inner = child.into_inner();
                let name = inner
                    .next()
                    .expect("VAR binding: missing name")
                    .as_str()
                    .to_string();
                let expr = build_ast(inner.next().expect("VAR binding: missing expression"))?;
                bindings.push((name, Box::new(expr)));
            }
            Rule::expr => {
                result = Some(Box::new(build_ast(child)?));
            }
            _ => unreachable!("Unexpected child in var_expr: {:?}", child.as_rule()),
        }
    }
    Ok(DaxExpr::VarExpr {
        bindings,
        result: result.expect("VAR expression missing RETURN body"),
    })
}

fn parse_unary(pair: Pair<Rule>) -> DaxResult<DaxExpr> {
    let mut inner = pair.into_inner();
    let first = inner.next().expect("unary: missing child");
    if first.as_rule() == Rule::unary_op {
        let op_str = first.as_str().trim().to_ascii_uppercase();
        let expr = build_ast(inner.next().expect("unary: missing operand"))?;
        Ok(DaxExpr::UnaryOp { op: op_str, expr: Box::new(expr) })
    } else {
        build_ast(first)
    }
}

fn parse_literal(pair: Pair<Rule>) -> DaxExpr {
    let inner_pair = pair
        .into_inner()
        .next()
        .expect("Literal missing inner value");
    match inner_pair.as_rule() {
        Rule::number => {
            let val: f64 = inner_pair.as_str().parse().expect("Failed to parse number");
            DaxExpr::Literal(Literal::Number(val))
        }
        Rule::string => {
            let s = inner_pair.as_str();
            let stripped = &s[1..s.len() - 1];
            DaxExpr::Literal(Literal::String(stripped.to_string()))
        }
        _ => unreachable!("Unexpected literal rule: {:?}", inner_pair.as_rule()),
    }
}
