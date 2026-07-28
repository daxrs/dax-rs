use pest::iterators::Pair;
use pest::Parser;
use pest_derive::Parser;

use super::ast::{
    Axis, Condition, ConditionOp, ConditionValue, FromClause, MdxQuery, MemberExpr, MemberRef,
    SetExpr, SetItem, Traversal,
};

fn collect_key_refs_from_set(set: &SetExpr, out: &mut Vec<MemberRef>) {
    match set {
        SetExpr::Literal(items) => {
            for item in items {
                match item {
                    SetItem::Member(m) if m.member.key.is_some() => out.push(m.member.clone()),
                    SetItem::Member(_) => {}
                    SetItem::Set(s) => collect_key_refs_from_set(s, out),
                }
            }
        }
        SetExpr::Hierarchize(inner) | SetExpr::AddCalculatedMembers(inner) => {
            collect_key_refs_from_set(inner, out);
        }
        _ => {}
    }
}
use super::error::MdxError;

#[derive(Parser)]
#[grammar = "mdx/mdx.pest"]
pub struct MdxParser;

pub fn parse_mdx(input: &str) -> Result<MdxQuery, MdxError> {
    let (cleaned, calc_measures, named_sets) = extract_calc_measures(input);
    let mut pairs =
        MdxParser::parse(Rule::mdx_query, &cleaned).map_err(|e| MdxError::Parse(e.to_string()))?;
    let query_pair = pairs.next().expect("mdx_query: no root pair");
    build_query(query_pair, calc_measures, named_sets)
}

// ── top level ─────────────────────────────────────────────────────────────────

fn build_query(
    pair: Pair<Rule>,
    calc_measures: Vec<(String, String)>,
    named_sets: Vec<(String, SetExpr)>,
) -> Result<MdxQuery, MdxError> {
    let select_stmt = pair
        .into_inner()
        .find(|p| p.as_rule() == Rule::select_stmt)
        .expect("mdx_query: missing select_stmt");

    let body = select_stmt
        .into_inner()
        .next()
        .expect("select_stmt: missing body");

    match body.as_rule() {
        Rule::system_select => build_system(body, calc_measures, named_sets),
        Rule::cube_select => build_cube(body, calc_measures, named_sets),
        r => unreachable!("select_stmt: unexpected rule {r:?}"),
    }
}

// ── $system query ─────────────────────────────────────────────────────────────

fn build_system(
    pair: Pair<Rule>,
    calc_measures: Vec<(String, String)>,
    named_sets: Vec<(String, SetExpr)>,
) -> Result<MdxQuery, MdxError> {
    let mut columns = Vec::new();
    let mut table = String::new();
    let mut conditions = Vec::new();

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::col_list => {
                columns = child
                    .into_inner()
                    .filter(|p| p.as_rule() == Rule::bracketed_ident)
                    .map(|p| strip_brackets(p.as_str()))
                    .collect();
            }
            Rule::system_from => {
                table = child.as_str()["$system.".len()..].to_uppercase();
            }
            Rule::cond_list => {
                conditions = child
                    .into_inner()
                    .filter(|p| p.as_rule() == Rule::condition)
                    .map(build_condition)
                    .collect();
            }
            r => unreachable!("system_select: unexpected rule {r:?}"),
        }
    }

    Ok(MdxQuery {
        calc_measures,
        named_sets,
        from: FromClause::System { table, columns, conditions },
        axes: vec![],
        slicer: vec![],
        cell_props: vec![],
    })
}

fn build_condition(pair: Pair<Rule>) -> Condition {
    let inner = pair.into_inner().next().expect("condition: missing inner");
    match inner.as_rule() {
        Rule::cond_eq => {
            let mut parts = inner.into_inner();
            let column = strip_brackets(parts.next().expect("cond_eq: missing column").as_str());
            let value = build_cond_value(parts.next().expect("cond_eq: missing value"));
            Condition { column, op: ConditionOp::Eq, value }
        }
        Rule::cond_ne => {
            let mut parts = inner.into_inner();
            let column = strip_brackets(parts.next().expect("cond_ne: missing column").as_str());
            let value = build_cond_value(parts.next().expect("cond_ne: missing value"));
            Condition { column, op: ConditionOp::Ne, value }
        }
        Rule::cond_bool => {
            let column = strip_brackets(
                inner
                    .into_inner()
                    .next()
                    .expect("cond_bool: missing column")
                    .as_str(),
            );
            Condition {
                column,
                op: ConditionOp::IsTrue,
                value: ConditionValue::Literal(String::new()),
            }
        }
        r => unreachable!("condition: unexpected rule {r:?}"),
    }
}

fn build_cond_value(pair: Pair<Rule>) -> ConditionValue {
    let inner = pair.into_inner().next().expect("cond_value: missing inner");
    match inner.as_rule() {
        Rule::param_ref => {
            let name = inner
                .into_inner()
                .next()
                .expect("param_ref: missing name")
                .as_str()
                .to_string();
            ConditionValue::Param(name)
        }
        Rule::string_literal => {
            let s = inner.as_str();
            ConditionValue::Literal(s[1..s.len() - 1].to_string())
        }
        Rule::integer_literal => ConditionValue::Literal(inner.as_str().to_string()),
        r => unreachable!("cond_value: unexpected rule {r:?}"),
    }
}

