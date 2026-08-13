use super::eval::{EvalCtx, EvaluatedSet, HierarchyRef, Member};

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

    let measure_pairs: Vec<(String, String)> = if measure_names.is_empty() {
        vec![("__implicit__".to_string(), "1".to_string())]
    } else {
        let mut pairs = Vec::with_capacity(measure_names.len());
        for name in &measure_names {
            pairs.push((name.clone(), ctx.resolve_measure(name)?.to_string()));
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

    let has_mixed = groupby_positions.iter().any(|&(_, is_mixed)| is_mixed);

    let inner = if groupby_positions.is_empty() {
        format!("ROW({measure_cols})")
    } else {
        let mut flag_idx = 0usize;
        let cols = groupby_positions
            .iter()
            .map(|&(i, is_mixed)| {
                let col = col_ref(i);
                if is_mixed {
                    let flag = format!("ROLLUPADDISSUBTOTAL({col}, \"_IsTotal_{flag_idx}\")");
                    flag_idx += 1;
                    flag
                } else {
                    col
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let summarize = format!("SUMMARIZECOLUMNS({cols}, {measure_cols})");
        if has_mixed {
            let mut select_args: Vec<String> = groupby_positions
                .iter()
                .map(|&(i, _)| {
                    let col = col_ref(i);
                    format!("\"{col}\", {col}")
                })
                .collect();
            for mi in 0..measure_pairs.len() {
                select_args.push(format!("\"M{mi}\", [M{mi}]"));
            }
            format!("SELECTCOLUMNS({summarize}, {})", select_args.join(", "))
        } else {
            summarize
        }
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

fn distinct_measure_names(set: &EvaluatedSet, measure_position: usize) -> Vec<String> {
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
