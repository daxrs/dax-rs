use super::{parse_mdx, Rule};
use pest::iterators::{Pair, Pairs};

#[derive(Debug, Clone)]
pub struct Query {
    pub with_items: Vec<WithItem>,
    pub body: QueryBody,
}

#[derive(Debug, Clone)]
pub enum QueryBody {
    Cube(CubeQuery),
    System(SystemQuery),
}

#[derive(Debug, Clone)]
pub struct CubeQuery {
    pub axes: Vec<Axis>,
    pub from: CubeFrom,
    pub where_clause: Option<Expr>,
    pub cell_props: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Axis {
    pub non_empty: bool,
    pub expr: Expr,
    pub dim_props: Vec<String>,
    pub id: AxisId,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AxisId {
    Columns,
    Rows,
    Pages,
    Chapters,
    Sections,
    Index(u32),
}

#[derive(Debug, Clone)]
pub enum CubeFrom {
    Cube(String),
    Subquery(Box<Subquery>),
}

#[derive(Debug, Clone)]
pub struct Subquery {
    pub axes: Vec<SubqAxis>,
    pub from: String,
    pub where_clause: Option<Expr>,
}

#[derive(Debug, Clone)]
pub struct SubqAxis {
    pub expr: Expr,
    pub dim_props: Vec<String>,
    pub id: AxisId,
}

#[derive(Debug, Clone)]
pub struct SystemQuery {
    pub columns: Vec<String>,
    pub table: String,
    pub conditions: Vec<Condition>,
}

#[derive(Debug, Clone)]
pub struct Condition {
    pub column: String,
    pub op: ConditionOp,
    pub value: ConditionValue,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConditionOp {
    Eq,
    Ne,
    IsTrue,
}

#[derive(Debug, Clone)]
pub enum ConditionValue {
    Literal(String),
    Param(String),
}

#[derive(Debug, Clone)]
pub enum WithItem {
    Measure {
        table: String,
        column: String,
        dax: String,
    },
    Member {
        name: MemberPath,
        expr: Expr,
        solve_order: Option<i64>,
        props: Vec<(String, Expr)>,
    },
    Set {
        name: MemberPath,
        expr: Expr,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberPath {
    pub segments: Vec<PathSegment>,
}

impl MemberPath {
    pub fn is_measure(&self) -> bool {
        self.segments
            .first()
            .map(|s| match s {
                PathSegment::Bracketed(n) | PathSegment::Bare(n) => {
                    n.eq_ignore_ascii_case("Measures")
                }
                PathSegment::Key(_) => false,
            })
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PathSegment {
    Bracketed(String),
    Bare(String),
    Key(Option<String>),
}

#[derive(Debug, Clone)]
pub enum Expr {
    Member(MemberPath),
    Tuple(Vec<Expr>),
    Set(Vec<Expr>),
    FunctionCall {
        name: String,
        args: Vec<Option<Expr>>,
    },
    MemberFunction {
        base: Box<Expr>,
        name: String,
        args: Vec<Option<Expr>>,
    },
    BinaryOp {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    UnaryOp {
        op: UnaryOpKind,
        expr: Box<Expr>,
    },
    Range {
        lo: Box<Expr>,
        hi: Box<Expr>,
    },
    Is {
        lhs: Box<Expr>,
        rhs: Option<Box<Expr>>,
        negated: bool,
    },
    Case {
        operand: Option<Box<Expr>>,
        whens: Vec<(Expr, Expr)>,
        else_: Option<Box<Expr>>,
    },
    Literal(Literal),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Concat,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnaryOpKind {
    Neg,
    Not,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    String(String),
    Number(f64),
    Null,
}

pub fn parse(input: &str) -> Result<Query, Box<pest::error::Error<Rule>>> {
    Ok(build_ast(parse_mdx(input)?))
}

pub fn build_ast(pairs: Pairs<Rule>) -> Query {
    let mdx_query = pairs
        .into_iter()
        .find(|p| p.as_rule() == Rule::mdx_query)
        .expect("mdx_query");
    let select_stmt = mdx_query
        .into_inner()
        .find(|p| p.as_rule() == Rule::select_stmt)
        .expect("select_stmt");
    build_select_stmt(select_stmt)
}

fn build_select_stmt(pair: Pair<Rule>) -> Query {
    let mut inner = pair.into_inner().peekable();
    let with_items = if inner.peek().map(|p| p.as_rule()) == Some(Rule::with_clause) {
        build_with_clause(inner.next().unwrap())
    } else {
        Vec::new()
    };
    let body_pair = inner.next().expect("cube_select or system_select");
    let body = match body_pair.as_rule() {
        Rule::cube_select => QueryBody::Cube(build_cube_select(body_pair)),
        Rule::system_select => QueryBody::System(build_system_select(body_pair)),
        r => unreachable!("unexpected select body rule {r:?}"),
    };
    Query { with_items, body }
}

fn build_with_clause(pair: Pair<Rule>) -> Vec<WithItem> {
    pair.into_inner().map(build_with_item).collect()
}

fn build_with_item(pair: Pair<Rule>) -> WithItem {
    let inner = pair.into_inner().next().expect("with_item body");
    match inner.as_rule() {
        Rule::measure_def => build_measure_def(inner),
        Rule::member_def => build_member_def(inner),
        Rule::set_def => build_set_def(inner),
        r => unreachable!("unexpected with_item rule {r:?}"),
    }
}

fn build_measure_def(pair: Pair<Rule>) -> WithItem {
    let mut inner = pair.into_inner();
    let quoted_table_ref = inner.next().expect("quoted_table_ref");
    let dax_pair = inner.next().expect("dax_passthrough");
    let mut tr_inner = quoted_table_ref.into_inner();
    let table = unescape_quoted(tr_inner.next().expect("string_literal").as_str(), '\'');
    let column = strip_bracketed(tr_inner.next().expect("bracketed_ident").as_str());
    let dax = dax_pair.as_str().trim().to_string();
    WithItem::Measure { table, column, dax }
}

fn build_member_def(pair: Pair<Rule>) -> WithItem {
    let mut inner = pair.into_inner();
    let name = build_member_name(inner.next().expect("member_name"));
    let expr = build_expr(inner.next().expect("expr"));
    let mut solve_order = None;
    let mut props = Vec::new();
    for extra in inner {
        let item = extra.into_inner().next().expect("member_extra body");
        match item.as_rule() {
            Rule::solve_order => {
                let n = item
                    .into_inner()
                    .next()
                    .expect("integer_literal")
                    .as_str()
                    .parse()
                    .expect("valid integer");
                solve_order = Some(n);
            }
            Rule::member_prop => {
                let mut p = item.into_inner();
                let key = p.next().expect("plain_ident").as_str().to_string();
                let value = build_expr(p.next().expect("expr"));
                props.push((key, value));
            }
            r => unreachable!("unexpected member_extra rule {r:?}"),
        }
    }
    WithItem::Member {
        name,
        expr,
        solve_order,
        props,
    }
}

fn build_set_def(pair: Pair<Rule>) -> WithItem {
    let mut inner = pair.into_inner();
    let name = build_member_name(inner.next().expect("member_name"));
    let expr = build_expr(inner.next().expect("expr"));
    WithItem::Set { name, expr }
}

fn build_member_name(pair: Pair<Rule>) -> MemberPath {
    let mut inner = pair.into_inner();
    let mut segments = build_compound_name(inner.next().expect("compound_name")).segments;
    for tail in inner {
        segments.push(PathSegment::Bare(tail.as_str().to_string()));
    }
    MemberPath { segments }
}

fn build_compound_name(pair: Pair<Rule>) -> MemberPath {
    let mut segments = Vec::new();
    for part in pair.into_inner() {
        match part.as_rule() {
            Rule::bracketed_ident => {
                segments.push(PathSegment::Bracketed(strip_bracketed(part.as_str())))
            }
            Rule::plain_ident => segments.push(PathSegment::Bare(part.as_str().to_string())),
            Rule::compound_tail => {
                let child = part.into_inner().next().expect("compound_tail body");
                match child.as_rule() {
                    Rule::bracketed_ident => {
                        segments.push(PathSegment::Bracketed(strip_bracketed(child.as_str())))
                    }
                    Rule::key_ref => {
                        let key = child
                            .into_inner()
                            .next()
                            .map(|b| strip_bracketed(b.as_str()));
                        segments.push(PathSegment::Key(key));
                    }
                    r => unreachable!("unexpected compound_tail rule {r:?}"),
                }
            }
            r => unreachable!("unexpected compound_name part rule {r:?}"),
        }
    }
    MemberPath { segments }
}

fn build_system_select(pair: Pair<Rule>) -> SystemQuery {
    let mut inner = pair.into_inner();
    let columns = inner
        .next()
        .expect("col_list")
        .into_inner()
        .map(|p| strip_bracketed(p.as_str()))
        .collect();
    let system_from = inner.next().expect("system_from");
    let table = system_from
        .as_str()
        .rsplit('.')
        .next()
        .expect("table name after $system.")
        .to_string();
    let conditions = match inner.next() {
        Some(cond_list) => cond_list.into_inner().map(build_condition).collect(),
        None => Vec::new(),
    };
    SystemQuery {
        columns,
        table,
        conditions,
    }
}

fn build_condition(pair: Pair<Rule>) -> Condition {
    let inner = pair.into_inner().next().expect("condition body");
    match inner.as_rule() {
        Rule::cond_eq => {
            let mut p = inner.into_inner();
            let column = strip_bracketed(p.next().expect("bracketed_ident").as_str());
            let value = build_cond_value(p.next().expect("cond_value"));
            Condition {
                column,
                op: ConditionOp::Eq,
                value,
            }
        }
        Rule::cond_ne => {
            let mut p = inner.into_inner();
            let column = strip_bracketed(p.next().expect("bracketed_ident").as_str());
            let value = build_cond_value(p.next().expect("cond_value"));
            Condition {
                column,
                op: ConditionOp::Ne,
                value,
            }
        }
        Rule::cond_bool => {
            let column = strip_bracketed(
                inner
                    .into_inner()
                    .next()
                    .expect("bracketed_ident")
                    .as_str(),
            );
            Condition {
                column,
                op: ConditionOp::IsTrue,
                value: ConditionValue::Literal(String::new()),
            }
        }
        r => unreachable!("unexpected condition rule {r:?}"),
    }
}

fn build_cond_value(pair: Pair<Rule>) -> ConditionValue {
    let inner = pair.into_inner().next().expect("cond_value body");
    match inner.as_rule() {
        Rule::param_ref => {
            let name = inner
                .into_inner()
                .next()
                .expect("plain_ident")
                .as_str()
                .to_string();
            ConditionValue::Param(name)
        }
        Rule::string_literal => ConditionValue::Literal(unescape_quoted(inner.as_str(), '\'')),
        Rule::integer_literal => ConditionValue::Literal(inner.as_str().to_string()),
        r => unreachable!("unexpected cond_value rule {r:?}"),
    }
}

fn build_cube_select(pair: Pair<Rule>) -> CubeQuery {
    let mut axes = Vec::new();
    let mut from = None;
    let mut where_clause = None;
    let mut cell_props = Vec::new();
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::axis_clause => axes.push(build_axis_clause(p)),
            Rule::cube_from => from = Some(build_cube_from(p)),
            Rule::expr => where_clause = Some(build_expr(p)),
            Rule::prop_list => cell_props = build_prop_list(p),
            r => unreachable!("unexpected cube_select child {r:?}"),
        }
    }
    CubeQuery {
        axes,
        from: from.expect("cube_from"),
        where_clause,
        cell_props,
    }
}

fn build_cube_from(pair: Pair<Rule>) -> CubeFrom {
    let inner = pair.into_inner().next().expect("cube_from body");
    match inner.as_rule() {
        Rule::bracketed_ident => CubeFrom::Cube(strip_bracketed(inner.as_str())),
        Rule::subquery => CubeFrom::Subquery(Box::new(build_subquery(inner))),
        r => unreachable!("unexpected cube_from rule {r:?}"),
    }
}

fn build_subquery(pair: Pair<Rule>) -> Subquery {
    let mut axes = Vec::new();
    let mut from = None;
    let mut where_clause = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::subq_axis => axes.push(build_subq_axis(p)),
            Rule::bracketed_ident => from = Some(strip_bracketed(p.as_str())),
            Rule::expr => where_clause = Some(build_expr(p)),
            r => unreachable!("unexpected subquery child {r:?}"),
        }
    }
    Subquery {
        axes,
        from: from.expect("cube name"),
        where_clause,
    }
}

fn build_subq_axis(pair: Pair<Rule>) -> SubqAxis {
    let mut expr = None;
    let mut dim_props = Vec::new();
    let mut id = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::expr => expr = Some(build_expr(p)),
            Rule::prop_list => dim_props = build_prop_list(p),
            Rule::axis_id => id = Some(build_axis_id(p)),
            r => unreachable!("unexpected subq_axis child {r:?}"),
        }
    }
    SubqAxis {
        expr: expr.expect("expr"),
        dim_props,
        id: id.expect("axis_id"),
    }
}

fn build_axis_clause(pair: Pair<Rule>) -> Axis {
    let mut non_empty = false;
    let mut expr = None;
    let mut dim_props = Vec::new();
    let mut id = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::kw_non_empty => non_empty = true,
            Rule::expr => expr = Some(build_expr(p)),
            Rule::prop_list => dim_props = build_prop_list(p),
            Rule::axis_id => id = Some(build_axis_id(p)),
            r => unreachable!("unexpected axis_clause child {r:?}"),
        }
    }
    Axis {
        non_empty,
        expr: expr.expect("expr"),
        dim_props,
        id: id.expect("axis_id"),
    }
}

fn build_axis_id(pair: Pair<Rule>) -> AxisId {
    let inner = pair.into_inner().next().expect("axis_id body");
    match inner.as_rule() {
        Rule::kw_columns => AxisId::Columns,
        Rule::kw_rows => AxisId::Rows,
        Rule::kw_pages => AxisId::Pages,
        Rule::kw_chapters => AxisId::Chapters,
        Rule::kw_sections => AxisId::Sections,
        Rule::integer_literal => {
            AxisId::Index(inner.as_str().parse().expect("valid axis index"))
        }
        r => unreachable!("unexpected axis_id rule {r:?}"),
    }
}

fn build_prop_list(pair: Pair<Rule>) -> Vec<String> {
    pair.into_inner()
        .map(|p| strip_optional_brackets(p.as_str()))
        .collect()
}

fn build_expr(pair: Pair<Rule>) -> Expr {
    build_or_expr(pair.into_inner().next().expect("or_expr"))
}

fn build_or_expr(pair: Pair<Rule>) -> Expr {
    let mut parts = pair.into_inner().map(build_and_expr);
    let mut acc = parts.next().expect("and_expr");
    for rhs in parts {
        acc = Expr::BinaryOp {
            op: BinOp::Or,
            lhs: Box::new(acc),
            rhs: Box::new(rhs),
        };
    }
    acc
}

fn build_and_expr(pair: Pair<Rule>) -> Expr {
    let mut parts = pair.into_inner().map(build_not_expr);
    let mut acc = parts.next().expect("not_expr");
    for rhs in parts {
        acc = Expr::BinaryOp {
            op: BinOp::And,
            lhs: Box::new(acc),
            rhs: Box::new(rhs),
        };
    }
    acc
}

fn build_not_expr(pair: Pair<Rule>) -> Expr {
    let inner = pair.into_inner().next().expect("not_expr body");
    match inner.as_rule() {
        Rule::negated_expr => {
            let compare = inner.into_inner().next().expect("compare_expr");
            Expr::UnaryOp {
                op: UnaryOpKind::Not,
                expr: Box::new(build_compare_expr(compare)),
            }
        }
        Rule::compare_expr => build_compare_expr(inner),
        r => unreachable!("unexpected not_expr rule {r:?}"),
    }
}

fn build_compare_expr(pair: Pair<Rule>) -> Expr {
    let mut inner = pair.into_inner();
    let lhs = build_concat_expr(inner.next().expect("concat_expr"));
    match inner.next() {
        None => lhs,
        Some(tail) => match tail.as_rule() {
            Rule::is_tail => build_is_tail(lhs, tail),
            Rule::compare_op => {
                let op = match tail.as_str() {
                    "<>" => BinOp::Ne,
                    "<=" => BinOp::Le,
                    ">=" => BinOp::Ge,
                    "=" => BinOp::Eq,
                    "<" => BinOp::Lt,
                    ">" => BinOp::Gt,
                    s => unreachable!("unexpected compare_op {s:?}"),
                };
                let rhs = build_concat_expr(inner.next().expect("concat_expr"));
                Expr::BinaryOp {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                }
            }
            r => unreachable!("unexpected compare_expr tail rule {r:?}"),
        },
    }
}

fn build_is_tail(lhs: Expr, pair: Pair<Rule>) -> Expr {
    let inner = pair.into_inner().next().expect("is_tail body");
    match inner.as_rule() {
        Rule::negated_is_tail => {
            let target = inner.into_inner().next().expect("negated_is_tail body");
            let rhs = match target.as_rule() {
                Rule::kw_null => None,
                Rule::concat_expr => Some(Box::new(build_concat_expr(target))),
                r => unreachable!("unexpected negated_is_tail body rule {r:?}"),
            };
            Expr::Is {
                lhs: Box::new(lhs),
                rhs,
                negated: true,
            }
        }
        Rule::kw_null => Expr::Is {
            lhs: Box::new(lhs),
            rhs: None,
            negated: false,
        },
        Rule::concat_expr => Expr::Is {
            lhs: Box::new(lhs),
            rhs: Some(Box::new(build_concat_expr(inner))),
            negated: false,
        },
        r => unreachable!("unexpected is_tail body rule {r:?}"),
    }
}

fn build_concat_expr(pair: Pair<Rule>) -> Expr {
    let mut parts = pair.into_inner().map(build_add_expr);
    let mut acc = parts.next().expect("add_expr");
    for rhs in parts {
        acc = Expr::BinaryOp {
            op: BinOp::Concat,
            lhs: Box::new(acc),
            rhs: Box::new(rhs),
        };
    }
    acc
}

fn build_add_expr(pair: Pair<Rule>) -> Expr {
    let mut inner = pair.into_inner();
    let mut acc = build_range_expr(inner.next().expect("range_expr"));
    let mut pending_op: Option<BinOp> = None;
    for p in inner {
        match p.as_rule() {
            Rule::add_op => {
                pending_op = Some(match p.as_str() {
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    s => unreachable!("unexpected add_op {s:?}"),
                });
            }
            Rule::range_expr => {
                let op = pending_op.take().expect("add_op before range_expr");
                let rhs = build_range_expr(p);
                acc = Expr::BinaryOp {
                    op,
                    lhs: Box::new(acc),
                    rhs: Box::new(rhs),
                };
            }
            r => unreachable!("unexpected add_expr child {r:?}"),
        }
    }
    acc
}

fn build_range_expr(pair: Pair<Rule>) -> Expr {
    let mut inner = pair.into_inner();
    let lo = build_mul_expr(inner.next().expect("mul_expr"));
    match inner.next() {
        None => lo,
        Some(hi_pair) => Expr::Range {
            lo: Box::new(lo),
            hi: Box::new(build_mul_expr(hi_pair)),
        },
    }
}

fn build_mul_expr(pair: Pair<Rule>) -> Expr {
    let mut inner = pair.into_inner();
    let mut acc = build_unary_expr(inner.next().expect("unary_expr"));
    let mut pending_op: Option<BinOp> = None;
    for p in inner {
        match p.as_rule() {
            Rule::mul_op => {
                pending_op = Some(match p.as_str() {
                    "*" => BinOp::Mul,
                    "/" => BinOp::Div,
                    s => unreachable!("unexpected mul_op {s:?}"),
                });
            }
            Rule::unary_expr => {
                let op = pending_op.take().expect("mul_op before unary_expr");
                let rhs = build_unary_expr(p);
                acc = Expr::BinaryOp {
                    op,
                    lhs: Box::new(acc),
                    rhs: Box::new(rhs),
                };
            }
            r => unreachable!("unexpected mul_expr child {r:?}"),
        }
    }
    acc
}

fn build_unary_expr(pair: Pair<Rule>) -> Expr {
    let inner = pair.into_inner().next().expect("unary_expr body");
    match inner.as_rule() {
        Rule::unary_expr => Expr::UnaryOp {
            op: UnaryOpKind::Neg,
            expr: Box::new(build_unary_expr(inner)),
        },
        Rule::postfix_expr => build_postfix_expr(inner),
        r => unreachable!("unexpected unary_expr rule {r:?}"),
    }
}

fn build_postfix_expr(pair: Pair<Rule>) -> Expr {
    let mut inner = pair.into_inner();
    let mut acc = build_primary(inner.next().expect("primary"));
    for suffix in inner {
        let child = suffix.into_inner().next().expect("dot_suffix body");
        acc = match child.as_rule() {
            Rule::function_call => {
                let (name, args) = build_function_call_parts(child);
                Expr::MemberFunction {
                    base: Box::new(acc),
                    name,
                    args,
                }
            }
            Rule::bracketed_prop => Expr::MemberFunction {
                base: Box::new(acc),
                name: strip_bracketed(child.as_str()),
                args: Vec::new(),
            },
            Rule::plain_ident => Expr::MemberFunction {
                base: Box::new(acc),
                name: child.as_str().to_string(),
                args: Vec::new(),
            },
            r => unreachable!("unexpected dot_suffix rule {r:?}"),
        };
    }
    acc
}

fn build_primary(pair: Pair<Rule>) -> Expr {
    let inner = pair.into_inner().next().expect("primary body");
    match inner.as_rule() {
        Rule::case_expr => build_case_expr(inner),
        Rule::function_call => {
            let (name, args) = build_function_call_parts(inner);
            Expr::FunctionCall { name, args }
        }
        Rule::tuple_expr => build_tuple_expr(inner),
        Rule::set_literal => build_set_literal(inner),
        Rule::member_ref => Expr::Member(build_member_ref(inner)),
        Rule::literal => build_literal(inner),
        r => unreachable!("unexpected primary rule {r:?}"),
    }
}

fn build_member_ref(pair: Pair<Rule>) -> MemberPath {
    build_compound_name(pair.into_inner().next().expect("compound_name"))
}

fn build_function_call_parts(pair: Pair<Rule>) -> (String, Vec<Option<Expr>>) {
    let mut inner = pair.into_inner();
    let name = inner.next().expect("func_name").as_str().to_string();
    let args = match inner.next() {
        Some(arg_list) => arg_list.into_inner().map(build_arg).collect(),
        None => Vec::new(),
    };
    (name, args)
}

fn build_arg(pair: Pair<Rule>) -> Option<Expr> {
    pair.into_inner().next().map(build_expr)
}

fn build_tuple_expr(pair: Pair<Rule>) -> Expr {
    Expr::Tuple(pair.into_inner().map(build_expr).collect())
}

fn build_set_literal(pair: Pair<Rule>) -> Expr {
    Expr::Set(pair.into_inner().map(build_expr).collect())
}

fn build_case_expr(pair: Pair<Rule>) -> Expr {
    let mut operand = None;
    let mut whens = Vec::new();
    let mut else_ = None;
    for p in pair.into_inner() {
        match p.as_rule() {
            Rule::expr => operand = Some(Box::new(build_expr(p))),
            Rule::when_clause => {
                let mut wi = p.into_inner();
                let cond = build_expr(wi.next().expect("when condition"));
                let then = build_expr(wi.next().expect("then expr"));
                whens.push((cond, then));
            }
            Rule::else_clause => {
                let e = p.into_inner().next().expect("else expr");
                else_ = Some(Box::new(build_expr(e)));
            }
            r => unreachable!("unexpected case_expr child {r:?}"),
        }
    }
    Expr::Case {
        operand,
        whens,
        else_,
    }
}

fn build_literal(pair: Pair<Rule>) -> Expr {
    let inner = pair.into_inner().next().expect("literal body");
    match inner.as_rule() {
        Rule::string_literal => {
            Expr::Literal(Literal::String(unescape_quoted(inner.as_str(), '\'')))
        }
        Rule::dq_string_literal => {
            Expr::Literal(Literal::String(unescape_quoted(inner.as_str(), '"')))
        }
        Rule::kw_null => Expr::Literal(Literal::Null),
        Rule::number_literal => {
            Expr::Literal(Literal::Number(inner.as_str().parse().expect("valid number")))
        }
        r => unreachable!("unexpected literal rule {r:?}"),
    }
}

fn strip_bracketed(s: &str) -> String {
    s[1..s.len() - 1].to_string()
}

fn strip_optional_brackets(s: &str) -> String {
    if s.starts_with('[') && s.ends_with(']') && s.len() >= 2 {
        strip_bracketed(s)
    } else {
        s.to_string()
    }
}

fn unescape_quoted(s: &str, quote: char) -> String {
    let inner = &s[1..s.len() - 1];
    let doubled: String = [quote, quote].iter().collect();
    inner.replace(&doubled, &quote.to_string())
}
