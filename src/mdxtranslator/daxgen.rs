use super::ast::Expr;
use super::eval::{table_hier_of, EvalCtx, EvaluatedSet, HierarchyRef, Member};
use std::collections::HashSet;

pub fn generate_dax(
    set: Option<&EvaluatedSet>,
    slicer: &[Member],
    ctx: &EvalCtx,
) -> Result<String, String> {
    let (groupby_positions, shape, measure_names) = match set {
        None => (Vec::new(), Vec::new(), slicer_measure_names(slicer)?),
        Some(set) => {
            let groupby_positions = classify_groupby_positions(set);
            let measure_position = set
                .shape
                .iter()
                .position(|href| *href == HierarchyRef::Measures);
            let measure_names = match measure_position {
                Some(mi) => distinct_measure_names(set, mi),
                None => slicer_measure_names(slicer)?,
            };
            (groupby_positions, set.shape.clone(), measure_names)
        }
    };

    let rollup_hierarchies: HashSet<(String, String)> = groupby_positions
        .iter()
        .filter(|&&(_, is_mixed)| is_mixed)
        .filter_map(|&(i, _)| match &shape[i] {
            HierarchyRef::Dimension { table, hier } => Some((table.clone(), hier.clone())),
            HierarchyRef::Measures => None,
        })
        .collect();

    let measure_pairs: Vec<(String, String)> = if measure_names.is_empty() {
        vec![("__implicit__".to_string(), "1".to_string())]
    } else {
        let mut pairs = Vec::with_capacity(measure_names.len());
        for name in &measure_names {
            let key = name.to_ascii_lowercase();
            let dax = if let Some(dax) = ctx.measures.get(&key) {
                dax.clone()
            } else if let Some(expr) = ctx.calculated_members.get(&key) {
                compile_measure_formula(expr, &rollup_hierarchies)?
            } else {
                return Err(format!("unknown measure: {name}"));
            };
            pairs.push((name.clone(), dax));
        }
        pairs
    };
    let measure_cols = measure_pairs
        .iter()
        .enumerate()
        .map(|(i, (_, expr))| format!("\"M{i}\", {expr}"))
        .collect::<Vec<_>>()
        .join(", ");

    let col_ref = |i: usize| -> String {
        match &shape[i] {
            HierarchyRef::Dimension { table, hier } => format!("'{table}'[{hier}]"),
            HierarchyRef::Measures => {
                unreachable!("a group-by position can never be the Measures hierarchy")
            }
        }
    };

    let inner = if groupby_positions.is_empty() {
        format!("ROW({measure_cols})")
    } else {
        let mut summarize_args: Vec<String> = Vec::new();
        let mut flag_idx = 0usize;
        for &(i, is_mixed) in &groupby_positions {
            let col = col_ref(i);
            if is_mixed {
                summarize_args.push(format!(
                    "ROLLUPADDISSUBTOTAL({col}, \"_IsTotal_{flag_idx}\")"
                ));
                flag_idx += 1;
                if let (HierarchyRef::Dimension { table, hier }, Some(evaluated)) = (&shape[i], set)
                {
                    if let Some(filter) = rollup_restriction_filter(ctx, evaluated, i, table, hier)?
                    {
                        summarize_args.push(filter);
                    }
                }
            } else {
                summarize_args.push(col);
            }
        }
        summarize_args.push(measure_cols.clone());
        format!("SUMMARIZECOLUMNS({})", summarize_args.join(", "))
    };

    let filters = slicer_filters(slicer);
    let inner = if filters.is_empty() {
        inner
    } else {
        format!("CALCULATETABLE({inner}, {})", filters.join(", "))
    };
    Ok(format!("EVALUATE {inner}"))
}

fn classify_groupby_positions(set: &EvaluatedSet) -> Vec<(usize, bool)> {
    let mut positions = Vec::new();
    for (i, href) in set.shape.iter().enumerate() {
        match href {
            HierarchyRef::Measures => {}
            HierarchyRef::Dimension { .. } => {
                let all_all = set
                    .tuples
                    .iter()
                    .all(|t| matches!(t.members[i], Member::All { .. }));
                let all_leaf = set
                    .tuples
                    .iter()
                    .all(|t| matches!(t.members[i], Member::Leaf { .. }));
                if all_leaf {
                    positions.push((i, false));
                } else if !all_all {
                    positions.push((i, true));
                }
            }
        }
    }
    positions
}

