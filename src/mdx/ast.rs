#[derive(Debug, Clone)]
pub struct MdxQuery {
    /// Inline measure definitions from `WITH MEASURE`/`WITH MEMBER` clauses.
    /// Each entry is `(lowercase_name, dax_expression)`.
    pub calc_measures: Vec<(String, String)>,
    /// Named sets defined in `WITH Set <Name> As <SetExpr>` clauses.
    /// Each entry is `(lowercase_name, set_expr)`.
    pub named_sets: Vec<(String, SetExpr)>,
    pub from: FromClause,
    pub axes: Vec<Axis>,
    pub slicer: Vec<MemberExpr>,
    pub cell_props: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum FromClause {
    System {
        table: String,
        columns: Vec<String>,
        conditions: Vec<Condition>,
    },
    Cube(String),
    SubqueryCube {
        cube: String,
        key_members: Vec<MemberRef>,
    },
}

// ── $system conditions ────────────────────────────────────────────────────────

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

// ── cube axes ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Axis {
    pub id: u32,
    pub non_empty: bool,
    pub set: SetExpr,
    pub dim_props: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum SetExpr {
    Literal(Vec<SetItem>),
    Hierarchize(Box<SetExpr>),
    AddCalculatedMembers(Box<SetExpr>),
    DrilldownLevel(Box<SetExpr>),
    /// CrossJoin(left, right) — produces a set of two-member tuples.
    CrossJoin(Box<SetExpr>, Box<SetExpr>),
    /// DrilldownMember(base, members_to_drill, hierarchy_parts).
    DrilldownMember {
        base: Box<SetExpr>,
        members: Box<SetExpr>,
        /// Bracket-stripped parts of the optional third hierarchy argument,
        /// e.g. ["Product", "ProductType"].
        hier: Option<Vec<String>>,
    },
    /// Generate(set, body) — for each member in `set`, evaluates `body` and unions results.
    Generate(Box<SetExpr>, Box<SetExpr>),
    /// Ascendants(member) — returns the member and all its ancestors up to the All level.
    Ascendants(Box<MemberExpr>),
    /// Reference to a named set defined in a WITH Set clause.
    NamedSetRef(String),
}

#[derive(Debug, Clone)]
pub enum SetItem {
    Set(SetExpr),
    Member(MemberExpr),
}

// ── member references ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MemberExpr {
    pub member: MemberRef,
    pub traversal: Option<Traversal>,
}

#[derive(Debug, Clone)]
pub struct MemberRef {
    /// Bracket-stripped path components, e.g. ["vtest_product", "Color", "Color"].
    pub parts: Vec<String>,
    /// Present for `.&[key]` references.
    pub key: Option<String>,
}

impl MemberRef {
    pub fn is_measure(&self) -> bool {
        self.parts
            .first()
            .map(|s| s.eq_ignore_ascii_case("Measures"))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Traversal {
    AllMembers,
    Members,
    Children,
    CurrentMember,
}
