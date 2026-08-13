use super::ast::{self, Expr, Literal, MemberPath, PathSegment, QuoteStyle, WithItem};
use crate::engine::expressions::Value;
use crate::engine::Engine;
use std::cell::RefCell;
use std::collections::HashMap;

pub fn normalize_with_item(item: WithItem) -> Result<WithItem, Box<pest::error::Error<super::Rule>>> {
    match item {
        WithItem::Member {
            name,
            expr,
            solve_order,
            props,
        } => Ok(WithItem::Member {
            name,
            expr: reparse_if_single_quoted(expr)?,
            solve_order,
            props,
        }),
        WithItem::Set { name, expr } => Ok(WithItem::Set {
            name,
            expr: reparse_if_single_quoted(expr)?,
        }),
        WithItem::Measure { .. } => Ok(item),
    }
}

fn reparse_if_single_quoted(expr: Expr) -> Result<Expr, Box<pest::error::Error<super::Rule>>> {
    match &expr {
        Expr::Literal(Literal::String {
            value,
            quote: QuoteStyle::Single,
        }) => ast::parse_expr(value),
        _ => Ok(expr),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Member {
    Leaf {
        table: String,
        hier: String,
        level: String,
        key: String,
        caption: String,
    },
    All {
        table: String,
        hier: String,
    },
    Measure {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HierarchyRef {
    Dimension { table: String, hier: String },
    Measures,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Tuple {
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedSet {
    pub shape: Vec<HierarchyRef>,
    pub tuples: Vec<Tuple>,
}

pub struct EvalCtx<'a> {
    pub engine: &'a Engine,
    pub named: HashMap<String, Expr>,
    pub measures: HashMap<String, String>,
    cache: RefCell<HashMap<(String, String), Vec<Member>>>,
    filtered_cache: RefCell<HashMap<(String, String, String, String), Vec<Member>>>,
}

impl<'a> EvalCtx<'a> {
    pub fn new(engine: &'a Engine) -> Self {
        Self {
            engine,
            named: HashMap::new(),
            measures: catalog_measures(engine),
            cache: RefCell::new(HashMap::new()),
            filtered_cache: RefCell::new(HashMap::new()),
        }
    }

    pub fn from_query(
        engine: &'a Engine,
        query: &ast::Query,
    ) -> Result<Self, Box<pest::error::Error<super::Rule>>> {
        let mut named = HashMap::new();
        let mut measures = catalog_measures(engine);
        for item in &query.with_items {
            match normalize_with_item(item.clone())? {
                WithItem::Set { name, expr } => {
                    if let [PathSegment::Bare(n)] = name.segments.as_slice() {
                        named.insert(n.to_ascii_lowercase(), expr);
                    }
                }
                WithItem::Measure { column, dax, .. } => {
                    measures.insert(column.to_ascii_lowercase(), dax);
                }
                WithItem::Member { .. } => {}
            }
        }
        Ok(Self {
            engine,
            named,
            measures,
            cache: RefCell::new(HashMap::new()),
            filtered_cache: RefCell::new(HashMap::new()),
        })
    }

    pub fn resolve_measure(&self, name: &str) -> Result<&str, String> {
        self.measures
            .get(&name.to_ascii_lowercase())
            .map(|s| s.as_str())
            .ok_or_else(|| format!("unknown measure: {name}"))
    }

    fn resolve_all_members(&self, table: &str, hier: &str) -> Result<Vec<Member>, String> {
        let key = (table.to_string(), hier.to_string());
        if let Some(cached) = self.cache.borrow().get(&key) {
            return Ok(cached.clone());
        }
        let dax = format!("EVALUATE VALUES('{table}'[{hier}])");
        let mut results = self.engine.evaluate_query(&dax).map_err(|e| e.to_string())?;
        let value = results
            .pop()
            .ok_or_else(|| "no result for VALUES query".to_string())?;
        let leaves = table_value_to_leaves(value, table, hier)?;
        self.cache.borrow_mut().insert(key, leaves.clone());
        Ok(leaves)
    }

    fn resolve_all_members_filtered(
        &self,
        table: &str,
        hier: &str,
        filter_hier: &str,
        filter_key: &str,
    ) -> Result<Vec<Member>, String> {
        let key = (
            table.to_string(),
            hier.to_string(),
            filter_hier.to_string(),
            filter_key.to_string(),
        );
        if let Some(cached) = self.filtered_cache.borrow().get(&key) {
            return Ok(cached.clone());
        }
        let filter = if filter_key.is_empty() {
            format!("FILTER(ALL('{table}'[{filter_hier}]), ISBLANK('{table}'[{filter_hier}]))")
        } else {
            format!("'{table}'[{filter_hier}] = \"{filter_key}\"")
        };
        let dax = format!("EVALUATE CALCULATETABLE(VALUES('{table}'[{hier}]), {filter})");
        let mut results = self.engine.evaluate_query(&dax).map_err(|e| e.to_string())?;
        let value = results
            .pop()
            .ok_or_else(|| "no result for filtered VALUES query".to_string())?;
        let leaves = table_value_to_leaves(value, table, hier)?;
        self.filtered_cache.borrow_mut().insert(key, leaves.clone());
        Ok(leaves)
    }
}

fn catalog_measures(engine: &Engine) -> HashMap<String, String> {
    engine
        .ctx()
        .catalog
        .measures
        .iter()
        .map(|(name, dax)| (name.to_ascii_lowercase(), dax.clone()))
        .collect()
}

pub fn eval_slicer(where_clause: &Expr, ctx: &EvalCtx) -> Result<Vec<Member>, String> {
    let set = eval_set(where_clause, ctx)?;
    match set.tuples.as_slice() {
        [tuple] => Ok(tuple.members.clone()),
        [] => Err("WHERE clause evaluated to an empty set".to_string()),
        _ => Err("WHERE clause must evaluate to a single tuple".to_string()),
    }
}

fn table_value_to_leaves(value: Value, table: &str, hier: &str) -> Result<Vec<Member>, String> {
    match value {
        Value::Table(_, df) => {
            let col_names = df.get_column_names();
            let col_name = col_names
                .first()
                .ok_or_else(|| "VALUES query returned no columns".to_string())?;
            let series = df.column(col_name).map_err(|e| e.to_string())?;
            let mut leaves = Vec::with_capacity(df.height());
            for i in 0..df.height() {
                let av = series.get(i).map_err(|e| e.to_string())?;
                let text = anyvalue_to_member_text(av);
                leaves.push(Member::Leaf {
                    table: table.to_string(),
                    hier: hier.to_string(),
                    level: hier.to_string(),
                    key: text.clone(),
                    caption: text,
                });
            }
            Ok(leaves)
        }
        Value::Integer(_)
        | Value::Number(_)
        | Value::String(_)
        | Value::Boolean(_)
        | Value::Blank
        | Value::DateTime(_)
        | Value::Series(_) => Err("expected a table result from VALUES query".to_string()),
    }
}

fn anyvalue_to_member_text(v: polars::prelude::AnyValue) -> String {
    use polars::prelude::AnyValue;
    match v {
        AnyValue::Null => String::new(),
        AnyValue::String(s) => s.to_string(),
        AnyValue::StringOwned(s) => s.to_string(),
        AnyValue::Int64(n) => n.to_string(),
        AnyValue::Int32(n) => n.to_string(),
        AnyValue::Int16(n) => n.to_string(),
        AnyValue::Int8(n) => n.to_string(),
        AnyValue::UInt64(n) => n.to_string(),
        AnyValue::UInt32(n) => n.to_string(),
        AnyValue::Float64(n) => n.to_string(),
        AnyValue::Float32(n) => n.to_string(),
        AnyValue::Boolean(b) => b.to_string(),
        other => format!("{other}"),
    }
}

fn segment_text(seg: &PathSegment) -> String {
    match seg {
        PathSegment::Bracketed(s) => s.clone(),
        PathSegment::Bare(s) => s.clone(),
        PathSegment::Key(Some(k)) => k.clone(),
        PathSegment::Key(None) => String::new(),
    }
}

fn is_all_marker(s: &str) -> bool {
    s.eq_ignore_ascii_case("All") || s.eq_ignore_ascii_case("(All)")
}

fn table_hier_of(path: &MemberPath) -> Result<(String, String), String> {
    let table = path
        .segments
        .first()
        .map(segment_text)
        .ok_or_else(|| "empty member path".to_string())?;
    let hier = if path.segments.len() >= 2 {
        segment_text(&path.segments[1])
    } else {
        table.clone()
    };
    Ok((table, hier))
}

fn hierarchy_ref_of(member: &Member) -> HierarchyRef {
    match member {
        Member::Leaf { table, hier, .. } => HierarchyRef::Dimension {
            table: table.clone(),
            hier: hier.clone(),
        },
        Member::All { table, hier } => HierarchyRef::Dimension {
            table: table.clone(),
            hier: hier.clone(),
        },
        Member::Measure { .. } => HierarchyRef::Measures,
    }
}

fn classify_member_path(path: &MemberPath) -> Result<Member, String> {
    if path.is_measure() {
        let name = path
            .segments
            .last()
            .map(segment_text)
            .ok_or_else(|| "empty measure path".to_string())?;
        return Ok(Member::Measure { name });
    }
    let (table, hier) = table_hier_of(path)?;
    let last = path
        .segments
        .last()
        .ok_or_else(|| "empty member path".to_string())?;
    match last {
        PathSegment::Key(key) => Ok(Member::Leaf {
            table,
            hier: hier.clone(),
            level: hier,
            key: key.clone().unwrap_or_default(),
            caption: key.clone().unwrap_or_default(),
        }),
        PathSegment::Bracketed(s) | PathSegment::Bare(s) if is_all_marker(s) => {
            Ok(Member::All { table, hier })
        }
        PathSegment::Bracketed(s) | PathSegment::Bare(s) => Ok(Member::Leaf {
            table,
            hier: hier.clone(),
            level: hier,
            key: s.clone(),
            caption: s.clone(),
        }),
    }
}

pub fn eval_set(expr: &Expr, ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    match expr {
        Expr::Member(path) => eval_member_expr_as_set(path, ctx),
        Expr::Set(items) => {
            let mut shape: Option<Vec<HierarchyRef>> = None;
            let mut tuples = Vec::new();
            for item in items {
                let sub = eval_set(item, ctx)?;
                match &shape {
                    None => shape = Some(sub.shape.clone()),
                    Some(s) if *s == sub.shape => {}
                    Some(_) => return Err("set items have mismatched shapes".to_string()),
                }
                tuples.extend(sub.tuples);
            }
            Ok(EvaluatedSet {
                shape: shape.unwrap_or_default(),
                tuples,
            })
        }
        Expr::Tuple(items) => {
            let mut shape = Vec::with_capacity(items.len());
            let mut members = Vec::with_capacity(items.len());
            for item in items {
                let m = eval_member(item, ctx)?;
                shape.push(hierarchy_ref_of(&m));
                members.push(m);
            }
            Ok(EvaluatedSet {
                shape,
                tuples: vec![Tuple { members }],
            })
        }
        Expr::FunctionCall { name, args } => eval_function_call(name, args, ctx),
        Expr::MemberFunction { base, name, args } => {
            eval_member_function_as_set(base, name, args, ctx)
        }
        Expr::BinaryOp { .. }
        | Expr::UnaryOp { .. }
        | Expr::Range { .. }
        | Expr::Is { .. }
        | Expr::Case { .. }
        | Expr::Literal(_) => Err(format!("{expr:?} is not a set-shaped expression")),
    }
}

fn eval_member(expr: &Expr, _ctx: &EvalCtx) -> Result<Member, String> {
    match expr {
        Expr::Member(path) => classify_member_path(path),
        other => Err(format!("{other:?} is not a member-shaped expression")),
    }
}

fn eval_member_expr_as_set(path: &MemberPath, ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    if let [PathSegment::Bare(name)] = path.segments.as_slice() {
        if let Some(def) = ctx.named.get(&name.to_ascii_lowercase()) {
            return eval_set(def, ctx);
        }
    }
    let member = classify_member_path(path)?;
    let shape = vec![hierarchy_ref_of(&member)];
    Ok(EvaluatedSet {
        shape,
        tuples: vec![Tuple {
            members: vec![member],
        }],
    })
}

fn eval_function_call(name: &str, args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    match name.to_ascii_lowercase().as_str() {
        "crossjoin" => eval_crossjoin(args, ctx),
        "hierarchize" => eval_hierarchize(args, ctx),
        "drilldownlevel" => eval_drilldown_level(args, ctx),
        "drilldownmember" => eval_drilldown_member(args, ctx),
        "addcalculatedmembers" => eval_add_calculated_members(args, ctx),
        other => Err(format!("unsupported function in set position: {other}")),
    }
}

fn eval_crossjoin(args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    let [Some(l), Some(r)] = args else {
        return Err("CrossJoin requires two arguments".to_string());
    };
    let left = eval_set(l, ctx)?;
    let right = eval_set(r, ctx)?;
    let mut shape = left.shape;
    shape.extend(right.shape);
    let mut tuples = Vec::with_capacity(left.tuples.len() * right.tuples.len());
    for lt in &left.tuples {
        for rt in &right.tuples {
            let mut members = lt.members.clone();
            members.extend(rt.members.clone());
            tuples.push(Tuple { members });
        }
    }
    Ok(EvaluatedSet { shape, tuples })
}

fn eval_hierarchize(args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    let [Some(inner)] = args else {
        return Err("Hierarchize requires one argument".to_string());
    };
    eval_set(inner, ctx)
}

fn eval_add_calculated_members(args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    let [Some(inner)] = args else {
        return Err("AddCalculatedMembers requires one argument".to_string());
    };
    eval_set(inner, ctx)
}

fn eval_drilldown_level(args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    let Some(Some(base_expr)) = args.first() else {
        return Err("DrilldownLevel requires a set argument".to_string());
    };
    let base = eval_set(base_expr, ctx)?;
    let [href] = base.shape.as_slice() else {
        return Err("DrilldownLevel expects a single-hierarchy set".to_string());
    };
    let HierarchyRef::Dimension { table, hier } = href else {
        return Err("DrilldownLevel expects a dimension hierarchy, not Measures".to_string());
    };
    let leaves = ctx.resolve_all_members(table, hier)?;
    let mut tuples = base.tuples;
    for leaf in leaves {
        tuples.push(Tuple {
            members: vec![leaf],
        });
    }
    Ok(EvaluatedSet {
        shape: base.shape,
        tuples,
    })
}

fn eval_drilldown_member(args: &[Option<Expr>], ctx: &EvalCtx) -> Result<EvaluatedSet, String> {
    let [Some(base_expr), Some(members_expr), Some(hier_expr)] = args else {
        return Err("DrilldownMember requires three arguments".to_string());
    };
    let base = eval_set(base_expr, ctx)?;
    let drill_targets = eval_set(members_expr, ctx)?;
    let [drill_href] = drill_targets.shape.as_slice() else {
        return Err("DrilldownMember's second argument must be a single-hierarchy set".to_string());
    };
    let HierarchyRef::Dimension {
        table: drill_table,
        hier: drill_hier,
    } = drill_href
    else {
        return Err("DrilldownMember cannot drill on the Measures hierarchy".to_string());
    };
    let drill_pos = base
        .shape
        .iter()
        .position(|h| h == drill_href)
        .ok_or_else(|| "DrilldownMember: base set does not contain the drilled hierarchy".to_string())?;

    let Expr::Member(target_path) = hier_expr else {
        return Err("DrilldownMember's third argument must be a hierarchy reference".to_string());
    };
    let (target_table, target_hier) = table_hier_of(target_path)?;
    let target_href = HierarchyRef::Dimension {
        table: target_table.clone(),
        hier: target_hier.clone(),
    };
    let target_pos = base
        .shape
        .iter()
        .position(|h| *h == target_href)
        .ok_or_else(|| "DrilldownMember: base set does not contain the target hierarchy".to_string())?;
    if target_table != *drill_table {
        return Err(
            "DrilldownMember across hierarchies on different tables is not yet supported".to_string(),
        );
    }

    let drillable_keys: std::collections::HashSet<String> = drill_targets
        .tuples
        .iter()
        .filter_map(|t| match &t.members[0] {
            Member::Leaf { key, .. } => Some(key.clone()),
            Member::All { .. } | Member::Measure { .. } => None,
        })
        .collect();

    let mut tuples = Vec::with_capacity(base.tuples.len());
    for t in base.tuples {
        let drill_key = match &t.members[drill_pos] {
            Member::Leaf { key, .. } => {
                if drillable_keys.contains(key) {
                    Some(key.clone())
                } else {
                    None
                }
            }
            Member::All { .. } | Member::Measure { .. } => None,
        };
        tuples.push(t.clone());
        if let Some(key) = drill_key {
            let filtered = ctx.resolve_all_members_filtered(&target_table, &target_hier, drill_hier, &key)?;
            for leaf in filtered {
                let mut members = t.members.clone();
                members[target_pos] = leaf;
                tuples.push(Tuple { members });
            }
        }
    }

    Ok(EvaluatedSet {
        shape: base.shape,
        tuples,
    })
}

fn eval_member_function_as_set(
    base: &Expr,
    name: &str,
    _args: &[Option<Expr>],
    ctx: &EvalCtx,
) -> Result<EvaluatedSet, String> {
    match name.to_ascii_lowercase().as_str() {
        "allmembers" => {
            let Expr::Member(path) = base else {
                return Err(format!("{name} requires a member base"));
            };
            let (table, hier) = table_hier_of(path)?;
            all_members_set(ctx, table, hier)
        }
        "members" => {
            let Expr::Member(path) = base else {
                return Err("Members requires a member base".to_string());
            };
            match classify_member_path(path)? {
                Member::All { table, hier } => Ok(EvaluatedSet {
                    shape: vec![HierarchyRef::Dimension {
                        table: table.clone(),
                        hier: hier.clone(),
                    }],
                    tuples: vec![Tuple {
                        members: vec![Member::All { table, hier }],
                    }],
                }),
                Member::Leaf { .. } => {
                    let (table, hier) = table_hier_of(path)?;
                    all_members_set(ctx, table, hier)
                }
                Member::Measure { name } => Err(format!(
                    "Members on a measure ([Measures].[{name}]) is not supported"
                )),
            }
        }
        "children" => {
            let Expr::Member(path) = base else {
                return Err("Children requires a member base".to_string());
            };
            let Member::All { table, hier } = classify_member_path(path)? else {
                return Err(
                    "Children is only supported on the [All] member (not yet supported on a specific member)"
                        .to_string(),
                );
            };
            all_members_set(ctx, table, hier)
        }
        other => Err(format!("unsupported member function in set position: .{other}")),
    }
}

fn all_members_set(ctx: &EvalCtx, table: String, hier: String) -> Result<EvaluatedSet, String> {
    let leaves = ctx.resolve_all_members(&table, &hier)?;
    let shape = vec![HierarchyRef::Dimension { table, hier }];
    let tuples = leaves
        .into_iter()
        .map(|m| Tuple { members: vec![m] })
        .collect();
    Ok(EvaluatedSet { shape, tuples })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first_with_item(mdx: &str) -> WithItem {
        let query = ast::parse(mdx).expect("should parse");
        query.with_items.into_iter().next().expect("with item")
    }

    #[test]
    fn single_quoted_numeric_body_reparses_to_number() {
        let item = first_with_item(
            "WITH MEMBER [Measures].[X] AS '1' SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        match normalized {
            WithItem::Member { expr, .. } => {
                assert!(matches!(expr, Expr::Literal(Literal::Number(n)) if n == 1.0));
            }
            other => panic!("expected WithItem::Member, got {other:?}"),
        }
    }

    #[test]
    fn single_quoted_formula_body_reparses_to_function_call() {
        let item = first_with_item(
            "WITH MEMBER [Measures].[X] AS 'Count([vtest_product].[Color].[Color])' SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        match normalized {
            WithItem::Member { expr, .. } => match expr {
                Expr::FunctionCall { name, args } => {
                    assert_eq!(name, "Count");
                    assert_eq!(args.len(), 1);
                }
                other => panic!("expected Expr::FunctionCall, got {other:?}"),
            },
            other => panic!("expected WithItem::Member, got {other:?}"),
        }
    }

    #[test]
    fn single_quoted_set_body_reparses_to_set() {
        let item = first_with_item(
            "WITH SET MySet AS '{[vtest_product].[Color].[All]}' SELECT MySet ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        match normalized {
            WithItem::Set { expr, .. } => {
                assert!(matches!(expr, Expr::Set(items) if items.len() == 1));
            }
            other => panic!("expected WithItem::Set, got {other:?}"),
        }
    }

    #[test]
    fn double_quoted_body_stays_literal() {
        let item = first_with_item(
            "WITH MEMBER [Measures].[X] AS \"1\" SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        match normalized {
            WithItem::Member { expr, .. } => {
                assert!(matches!(
                    expr,
                    Expr::Literal(Literal::String {
                        quote: QuoteStyle::Double,
                        ..
                    })
                ));
            }
            other => panic!("expected WithItem::Member, got {other:?}"),
        }
    }

    #[test]
    fn bare_body_passes_through_unchanged() {
        let item = first_with_item(
            "WITH MEMBER [Measures].[X] AS [Measures].[Amount] * 2 SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        match normalized {
            WithItem::Member { expr, .. } => {
                assert!(matches!(expr, Expr::BinaryOp { .. }));
            }
            other => panic!("expected WithItem::Member, got {other:?}"),
        }
    }

    #[test]
    fn measure_item_passes_through_unchanged() {
        let item = first_with_item(
            "WITH MEASURE 'Sales'[X] = SUM('Sales'[Amount]) SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        );
        let normalized = normalize_with_item(item).expect("should normalize");
        assert!(matches!(normalized, WithItem::Measure { .. }));
    }
}