/// Restricts a rollup-mixed position's underlying column to exactly the
/// leaf keys present in the evaluated axis tuples at that position (plus
/// ISBLANK when a blank/unknown-member leaf is present) — but only when that
/// set is a genuine proper subset of the hierarchy's full domain (queried
/// via the same VALUES()-backed resolve_all_members eval_set already uses,
/// so it's free when the same hierarchy was already resolved during axis
/// evaluation, thanks to EvalCtx's own cache). For an axis built from
/// DrilldownLevel/AllMembers (the GTOPT "grand total + every detail row"
/// pattern), the evaluated leaves already cover the full domain, so no
/// filter is added — adding one anyway, even though logically a no-op,
/// changes how SUMMARIZECOLUMNS's nested ROLLUPADDISSUBTOTAL combinatorics
/// interact with the auto-drop rule and was found to silently drop
/// legitimate duplicate marginal-subtotal rows. For an axis built from a
/// narrower source — e.g. Generate over a filtered/named set, as in the
/// `.Children.Count`-style calculated-member pattern — this correctly
/// narrows the rollup to just the member(s) that pattern actually produced.
fn rollup_restriction_filter(
    ctx: &EvalCtx,
    set: &EvaluatedSet,
    i: usize,
    table: &str,
    hier: &str,
) -> Result<Option<String>, String> {
    let mut keys: Vec<String> = Vec::new();
    let mut has_blank = false;
    for t in &set.tuples {
        if let Member::Leaf { key, .. } = &t.members[i] {
            if key.is_empty() {
                has_blank = true;
            } else if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
    }
    if keys.is_empty() && !has_blank {
        return Ok(None);
    }

    let full_domain = ctx.resolve_all_members(table, hier)?;
    let mut full_keys: HashSet<&str> = HashSet::new();
    let mut full_has_blank = false;
    for m in &full_domain {
        if let Member::Leaf { key, .. } = m {
            if key.is_empty() {
                full_has_blank = true;
            } else {
                full_keys.insert(key.as_str());
            }
        }
    }
    let present_keys: HashSet<&str> = keys.iter().map(String::as_str).collect();
    if present_keys == full_keys && has_blank == full_has_blank {
        return Ok(None);
    }

    keys.sort();
    let col = format!("'{table}'[{hier}]");
    let mut clauses = Vec::new();
    if !keys.is_empty() {
        let quoted = keys
            .iter()
            .map(|k| format!("\"{k}\""))
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("{col} IN {{{quoted}}}"));
    }
    if has_blank {
        clauses.push(format!("ISBLANK({col})"));
    }
    Ok(Some(format!(
        "FILTER(ALL({col}), {})",
        clauses.join(" || ")
    )))
}

/// Compiles a calculated-member scalar formula (a `WITH MEMBER` body) into
/// DAX text, given which hierarchies in the current combined axis shape are
/// rollup-mixed (so member-navigation functions know whether CurrentMember
/// can be at the collapsed/All position — DAX's ISINSCOPE — or is always a
/// fixed leaf). A small compositional compiler over the same Expr AST
/// eval_set uses for axis-set expressions, not a per-formula pattern match.
fn compile_measure_formula(
    expr: &Expr,
    rollup_hierarchies: &HashSet<(String, String)>,
) -> Result<String, String> {
    match expr {
        Expr::MemberFunction { base, name, args }
            if args.is_empty() && name.eq_ignore_ascii_case("count") =>
        {
            let set = compile_set_expr(base, rollup_hierarchies)?;
            Ok(render_count(set, rollup_hierarchies))
        }
        Expr::FunctionCall { name, args } if name.eq_ignore_ascii_case("count") => {
            let [Some(inner)] = args.as_slice() else {
                return Err("Count requires exactly one argument".to_string());
            };
            let set = compile_set_expr(inner, rollup_hierarchies)?;
            Ok(render_count(set, rollup_hierarchies))
        }
        other => Err(format!(
            "{other:?} is not a supported calculated-measure formula"
        )),
    }
}

