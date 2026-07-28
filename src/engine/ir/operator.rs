use crate::engine::ir::expr_node::{BoundExprNode, ExprNode};
use polars::prelude::DataType;

#[derive(Debug, Clone, PartialEq)]
pub enum CrossFilterDirection {
    None,
    OneWay,
    Both,
}

#[derive(Debug, Clone)]
pub enum LiteralValue {
    Integer(i64),
    Number(f64),
    String(String),
    Boolean(bool),
    /// Milliseconds since Unix epoch (UTC), matching Polars Datetime(Milliseconds).
    DateTime(i64),
    CrossFilterDirection(CrossFilterDirection),
    Blank,
}

impl LiteralValue {
    pub fn dtype(&self) -> DataType {
        match self {
            LiteralValue::Integer(_) => DataType::Int64,
            LiteralValue::Number(_) => DataType::Float64,
            LiteralValue::String(_) => DataType::String,
            LiteralValue::Boolean(_) => DataType::Boolean,
            LiteralValue::DateTime(_) => {
                DataType::Datetime(polars::prelude::TimeUnit::Milliseconds, None)
            }
            LiteralValue::CrossFilterDirection(_) => DataType::String,
            LiteralValue::Blank => DataType::Null,
        }
    }
}

pub fn infer_binary_dtype(op: &BinaryOperator, left: &DataType, right: &DataType) -> DataType {
    use polars::prelude::TimeUnit;
    match op {
        BinaryOperator::Eq
        | BinaryOperator::Neq
        | BinaryOperator::Gt
        | BinaryOperator::Lt
        | BinaryOperator::Gte
        | BinaryOperator::Lte
        | BinaryOperator::And
        | BinaryOperator::Or
        | BinaryOperator::In
        | BinaryOperator::NotIn => DataType::Boolean,

        BinaryOperator::Div => DataType::Float64,

        BinaryOperator::Concat => DataType::String,

        BinaryOperator::Sub => match (left, right) {
            (DataType::Datetime(_, _), DataType::Datetime(_, _)) => DataType::Int64,
            (DataType::Datetime(_, _), _) => DataType::Datetime(TimeUnit::Milliseconds, None),
            _ => promote_numeric(left, right),
        },

        BinaryOperator::Add => match (left, right) {
            (DataType::Datetime(_, _), _) | (_, DataType::Datetime(_, _)) => {
                DataType::Datetime(TimeUnit::Milliseconds, None)
            }
            _ => promote_numeric(left, right),
        },

        _ => promote_numeric(left, right),
    }
}

fn promote_numeric(a: &DataType, b: &DataType) -> DataType {
    match (a, b) {
        (DataType::Float64, _) | (_, DataType::Float64) => DataType::Float64,
        (DataType::Int64, _) | (_, DataType::Int64) => DataType::Int64,
        (DataType::Int32, _) | (_, DataType::Int32) => DataType::Int32,
        _ => a.clone(),
    }
}

#[derive(Debug, Clone)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone)]
pub struct MeasureRef {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct FunctionCall {
    pub name: String,
    pub args: Vec<ExprNode>,
}

#[derive(Debug, Clone)]
pub struct BinaryOpNode {
    pub left: Box<ExprNode>,
    pub right: Box<ExprNode>,
    pub op: BinaryOperator,
}

#[derive(Debug, Clone)]
pub enum BinaryOperator {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Gt,
    Lt,
    Gte,
    Lte,
    And,
    Or,
    Concat,
    In,
    NotIn,
}

impl BinaryOperator {
    pub fn flip(self) -> Self {
        match self {
            BinaryOperator::Gt => BinaryOperator::Lt,
            BinaryOperator::Lt => BinaryOperator::Gt,
            BinaryOperator::Gte => BinaryOperator::Lte,
            BinaryOperator::Lte => BinaryOperator::Gte,
            other => other,
        }
    }
}

#[derive(Debug, Clone)]
pub struct UnaryOpNode {
    pub op: UnaryOperator,
    pub expr: Box<ExprNode>,
}

#[derive(Debug, Clone)]
pub enum UnaryOperator {
    Negate,
    Not,
}

#[derive(Debug, Clone)]
pub struct CalculateNode {
    pub expression: Box<ExprNode>,
    pub filters: Vec<ExprNode>,
}

#[derive(Debug, Clone)]
pub struct SummarizeNode {
    pub table: Box<ExprNode>,
    pub group_by: Vec<ExprNode>,
    pub rollup_cols: Vec<(ExprNode, Option<String>)>,
    pub extensions: Vec<(String, ExprNode)>,
}

#[derive(Debug, Clone)]
pub struct VarNode {
    pub bindings: Vec<(String, ExprNode)>,
    pub result: Box<ExprNode>,
}

#[derive(Debug, Clone)]
pub struct SummarizeColumnsNode {
    pub group_by_cols: Vec<ExprNode>,
    pub rollup_groups: Vec<Vec<(Vec<ExprNode>, Option<String>)>>,
    pub filters: Vec<ExprNode>,
    pub extensions: Vec<(String, ExprNode, bool)>,
}

#[derive(Debug, Clone)]
pub struct BoundLiteral {
    pub value: LiteralValue,
    pub dtype: DataType,
}

#[derive(Debug, Clone)]
pub struct BoundColumn {
    pub table: String,
    pub column: String,
    pub dtype: DataType,
}

#[derive(Debug, Clone)]
pub struct BoundMeasure {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BoundCalculate {
    pub expression: Box<BoundExprNode>,
    pub filters: Vec<BoundExprNode>,
    pub dtype: Option<DataType>,
}

#[derive(Debug, Clone)]
pub struct BoundSummarize {
    pub table: Box<BoundExprNode>,
    pub group_by: Vec<BoundExprNode>,
    pub rollup_cols: Vec<(BoundExprNode, Option<String>)>,
    pub extensions: Vec<(String, BoundExprNode)>,
}

pub type SummarizeExtensions = Vec<(String, BoundExprNode, bool)>;

#[derive(Debug, Clone)]
pub struct BoundSummarizeColumns {
    pub group_by_cols: Vec<BoundExprNode>,
    pub rollup_groups: Vec<Vec<(Vec<BoundExprNode>, Option<String>)>>,
    pub filters: Vec<BoundExprNode>,
    pub extensions: SummarizeExtensions,
}

#[derive(Debug, Clone)]
pub struct BoundVar {
    pub bindings: Vec<(String, BoundExprNode)>,
    pub result: Box<BoundExprNode>,
}

#[derive(Debug, Clone)]
pub struct BoundTable {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct BoundUnaryOp {
    pub op: UnaryOperator,
    pub expr: Box<BoundExprNode>,
    pub dtype: Option<DataType>,
}

#[derive(Debug, Clone)]
pub struct BoundBinaryOp {
    pub left: Box<BoundExprNode>,
    pub right: Box<BoundExprNode>,
    pub op: BinaryOperator,
    pub dtype: Option<DataType>,
}

#[derive(Debug, Clone)]
pub struct BoundFunction {
    pub name: String,
    pub args: Vec<BoundExprNode>,
    pub dtype: Option<DataType>,
}