// ── cube query ────────────────────────────────────────────────────────────────

fn build_cube(
    pair: Pair<Rule>,
    calc_measures: Vec<(String, String)>,
    named_sets: Vec<(String, SetExpr)>,
) -> Result<MdxQuery, MdxError> {
    let mut axes = Vec::new();
    let mut from_clause = FromClause::Cube(String::new());
    let mut slicer = Vec::new();
    let mut cell_props = Vec::new();

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::axis_clause => axes.push(build_axis(child)),
            Rule::cube_from => {
                from_clause = build_cube_from(child);
            }
            Rule::slicer => slicer = build_slicer(child),
            Rule::prop_list => cell_props = build_prop_list(child),
            r => unreachable!("cube_select: unexpected rule {r:?}"),
        }
    }

    Ok(MdxQuery {
        calc_measures,
        named_sets,
        from: from_clause,
        axes,
        slicer,
        cell_props,
    })
}

fn build_cube_from(pair: Pair<Rule>) -> FromClause {
    let inner = pair.into_inner().next().expect("cube_from: missing inner");
    match inner.as_rule() {
        Rule::bracketed_ident => FromClause::Cube(strip_brackets(inner.as_str())),
        Rule::subquery => build_subquery(inner),
        r => unreachable!("cube_from: unexpected rule {r:?}"),
    }
}

fn build_subquery(pair: Pair<Rule>) -> FromClause {
    let mut key_members = Vec::new();
    let mut cube = String::new();

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::subq_axis => {
                let set = child
                    .into_inner()
                    .find(|p| p.as_rule() == Rule::set_expr)
                    .map(build_set_expr);
                if let Some(set_expr) = set {
                    collect_key_refs_from_set(&set_expr, &mut key_members);
                }
            }
            Rule::bracketed_ident => {
                cube = strip_brackets(child.as_str());
            }
            _ => {}
        }
    }

    FromClause::SubqueryCube { cube, key_members }
}

fn build_axis(pair: Pair<Rule>) -> Axis {
    let mut non_empty = false;
    let mut set = None;
    let mut dim_props = Vec::new();
    let mut id = 0u32;

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::kw_non_empty => non_empty = true,
            Rule::set_expr => set = Some(build_set_expr(child)),
            Rule::prop_list => dim_props = build_prop_list(child),
            Rule::axis_id => id = build_axis_id(child),
            r => unreachable!("axis_clause: unexpected rule {r:?}"),
        }
    }

    Axis {
        id,
        non_empty,
        set: set.expect("axis_clause: missing set_expr"),
        dim_props,
    }
}

fn build_axis_id(pair: Pair<Rule>) -> u32 {
    let inner = pair.into_inner().next().expect("axis_id: missing inner");
    match inner.as_rule() {
        Rule::kw_columns => 0,
        Rule::kw_rows => 1,
        Rule::integer_literal => inner.as_str().parse().unwrap_or(0),
        r => unreachable!("axis_id: unexpected rule {r:?}"),
    }
}

// ── set expressions ───────────────────────────────────────────────────────────

fn build_set_expr(pair: Pair<Rule>) -> SetExpr {
    let inner = pair.into_inner().next().expect("set_expr: missing inner");
    match inner.as_rule() {
        Rule::func_hierarchize => {
            let child = inner
                .into_inner()
                .next()
                .expect("func_hierarchize: missing set_expr");
            SetExpr::Hierarchize(Box::new(build_set_expr(child)))
        }
        Rule::func_add_calc => {
            let child = inner
                .into_inner()
                .next()
                .expect("func_add_calc: missing set_expr");
            SetExpr::AddCalculatedMembers(Box::new(build_set_expr(child)))
        }
        Rule::func_drilldown_level => {
            // Only the first argument (the set) is relevant; extra args are navigation hints.
            let child = inner
                .into_inner()
                .find(|p| p.as_rule() == Rule::set_expr)
                .expect("func_drilldown_level: missing set_expr");
            SetExpr::DrilldownLevel(Box::new(build_set_expr(child)))
        }
        Rule::func_crossjoin => {
            let mut it = inner.into_inner().filter(|p| p.as_rule() == Rule::set_expr);
            let left = it.next().expect("func_crossjoin: missing first set");
            let right = it.next().expect("func_crossjoin: missing second set");
            SetExpr::CrossJoin(
                Box::new(build_set_expr(left)),
                Box::new(build_set_expr(right)),
            )
        }
        Rule::func_drilldown_member => {
            let mut set_exprs: Vec<SetExpr> = Vec::new();
            let mut hier: Option<Vec<String>> = None;
            for child in inner.into_inner() {
                match child.as_rule() {
                    Rule::set_expr => set_exprs.push(build_set_expr(child)),
                    Rule::drilldown_member_arg if hier.is_none() => {
                        let arg = child
                            .into_inner()
                            .next()
                            .expect("drilldown_member_arg: missing inner");
                        hier = match arg.as_rule() {
                            Rule::member_expr => Some(build_member_expr(arg).member.parts),
                            Rule::plain_ident => Some(vec![arg.as_str().to_string()]),
                            _ => None,
                        };
                    }
                    _ => {}
                }
            }
            let mut it = set_exprs.into_iter();
            let base = it.next().expect("func_drilldown_member: missing base set");
            let members = it
                .next()
                .expect("func_drilldown_member: missing members set");
            SetExpr::DrilldownMember { base: Box::new(base), members: Box::new(members), hier }
        }
        Rule::func_generate => {
            let mut it = inner.into_inner().filter(|p| p.as_rule() == Rule::set_expr);
            let set = it.next().expect("func_generate: missing first set");
            let body = it.next().expect("func_generate: missing second set");
            SetExpr::Generate(
                Box::new(build_set_expr(set)),
                Box::new(build_set_expr(body)),
            )
        }
        Rule::func_ascendants => {
            let member = inner
                .into_inner()
                .find(|p| p.as_rule() == Rule::member_expr)
                .expect("func_ascendants: missing member_expr");
            SetExpr::Ascendants(Box::new(build_member_expr(member)))
        }
        Rule::set_literal => {
            let items = inner
                .into_inner() // set_items
                .next()
                .expect("set_literal: missing set_items")
                .into_inner() // set_item*
                .map(build_set_item)
                .collect();
            SetExpr::Literal(items)
        }
        // A bare member expression used directly as a one-element set (e.g. as a DrilldownMember arg).
        Rule::member_expr => SetExpr::Literal(vec![SetItem::Member(build_member_expr(inner))]),
        Rule::named_set_ref => SetExpr::NamedSetRef(inner.as_str().to_string()),
        Rule::paren_set => {
            let child = inner
                .into_inner()
                .next()
                .expect("paren_set: missing set_expr");
            build_set_expr(child)
        }
        r => unreachable!("set_expr: unexpected rule {r:?}"),
    }
}

