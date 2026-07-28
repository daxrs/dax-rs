use crate::engine::ir::operator::{
    BinaryOpNode, BoundBinaryOp, BoundCalculate, BoundColumn, BoundFunction, BoundLiteral,
    BoundMeasure, BoundSummarize, BoundSummarizeColumns, BoundTable, BoundUnaryOp, BoundVar,
    CalculateNode, ColumnRef, FunctionCall, LiteralValue, SummarizeColumnsNode, SummarizeNode,
    UnaryOpNode, VarNode,
};
use polars::prelude::DataType;

#[derive(Debug, Clone)]
pub enum ExprNode {
    Literal(LiteralValue),
    Column(ColumnRef),
    Function(FunctionCall),
    BinaryOp(BinaryOpNode),
    UnaryOp(UnaryOpNode),
    Identifier(String),
    MeasureRef(String),
    Calculate(CalculateNode),
    Summarize(SummarizeNode),
    SummarizeColumns(SummarizeColumnsNode),
    Var(VarNode),
    TableConstructor(Vec<Vec<ExprNode>>),
}

#[derive(Debug, Clone)]
pub enum BoundExprNode {
    Literal(BoundLiteral),
    UnaryOp(BoundUnaryOp),
    BinaryOp(BoundBinaryOp),
    Column(BoundColumn),
    Measure(BoundMeasure),
    Table(BoundTable),
    Function(BoundFunction),
    Calculate(BoundCalculate),
    Summarize(BoundSummarize),
    SummarizeColumns(BoundSummarizeColumns),
    TableConstructor(Vec<Vec<BoundExprNode>>),
    Var(BoundVar),
    VarRef(String),
}

impl BoundExprNode {
    pub fn dtype(&self) -> Option<DataType> {
        match self {
            BoundExprNode::Literal(l) => Some(l.dtype.clone()),
            BoundExprNode::Column(c) => Some(c.dtype.clone()),
            BoundExprNode::BinaryOp(op) => op.dtype.clone(),
            BoundExprNode::Function(f) => f.dtype.clone(),
            BoundExprNode::Calculate(c) => c.dtype.clone(),
            BoundExprNode::UnaryOp(op) => op.dtype.clone(),
            BoundExprNode::Measure(_) => None,
            BoundExprNode::Table(_) => None,
            BoundExprNode::Summarize(_) => None,
            BoundExprNode::SummarizeColumns(_) => None,
            BoundExprNode::TableConstructor(_) => None,
            BoundExprNode::Var(v) => v.result.dtype(),
            BoundExprNode::VarRef(_) => None,
        }
    }
}
