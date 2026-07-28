#[derive(Debug, Clone)]
pub enum DaxExpr {
    Literal(Literal),
    ColumnRef {
        table: String,
        column: String,
    },
    Identifier(String),
    FunctionCall {
        name: String,
        args: Vec<DaxExpr>,
    },
    BinaryOp {
        op: String,
        lhs: Box<DaxExpr>,
        rhs: Box<DaxExpr>,
    },
    VarExpr {
        bindings: Vec<(String, Box<DaxExpr>)>,
        result: Box<DaxExpr>,
    },
    UnaryOp {
        op: String,
        expr: Box<DaxExpr>,
    },
    MeasureRef(String),
    TableConstructor(Vec<Vec<DaxExpr>>),
}

#[derive(Debug, Clone)]
pub enum Literal {
    Number(f64),
    String(String),
    DateTime(i64),
}

#[derive(Debug, Clone)]
pub struct DaxQuery {
    pub define: Vec<Definition>,
    pub statements: Vec<EvaluateStatement>,
}

#[derive(Debug, Clone)]
pub enum Definition {
    Var {
        name: String,
        expr: Box<DaxExpr>,
    },
    Measure {
        table: String,
        name: String,
        expr: Box<DaxExpr>,
    },
}

#[derive(Debug, Clone)]
pub struct EvaluateStatement {
    pub expr: Box<DaxExpr>,
    pub order_by: Vec<(DaxExpr, SortDir)>,
    pub start_at: Vec<DaxExpr>,
}

#[derive(Debug, Clone)]
pub enum SortDir {
    Asc,
    Desc,
}