fn build_set_item(pair: Pair<Rule>) -> SetItem {
    let inner = pair.into_inner().next().expect("set_item: missing inner");
    match inner.as_rule() {
        Rule::set_expr => SetItem::Set(build_set_expr(inner)),
        Rule::member_expr => SetItem::Member(build_member_expr(inner)),
        Rule::set_item_tuple => {
            let m = inner
                .into_inner()
                .next()
                .expect("set_item_tuple: missing member_expr");
            SetItem::Member(build_member_expr(m))
        }
        r => unreachable!("set_item: unexpected rule {r:?}"),
    }
}

// ── member references ─────────────────────────────────────────────────────────

fn build_member_expr(pair: Pair<Rule>) -> MemberExpr {
    let mut inner = pair.into_inner();
    let member = build_member_ref(inner.next().expect("member_expr: missing member_ref"));
    let traversal = inner.next().map(build_traversal);
    MemberExpr { member, traversal }
}

fn build_traversal(pair: Pair<Rule>) -> Traversal {
    let kw = pair
        .into_inner()
        .next()
        .expect("traversal: missing keyword");
    match kw.as_rule() {
        Rule::kw_all_members => Traversal::AllMembers,
        Rule::kw_members => Traversal::Members,
        Rule::kw_children => Traversal::Children,
        Rule::kw_current_member => Traversal::CurrentMember,
        r => unreachable!("traversal: unexpected rule {r:?}"),
    }
}

fn build_member_ref(pair: Pair<Rule>) -> MemberRef {
    let mut parts = Vec::new();
    let mut key = None;

    for child in pair.into_inner() {
        match child.as_rule() {
            Rule::bracketed_ident => parts.push(strip_brackets(child.as_str())),
            Rule::member_ref_tail => {
                let inner = child
                    .into_inner()
                    .next()
                    .expect("member_ref_tail: missing inner");
                match inner.as_rule() {
                    Rule::bracketed_ident => parts.push(strip_brackets(inner.as_str())),
                    Rule::member_unbracketed_name => parts.push(inner.as_str().to_string()),
                    Rule::key_ref => {
                        let bi = inner.into_inner().next().expect("key_ref: missing ident");
                        key = Some(strip_brackets(bi.as_str()));
                    }
                    r => unreachable!("member_ref_tail: unexpected rule {r:?}"),
                }
            }
            r => unreachable!("member_ref: unexpected rule {r:?}"),
        }
    }

    MemberRef { parts, key }
}

// ── slicer / prop list ────────────────────────────────────────────────────────

fn build_slicer(pair: Pair<Rule>) -> Vec<MemberExpr> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::member_expr)
        .map(build_member_expr)
        .collect()
}

fn build_prop_list(pair: Pair<Rule>) -> Vec<String> {
    pair.into_inner()
        .filter(|p| p.as_rule() == Rule::prop_name)
        .map(|p| p.as_str().to_string())
        .collect()
}

// ── WITH MEASURE / WITH MEMBER preprocessing ─────────────────────────────────

/// Strips `WITH MEASURE`/`WITH MEMBER`/`WITH Set` clauses that precede the SELECT statement.
/// Returns `(cleaned_select_stmt, calc_measures, named_sets)`.
#[allow(clippy::type_complexity)]
pub(crate) fn extract_calc_measures(
    stmt: &str,
) -> (String, Vec<(String, String)>, Vec<(String, SetExpr)>) {
    let trimmed = stmt.trim_start();
    if !trimmed
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("WITH"))
    {
        return (trimmed.to_string(), vec![], vec![]);
    }
    let select_pos = match find_kw(trimmed, "SELECT") {
        Some(p) => p,
        None => return (trimmed.to_string(), vec![], vec![]),
    };
    let with_section = &trimmed[..select_pos];
    let select_stmt = &trimmed[select_pos..];
    let mut measures = Vec::new();
    let mut named_sets = Vec::new();
    parse_with_section(with_section, &mut measures, &mut named_sets);
    (select_stmt.to_string(), measures, named_sets)
}