fn render_count(set: CompiledSet, rollup_hierarchies: &HashSet<(String, String)>) -> String {
    match set {
        CompiledSet::ChildrenOfCurrentMember { table, hier } => {
            let col = format!("'{table}'[{hier}]");
            if rollup_hierarchies.contains(&(table, hier)) {
                format!("IF(ISINSCOPE({col}), 0, COUNTROWS(ALL({col})))")
            } else {
                "0".to_string()
            }
        }
        CompiledSet::AllValuesOf { table, hier } => {
            format!("COUNTROWS(VALUES('{table}'[{hier}]))")
        }
    }
}

/// A compiled fragment for a set-shaped sub-expression inside a
/// calculated-member formula — distinct from `EvaluatedSet` (which holds
/// concrete axis tuples) because this is a DAX-text-generation-time
/// description of a set relative to CurrentMember, not a resolved value.
enum CompiledSet {
    /// The children of `CurrentMember` for `(table, hier)`: empty when
    /// CurrentMember is at the leaf level, every leaf value when it's at
    /// the collapsed/All level — matching DAX's ISINSCOPE distinction
    /// exactly, so `.Count` of this compiles to the ISINSCOPE-conditional
    /// form when the hierarchy is rollup-mixed in this query, or a bare `0`
    /// when it isn't (CurrentMember is then always a fixed leaf, whose
    /// children are unconditionally empty in this flat-hierarchy model).
    ChildrenOfCurrentMember {
        table: String,
        hier: String,
    },
    AllValuesOf {
        table: String,
        hier: String,
    },
}

fn compile_set_expr(
    expr: &Expr,
    rollup_hierarchies: &HashSet<(String, String)>,
) -> Result<CompiledSet, String> {
    match expr {
        Expr::Member(path) => {
            let (table, hier) = table_hier_of(path)?;
            Ok(CompiledSet::AllValuesOf { table, hier })
        }
        Expr::MemberFunction { base, name, args }
            if args.is_empty() && name.eq_ignore_ascii_case("children") =>
        {
            let Expr::MemberFunction { base: cm_base, name: cm_name, args: cm_args } =
                base.as_ref()
            else {
                return Err(
                    ".Children is only supported on CurrentMember in a calculated-measure formula"
                        .to_string(),
                );
            };
            if !cm_args.is_empty() || !cm_name.eq_ignore_ascii_case("currentmember") {
                return Err(
                    ".Children is only supported on CurrentMember in a calculated-measure formula"
                        .to_string(),
                );
            }
            let Expr::Member(path) = cm_base.as_ref() else {
                return Err("CurrentMember requires a hierarchy reference".to_string());
            };
            let (table, hier) = table_hier_of(path)?;
            Ok(CompiledSet::ChildrenOfCurrentMember { table, hier })
        }
        Expr::FunctionCall { name, args } if name.eq_ignore_ascii_case("addcalculatedmembers") => {
            let [Some(inner)] = args.as_slice() else {
                return Err("AddCalculatedMembers requires one argument".to_string());
            };
            // No genuine additional calculated members target any hierarchy
            // in this model, so this is an identity passthrough — the same
            // semantics eval_add_calculated_members already applies for
            // axis-set position.
            compile_set_expr(inner, rollup_hierarchies)
        }
        other => Err(format!(
            "{other:?} is not a supported set expression in a calculated-measure formula"
        )),
    }
}

pub fn distinct_measure_names(set: &EvaluatedSet, measure_position: usize) -> Vec<String> {
    let mut names: Vec<String> = set
        .tuples
        .iter()
        .map(|t| match &t.members[measure_position] {
            Member::Measure { name } => name.clone(),
            other => {
                unreachable!("expected Member::Measure at the Measures position, got {other:?}")
            }
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

fn slicer_measure_names(slicer: &[Member]) -> Result<Vec<String>, String> {
    Ok(slicer
        .iter()
        .filter_map(|m| match m {
            Member::Measure { name } => Some(name.clone()),
            Member::Leaf { .. } | Member::All { .. } => None,
        })
        .collect())
}

fn slicer_filters(slicer: &[Member]) -> Vec<String> {
    slicer
        .iter()
        .filter_map(|m| match m {
            Member::Leaf { table, hier, key, .. } => Some(format!("'{table}'[{hier}] = \"{key}\"")),
            Member::All { .. } | Member::Measure { .. } => None,
        })
        .collect()
}
