use super::eval::{EvalCtx, EvaluatedSet, HierarchyRef, Member};

pub fn generate_dax(
    set: Option<&EvaluatedSet>,
    slicer: &[Member],
    ctx: &EvalCtx,
) -> Result<String, String> {
    let (groupby_positions, shape, measure_names) = match set {
        None => (Vec::new(), Vec::new(), slicer_measure_names(slicer)?),
        Some(set) => {
            let groupby_positions = classify_groupby_positions(set)?;
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

    if measure_names.is_empty() {
        return Err("no measure specified on the axis or in the WHERE clause".to_string());
    }
    let mut measure_pairs = Vec::with_capacity(measure_names.len());
    for name in &measure_names {
        measure_pairs.push((name.clone(), ctx.resolve_measure(name)?.to_string()));
    }
    let measure_cols = measure_pairs
        .iter()
        .enumerate()
        .map(|(i, (_, expr))| format!("\"M{i}\", {expr}"))
        .collect::<Vec<_>>()
        .join(", ");

    let inner = if groupby_positions.is_empty() {
        format!("ROW({measure_cols})")
    } else {
        let cols = groupby_positions
            .iter()
            .map(|&i| match &shape[i] {
                HierarchyRef::Dimension { table, hier } => format!("'{table}'[{hier}]"),
                HierarchyRef::Measures => {
                    unreachable!("a group-by position can never be the Measures hierarchy")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("SUMMARIZECOLUMNS({cols}, {measure_cols})")
    };

    let filters = slicer_filters(slicer);
    let inner = if filters.is_empty() {
        inner
    } else {
        format!("CALCULATETABLE({inner}, {})", filters.join(", "))
    };
    Ok(format!("EVALUATE {inner}"))
}

fn classify_groupby_positions(set: &EvaluatedSet) -> Result<Vec<usize>, String> {
    let mut groupby_positions = Vec::new();
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
                    groupby_positions.push(i);
                } else if !all_all {
                    return Err(
                        "mixed All/Leaf shapes require multi-partition generation (not yet supported)"
                            .to_string(),
                    );
                }
            }
        }
    }
    Ok(groupby_positions)
}

fn distinct_measure_names(set: &EvaluatedSet, measure_position: usize) -> Vec<String> {
    let mut names: Vec<String> = set
        .tuples
        .iter()
        .map(|t| match &t.members[measure_position] {
            Member::Measure { name } => name.clone(),
            other => unreachable!("expected Member::Measure at the Measures position, got {other:?}"),
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
            Member::Leaf {
                table, hier, key, ..
            } => Some(format!("'{table}'[{hier}] = \"{key}\"")),
            Member::All { .. } | Member::Measure { .. } => None,
        })
        .collect()
}