fn parse_with_section(
    section: &str,
    measures: &mut Vec<(String, String)>,
    named_sets: &mut Vec<(String, SetExpr)>,
) {
    let mut s = section.trim_start();

    if !s.get(..4).is_some_and(|w| w.eq_ignore_ascii_case("WITH")) {
        return;
    }
    s = s[4..].trim_start();

    loop {
        let is_measure = s
            .get(..7)
            .is_some_and(|w| w.eq_ignore_ascii_case("MEASURE"));
        let is_member = !is_measure && s.get(..6).is_some_and(|w| w.eq_ignore_ascii_case("MEMBER"));
        let is_set =
            !is_measure && !is_member && s.get(..3).is_some_and(|w| w.eq_ignore_ascii_case("SET"));

        if is_set {
            s = s[3..].trim_start();
            // Parse: <ident> AS <set_expr>
            let name_end = s
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(s.len());
            if name_end == 0 {
                break;
            }
            let name = s[..name_end].to_lowercase();
            s = s[name_end..].trim_start();
            // Expect AS keyword with word boundary
            if !s.get(..2).is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
                break;
            }
            let after_as = s.as_bytes().get(2).copied().unwrap_or(b' ');
            if after_as.is_ascii_alphanumeric() || after_as == b'_' {
                break;
            }
            s = s[2..].trim_start();
            // Expression may be wrapped in single quotes: As '{...}'
            let expr_raw = if s.starts_with('\'') {
                let close = s[1..].find('\'').map(|p| p + 1).unwrap_or(s.len() - 1);
                let raw = s[1..close].to_string();
                s = s[close + 1..].trim_start();
                raw
            } else {
                let expr_end = find_kw(s, "MEASURE")
                    .or_else(|| find_kw(s, "MEMBER"))
                    .or_else(|| find_kw(s, "SET"))
                    .unwrap_or(s.len());
                let raw = s[..expr_end].trim().to_string();
                s = s[expr_end..].trim_start();
                raw
            };
            if let Some(set_expr) = parse_set_str(&expr_raw) {
                named_sets.push((name, set_expr));
            }
            continue;
        }

        if !is_measure && !is_member {
            break;
        }
        let kw_len = if is_measure { 7 } else { 6 };
        s = s[kw_len..].trim_start();

        let name: String = if is_measure {
            // MEASURE 'table'[name] or [name]
            if s.starts_with('\'') {
                let close = match s[1..].find('\'') {
                    Some(p) => p + 1,
                    None => break,
                };
                s = s[close + 1..].trim_start();
            }
            if !s.starts_with('[') {
                break;
            }
            let close = match s[1..].find(']') {
                Some(p) => p + 1,
                None => break,
            };
            let name = s[1..close].to_lowercase();
            s = s[close + 1..].trim_start();
            name
        } else {
            // MEMBER [Measures].[name] or [Measures].name — take the last segment.
            let mut name = String::new();
            loop {
                if s.starts_with('[') {
                    let close = match s[1..].find(']') {
                        Some(p) => p + 1,
                        None => break,
                    };
                    name = s[1..close].to_lowercase();
                    s = &s[close + 1..];
                    if s.starts_with('.') {
                        s = &s[1..];
                    } else {
                        break;
                    }
                } else {
                    // Unbracketed final segment (e.g. cChildren in MEMBER [Measures].cChildren)
                    let name_end = s
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(s.len());
                    if name_end == 0 {
                        break;
                    }
                    name = s[..name_end].to_lowercase();
                    s = &s[name_end..];
                    break;
                }
            }
            s = s.trim_start();
            name
        };

        if name.is_empty() {
            break;
        }

        // Accept '=' (DAX MEASURE style) or 'AS' (MDX MEMBER style) as separator.
        if s.starts_with('=') {
            s = s[1..].trim_start();
        } else if s.get(..2).is_some_and(|w| w.eq_ignore_ascii_case("AS")) {
            let after_as = s.as_bytes().get(2).copied().unwrap_or(b' ');
            if after_as.is_ascii_alphanumeric() || after_as == b'_' {
                break;
            }
            s = s[2..].trim_start();
        } else {
            break;
        }

        // Expression runs until the next MEASURE/MEMBER/SET keyword (or end of section).
        let expr_end = find_kw(s, "MEASURE")
            .or_else(|| find_kw(s, "MEMBER"))
            .or_else(|| find_kw(s, "SET"))
            .unwrap_or(s.len());
        let expr = s[..expr_end].trim();
        // Strip surrounding single quotes (MDX 'As expr' form wraps expression in quotes).
        let expr = if expr.starts_with('\'') && expr.ends_with('\'') && expr.len() > 2 {
            &expr[1..expr.len() - 1]
        } else {
            expr
        };
        let final_expr = try_translate_cchildren(expr).unwrap_or_else(|| expr.to_string());
        measures.push((name, final_expr));
        s = s[expr_end..].trim_start();
    }
}

/// Detect `AddCalculatedMembers([T].[H].currentmember.children).count` and
/// return the equivalent DAX expression, or `None` if the pattern doesn't match.
fn try_translate_cchildren(expr: &str) -> Option<String> {
    let s = expr.trim();

    let prefix = "AddCalculatedMembers(";
    if s.len() < prefix.len() || !s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        return None;
    }

    let suffix = ").count";
    if s.len() < prefix.len() + suffix.len()
        || !s[s.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    {
        return None;
    }

    let inner = s[prefix.len()..s.len() - suffix.len()].trim();

    // Parse [T]
    if !inner.starts_with('[') {
        return None;
    }
    let t_close = inner[1..].find(']')? + 1;
    let table = &inner[1..t_close];
    let rest = &inner[t_close + 1..];

    // Then .[H]
    let rest = rest.strip_prefix('.')?;
    if !rest.starts_with('[') {
        return None;
    }
    let h_close = rest[1..].find(']')? + 1;
    let hier = &rest[1..h_close];
    let rest = &rest[h_close + 1..];

    // Then .currentmember.children (case-insensitive, nothing else)
    if !rest.eq_ignore_ascii_case(".currentmember.children") {
        return None;
    }

    Some(format!(
        "IF(ISINSCOPE('{table}'[{hier}]), 0, COUNTROWS(VALUES('{table}'[{hier}])))"
    ))
}

/// Parse a raw set expression string using the grammar, returning the AST node.
fn parse_set_str(s: &str) -> Option<SetExpr> {
    let s = s.trim();
    let mut pairs = MdxParser::parse(Rule::set_expr, s).ok()?;
    let pair = pairs.next()?;
    if pair.as_str().len() != s.len() {
        return None;
    }
    Some(build_set_expr(pair))
}

/// Returns the byte offset of `kw` in `s` where it appears at a word boundary
/// (not preceded/followed by an alphanumeric character or `_`).
fn find_kw(s: &str, kw: &str) -> Option<usize> {
    let kw_len = kw.len();
    for (i, _) in s.char_indices() {
        let Some(slice) = s.get(i..i + kw_len) else {
            continue;
        };
        if slice.eq_ignore_ascii_case(kw) {
            let before_ok = i == 0
                || s.as_bytes()
                    .get(i - 1)
                    .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');
            let after_ok = s
                .as_bytes()
                .get(i + kw_len)
                .is_none_or(|&b| !b.is_ascii_alphanumeric() && b != b'_');
            if before_ok && after_ok {
                return Some(i);
            }
        }
    }
    None
}

// ── helpers ───────────────────────────────────────────────────────────────────

fn strip_brackets(s: &str) -> String {
    s.trim_start_matches('[').trim_end_matches(']').to_string()
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::mdx::ast::{ConditionOp, ConditionValue, FromClause, Traversal};

    // ── $system queries ───────────────────────────────────────────────────────

    #[test]
    fn system_single_col_integer_where() {
        let q = parse_mdx("SELECT [CUBE_NAME] FROM $system.MDSCHEMA_CUBES WHERE [CUBE_SOURCE] = 1")
            .unwrap();
        let FromClause::System { table, columns, conditions } = q.from else {
            panic!()
        };
        assert_eq!(table, "MDSCHEMA_CUBES");
        assert_eq!(columns, ["CUBE_NAME"]);
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].column, "CUBE_SOURCE");
        assert_eq!(conditions[0].op, ConditionOp::Eq);
        assert!(matches!(&conditions[0].value, ConditionValue::Literal(v) if v == "1"));
    }

    #[test]
    fn system_multi_col_no_where() {
        let q = parse_mdx(
            "SELECT [CATALOG_NAME], [DESCRIPTION], [DATE_MODIFIED], [COMPATIBILITY_LEVEL] \
             FROM $system.DBSCHEMA_CATALOGS",
        )
        .unwrap();
        let FromClause::System { table, columns, conditions } = q.from else {
            panic!()
        };
        assert_eq!(table, "DBSCHEMA_CATALOGS");
        assert_eq!(
            columns,
            [
                "CATALOG_NAME",
                "DESCRIPTION",
                "DATE_MODIFIED",
                "COMPATIBILITY_LEVEL"
            ]
        );
        assert!(conditions.is_empty());
    }

    #[test]
    fn system_param_and_integer_where() {
        let q = parse_mdx(
            "SELECT [CUBE_NAME], [LAST_DATA_UPDATE], [DESCRIPTION], [BASE_CUBE_NAME] \
             FROM $system.MDSCHEMA_CUBES \
             WHERE [CATALOG_NAME] = @CatalogName AND [CUBE_SOURCE] = 1",
        )
        .unwrap();
        let FromClause::System { conditions, .. } = q.from else {
            panic!()
        };
        assert_eq!(conditions.len(), 2);
        assert!(matches!(&conditions[0].value, ConditionValue::Param(n) if n == "CatalogName"));
        assert_eq!(conditions[1].op, ConditionOp::Eq);
    }

    #[test]
    fn system_ne_and_bool_conditions() {
        let q = parse_mdx(
            "SELECT [DIMENSION_CAPTION] FROM $system.mdschema_dimensions \
             WHERE [CUBE_NAME] = @CubeName \
             AND [DIMENSION_UNIQUE_NAME] <> '[Measures]' \
             AND [DIMENSION_IS_VISIBLE]",
        )
        .unwrap();
        let FromClause::System { conditions, .. } = q.from else {
            panic!()
        };
        assert_eq!(conditions.len(), 3);
        assert_eq!(conditions[1].op, ConditionOp::Ne);
        assert!(matches!(&conditions[1].value, ConditionValue::Literal(v) if v == "[Measures]"));
        assert_eq!(conditions[2].op, ConditionOp::IsTrue);
        assert_eq!(conditions[2].column, "DIMENSION_IS_VISIBLE");
    }

    #[test]
    fn system_bool_where_only() {
        let q = parse_mdx(
            "SELECT [MEASURE_CAPTION] FROM $system.mdschema_measures \
             WHERE [CUBE_NAME] = @CubeName AND [MEASURE_IS_VISIBLE]",
        )
        .unwrap();
        let FromClause::System { conditions, .. } = q.from else {
            panic!()
        };
        assert_eq!(conditions[1].op, ConditionOp::IsTrue);
    }

    // ── cube queries ──────────────────────────────────────────────────────────

    #[test]
    fn cube_non_empty_hierarchize_measure_slicer() {
        let q = parse_mdx(
            "SELECT NON EMPTY \
             { {[vtest_product].[Color].[All]} , \
               {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} } \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        assert!(matches!(q.from, FromClause::Cube(ref s) if s == "Model"));
        assert_eq!(q.axes.len(), 1);
        assert!(q.axes[0].non_empty);
        assert_eq!(q.axes[0].id, 0);
        assert_eq!(
            q.axes[0].dim_props,
            ["PARENT_UNIQUE_NAME", "HIERARCHY_UNIQUE_NAME"]
        );
        assert_eq!(q.slicer.len(), 1);
        assert!(q.slicer[0].member.is_measure());
        assert_eq!(
            q.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
    }

    #[test]
    fn cube_add_calculated_members() {
        let q = parse_mdx(
            "SELECT {AddCalculatedMembers({[vtest_product].[ProductType].[(All)].Members})} \
             DIMENSION PROPERTIES MEMBER_TYPE ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES CELL_ORDINAL",
        )
        .unwrap();
        assert_eq!(q.axes.len(), 1);
        assert!(!q.axes[0].non_empty);
        assert!(matches!(
            q.axes[0].set,
            crate::mdx::ast::SetExpr::Literal(_)
        ));
        assert!(q.slicer.is_empty());
        assert_eq!(q.cell_props, ["CELL_ORDINAL"]);
    }

    #[test]
    fn cube_key_filter_slicer() {
        let q = parse_mdx(
            "SELECT NON EMPTY \
             {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([vtest_product].[ProductType].&[Widget],[Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        assert_eq!(q.slicer.len(), 2);
        assert_eq!(q.slicer[0].member.key.as_deref(), Some("Widget"));
        assert!(q.slicer[1].member.is_measure());
    }

    #[test]
    fn cube_dimension_slicer_plus_measure() {
        let q = parse_mdx(
            "SELECT NON EMPTY \
             {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([vtest_product].[ProductType].[All],[Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        assert_eq!(q.slicer.len(), 2);
        assert!(!q.slicer[0].member.is_measure());
        assert!(q.slicer[1].member.is_measure());
    }

    #[test]
    fn cube_drilldown_level_all_member() {
        let q = parse_mdx(
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        assert!(matches!(q.from, FromClause::Cube(ref s) if s == "Model"));
        assert_eq!(q.axes.len(), 1);
        assert!(q.axes[0].non_empty);
        assert_eq!(q.axes[0].id, 0);
        assert_eq!(
            q.axes[0].dim_props,
            ["PARENT_UNIQUE_NAME", "HIERARCHY_UNIQUE_NAME"]
        );
        assert_eq!(
            q.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
        // Outer Hierarchize wraps a set literal containing a DrilldownLevel
        let SetExpr::Hierarchize(inner) = &q.axes[0].set else {
            panic!("expected Hierarchize")
        };
        let SetExpr::Literal(items) = inner.as_ref() else {
            panic!("expected Literal")
        };
        assert_eq!(items.len(), 1);
        let SetItem::Set(inner_set) = &items[0] else {
            panic!("expected Set item")
        };
        assert!(matches!(inner_set, SetExpr::DrilldownLevel(_)));
    }

    #[test]
    fn with_measure_stripped_and_captured() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[My Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY {Hierarchize({[Sales].[Color].[Color].AllMembers})}\n",
            "  DIMENSION PROPERTIES PARENT_UNIQUE_NAME ON COLUMNS\n",
            "FROM [Model]\n",
            "WHERE ([Measures].[My Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING",
        ))
        .unwrap();
        assert_eq!(q.calc_measures.len(), 1);
        assert_eq!(q.calc_measures[0].0, "my amount");
        assert_eq!(q.calc_measures[0].1, "SUM('Sales'[Amount])");
        assert!(matches!(q.from, FromClause::Cube(ref s) if s == "Model"));
        assert_eq!(q.slicer.len(), 1);
        assert!(q.slicer[0].member.is_measure());
        assert_eq!(
            q.slicer[0].member.parts.last().map(String::as_str),
            Some("My Amount")
        );
    }

    #[test]
    fn with_measure_dax_substituted_in_translation() {
        use crate::mdx::translator::mdx_to_dax;
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[My Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY {Hierarchize({[Sales].[Color].[Color].AllMembers})}\n",
            "  DIMENSION PROPERTIES PARENT_UNIQUE_NAME ON COLUMNS\n",
            "FROM [Model]\n",
            "WHERE ([Measures].[My Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        use crate::mdx::QueryShape;
        let QueryShape::SingleDim { ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(measure_name.as_deref(), Some("My Amount"));
        assert_eq!(
            t.cell_dax.as_deref(),
            Some(r#"EVALUATE SUMMARIZECOLUMNS('Sales'[Color], "Value", SUM('Sales'[Amount]))"#)
        );
        assert_eq!(
            t.total_dax.as_deref(),
            Some("EVALUATE { CALCULATE(SUM('Sales'[Amount])) }")
        );
    }

    #[test]
    fn cube_block_comments_stripped() {
        // Simplified version of the GTOPT comment pattern PowerBI emits.
        let q = parse_mdx(
            "SELECT NON EMPTY \
             { /* section 1 */ {[vtest_product].[Color].[All]} /* end */ , \
               {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} } \
             ON COLUMNS FROM [Model]",
        )
        .unwrap();
        assert_eq!(q.axes.len(), 1);
        assert!(q.axes[0].non_empty);
    }

    // ── Phase-1 new-function tests ────────────────────────────────────────────

    #[test]
    fn translate_cchildren_measure_expression() {
        let q = parse_mdx(concat!(
            "WITH MEMBER [Measures].cChildren As ",
            "'AddCalculatedMembers([Product].[Color].currentmember.children).count' ",
            "SELECT {[Measures].cChildren} ON COLUMNS FROM [Model]",
        ))
        .unwrap();
        assert_eq!(q.calc_measures.len(), 1);
        assert_eq!(q.calc_measures[0].0, "cchildren");
        assert_eq!(
            q.calc_measures[0].1,
            "IF(ISINSCOPE('Product'[Color]), 0, COUNTROWS(VALUES('Product'[Color])))"
        );
    }

    #[test]
    fn with_named_set_parsed() {
        let q = parse_mdx(concat!(
            "WITH MEMBER [Measures].cChildren As ",
            "'AddCalculatedMembers([Product].[Color].currentmember.children).count' ",
            "Set FilteredMembers As '{[Product].[Color].&[Blue]}' ",
            "Select {[Measures].cChildren} on ROWS, ",
            "Hierarchize(Generate(FilteredMembers, Ascendants([Product].[Color].currentmember))) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME, MEMBER_TYPE ON COLUMNS FROM [Model]",
        ))
        .unwrap();

        // Calc measure translated to DAX (cChildren pattern detected at parse time)
        assert_eq!(q.calc_measures.len(), 1);
        assert_eq!(q.calc_measures[0].0, "cchildren");
        assert_eq!(
            q.calc_measures[0].1,
            "IF(ISINSCOPE('Product'[Color]), 0, COUNTROWS(VALUES('Product'[Color])))"
        );

        // Named set captured and parsed
        assert_eq!(q.named_sets.len(), 1);
        assert_eq!(q.named_sets[0].0, "filteredmembers");
        // {[Product].[Color].&[Blue]} → Literal([Set(Literal([Member(...)]))])
        // The set_literal wrapper creates one outer level, and set_item→set_expr adds one more.
        let SetExpr::Literal(outer) = &q.named_sets[0].1 else {
            panic!("expected Literal")
        };
        assert_eq!(outer.len(), 1);
        let SetItem::Set(inner_set) = &outer[0] else {
            panic!("expected Set item")
        };
        let SetExpr::Literal(inner) = inner_set else {
            panic!("expected inner Literal")
        };
        let SetItem::Member(m) = &inner[0] else {
            panic!("expected Member")
        };
        assert_eq!(m.member.parts, ["Product", "Color"]);
        assert_eq!(m.member.key.as_deref(), Some("Blue"));

        // Two axes parsed: ROWS (id=1) and COLUMNS (id=0)
        assert_eq!(q.axes.len(), 2);
        assert_eq!(q.axes[0].id, 1); // ROWS
        assert_eq!(q.axes[1].id, 0); // COLUMNS
        assert_eq!(q.axes[1].dim_props, ["PARENT_UNIQUE_NAME", "MEMBER_TYPE"]);

        // COLUMNS axis is Hierarchize(Generate(NamedSetRef, Ascendants(...)))
        let SetExpr::Hierarchize(gen) = &q.axes[1].set else {
            panic!("expected Hierarchize")
        };
        let SetExpr::Generate(set_ref, body) = gen.as_ref() else {
            panic!("expected Generate")
        };
        assert!(matches!(set_ref.as_ref(), SetExpr::NamedSetRef(n) if n == "FilteredMembers"));
        let SetExpr::Ascendants(asc_m) = body.as_ref() else {
            panic!("expected Ascendants")
        };
        assert_eq!(asc_m.member.parts, ["Product", "Color"]);
        assert_eq!(asc_m.traversal, Some(Traversal::CurrentMember));
    }

    #[test]
    fn func_generate_parsed() {
        // Bare Generate without {..} wrapper: set_expr is directly Generate.
        let q = parse_mdx(concat!(
            "SELECT Hierarchize(Generate(",
            "{[Product].[Color].&[Blue]}, ",
            "Ascendants([Product].[Color].currentmember)",
            ")) ON COLUMNS FROM [Model]",
        ))
        .unwrap();
        let SetExpr::Hierarchize(inner) = &q.axes[0].set else {
            panic!("expected Hierarchize")
        };
        let SetExpr::Generate(set, body) = inner.as_ref() else {
            panic!("expected Generate")
        };
        // First arg is a set_literal (Literal wrapping a Set(Literal(Member)))
        assert!(matches!(set.as_ref(), SetExpr::Literal(_)));
        // Second arg is Ascendants wrapping a member with CurrentMember traversal
        let SetExpr::Ascendants(m) = body.as_ref() else {
            panic!("expected Ascendants")
        };
        assert_eq!(m.member.parts, ["Product", "Color"]);
        assert_eq!(m.traversal, Some(Traversal::CurrentMember));
    }

    #[test]
    fn func_ascendants_parsed() {
        // Bare Ascendants as the axis set_expr (no enclosing {..}).
        let q =
            parse_mdx("SELECT Ascendants([Product].[Color].currentmember) ON COLUMNS FROM [Model]")
                .unwrap();
        let SetExpr::Ascendants(m) = &q.axes[0].set else {
            panic!("expected Ascendants")
        };
        assert_eq!(m.member.parts, ["Product", "Color"]);
        assert_eq!(m.traversal, Some(Traversal::CurrentMember));
    }

    #[test]
    fn current_member_traversal() {
        // Bare member expression as the axis set_expr (no enclosing {..}) gives a flat Literal.
        let q =
            parse_mdx("SELECT [Product].[Color].currentmember ON COLUMNS FROM [Model]").unwrap();
        let SetExpr::Literal(items) = &q.axes[0].set else {
            panic!("expected Literal")
        };
        let SetItem::Member(m) = &items[0] else {
            panic!("expected Member")
        };
        assert_eq!(m.member.parts, ["Product", "Color"]);
        assert_eq!(m.traversal, Some(Traversal::CurrentMember));
    }

    // ── Subquery FROM parsing — stage 2a ─────────────────────────────────────

    #[test]
    fn subquery_from_two_dim_axis_parsed() {
        // Two-axis query: ProductType ON COLUMNS, Color ON ROWS, subquery FROM with Red filter.
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS,\n",
            "NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS\n",
            "FROM (SELECT ({[Product].[Color].&[Red]}) ON COLUMNS FROM [Model])\n",
            "WHERE ([Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();

        let FromClause::SubqueryCube { cube, key_members } = &q.from else {
            panic!("expected SubqueryCube, got {:?}", q.from);
        };
        assert_eq!(cube, "Model");
        assert_eq!(key_members.len(), 1);
        assert_eq!(key_members[0].parts, ["Product", "Color"]);
        assert_eq!(key_members[0].key.as_deref(), Some("Red"));

        assert_eq!(q.axes.len(), 2);
        assert_eq!(q.axes[0].id, 0); // COLUMNS = ProductType
        assert_eq!(q.axes[1].id, 1); // ROWS = Color
        assert!(q.slicer.len() == 1 && q.slicer[0].member.is_measure());
    }

    #[test]
    fn subquery_from_two_members_parsed_as_subquery_cube() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS\n",
            "FROM (SELECT ({[Product].[Color].&[Red], [Product].[Color].&[Blue]}) ON COLUMNS FROM [Model])\n",
            "WHERE ([Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();

        let FromClause::SubqueryCube { cube, key_members } = &q.from else {
            panic!("expected SubqueryCube, got {:?}", q.from);
        };
        assert_eq!(cube, "Model");
        assert_eq!(key_members.len(), 2);
        assert_eq!(key_members[0].parts, ["Product", "Color"]);
        assert_eq!(key_members[0].key.as_deref(), Some("Red"));
        assert_eq!(key_members[1].parts, ["Product", "Color"]);
        assert_eq!(key_members[1].key.as_deref(), Some("Blue"));
    }

    #[test]
    fn subquery_from_parsed_as_subquery_cube() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS\n",
            "FROM (SELECT ({[Product].[Color].&[Red]}) ON COLUMNS FROM [Model])\n",
            "WHERE ([Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();

        let FromClause::SubqueryCube { cube, key_members } = &q.from else {
            panic!("expected SubqueryCube, got {:?}", q.from);
        };
        assert_eq!(cube, "Model");
        assert_eq!(key_members.len(), 1);
        assert_eq!(key_members[0].parts, ["Product", "Color"]);
        assert_eq!(key_members[0].key.as_deref(), Some("Red"));

        assert_eq!(q.calc_measures.len(), 1);
        assert_eq!(q.calc_measures[0].0, "cimsummen af amount");
        assert_eq!(q.slicer.len(), 1);
        assert!(q.slicer[0].member.is_measure());
        assert_eq!(
            q.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
    }
}
