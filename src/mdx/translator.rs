use super::ast::{Axis, FromClause, MdxQuery, MemberExpr, SetExpr, SetItem, Traversal};
use super::error::MdxError;
use crate::engine::table_col::TableCol;

// ── public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum QueryShape {
    /// Zero-axis: SELECT FROM [Model] WHERE ([Measures].[M])
    Scalar { measure_name: String },
    /// Pure measures on COLUMNS only, no dim axis.
    MeasuresOnly { measures: Vec<(String, String)> },
    /// CrossJoin(dim, measures) on a single axis (COLUMNS or ROWS), no second axis.
    SingleAxisCrossJoin {
        dim_axis: AxisPlan,
        measures: Vec<(String, String)>,
        measures_first: bool,
    },
    /// CrossJoin(dim, measures) on one axis + plain dim on the other.
    CrossJoinMatrix {
        crossjoin_dim: AxisPlan,
        plain_dim: AxisPlan,
        measures: Vec<(String, String)>,
        measures_first: bool,
        /// True when the CrossJoin is on ROWS (axis 1), false when on COLUMNS (axis 0).
        crossjoin_on_rows: bool,
    },
    /// Measures on one axis + single-hier dim on the other (no CrossJoin).
    DimMeasureMatrix {
        dim_axis: AxisPlan,
        measures: Vec<(String, String)>,
        /// True when measures are on ROWS, false when on COLUMNS.
        measures_on_rows: bool,
    },
    /// Measures on COLUMNS, two-hierarchy dim on ROWS.
    TwoHierWithMeasures {
        dim_axis: AxisPlan,
        measures: Vec<(String, String)>,
    },
    /// Two independent dim axes, optional single measure in WHERE.
    TwoDimAxes {
        col_axis: AxisPlan,
        row_axis: AxisPlan,
        measure_name: Option<String>,
    },
    /// Two-hierarchy dim on a single axis, optional measure from WHERE slicer.
    TwoHierDim {
        axis: AxisPlan,
        measure_name: Option<String>,
    },
    /// Single-hierarchy dim, optional measure (either in WHERE or on ROWS via Generate/Ascendants).
    SingleDim {
        axis: AxisPlan,
        measure_name: Option<String>,
        has_measure_axis: bool,
    },
    /// CrossJoin with N≥2 independently-drilled dims and measures on a single axis.
    /// `measures_position` is the index in the left-to-right tuple where Measures appears.
    SingleAxisMultiDimCrossJoin {
        dims: Vec<AxisPlan>,
        measures: Vec<(String, String)>,
        measures_position: usize,
    },
}

#[derive(Debug, Clone)]
pub struct SecondHierPlan {
    pub table: String,
    pub hier: String,
    pub level: String,
    pub dax_column: String,
}

#[derive(Debug, Clone)]
pub struct AxisPlan {
    pub axis_id: u32,
    pub table: String,
    pub hier: String,
    /// Leaf-level name, e.g. "Color".  "(All)" when the query targets only the All level.
    pub level: String,
    /// Column reference used in SUMMARIZECOLUMNS / VALUES, e.g. "vtest_product[Color]".
    pub dax_column: String,
    pub dim_props: Vec<String>,
    pub include_all: bool,
    /// True when the axis targets only the All (grand-total) member — no leaf rows needed.
    pub all_only: bool,
    /// Present for two-hierarchy DrilldownMember(CrossJoin(...)) queries.
    pub second_hier: Option<SecondHierPlan>,
}

#[derive(Debug, Clone)]
pub struct DaxTranslation {
    pub cube: String,
    pub cell_dax: Option<String>,
    pub total_dax: Option<String>,
    pub non_empty: bool,
    pub cell_props: Vec<String>,
    pub shape: QueryShape,
}

// ── private axis classifier ───────────────────────────────────────────────────

enum AxisContent {
    Dim(AxisPlan),
    Measures(Vec<(String, String)>),
    CrossJoin {
        dim: AxisPlan,
        measures: Vec<(String, String)>,
        measures_first: bool,
    },
    MultiDimCrossJoin {
        dims: Vec<AxisPlan>,
        measures: Vec<(String, String)>,
        measures_position: usize,
    },
}

fn classify_axis(axis: &Axis, calc_measures: &[(String, String)]) -> Result<AxisContent, MdxError> {
    if is_all_measures_set(&axis.set) {
        let measures = collect_measure_names(&axis.set, calc_measures);
        if !measures.is_empty() {
            return Ok(AxisContent::Measures(measures));
        }
    }
    if let Some((measures, measures_first)) =
        try_extract_col_crossjoin_dim_measures(&axis.set, calc_measures)
    {
        let plan = extract_axis(axis)?;
        return Ok(AxisContent::CrossJoin { dim: plan, measures, measures_first });
    }
    if let Some((dims, measures, measures_position)) =
        try_extract_multi_dim_crossjoin(&axis.set, calc_measures, axis.id, &axis.dim_props)
    {
        return Ok(AxisContent::MultiDimCrossJoin { dims, measures, measures_position });
    }
    Ok(AxisContent::Dim(extract_axis(axis)?))
}

// ── DAX building helpers ──────────────────────────────────────────────────────

/// Build `EVALUATE SUMMARIZECOLUMNS(groupby..., [filters,] measure_pairs)`.
/// `measure_pairs` must start with `, ` when non-empty (e.g. `", \"M0\", expr"`).
fn build_summarize_dax(
    groupby_cols: &[(&str, &str)],
    filter_args: &[String],
    measure_pairs: &str,
) -> String {
    let cols: String = groupby_cols
        .iter()
        .map(|(t, c)| format!("'{t}'[{c}]"))
        .collect::<Vec<_>>()
        .join(", ");
    if filter_args.is_empty() {
        format!("EVALUATE SUMMARIZECOLUMNS({cols}{measure_pairs})")
    } else {
        let filters = filter_args.join(", ");
        format!("EVALUATE SUMMARIZECOLUMNS({cols}, {filters}{measure_pairs})")
    }
}

/// Format multi-measure columns as `", \"M0\", expr0, \"M1\", expr1, ..."`.
fn format_measure_cols(measures: &[(String, String)]) -> String {
    measures
        .iter()
        .enumerate()
        .map(|(i, (_, e))| format!(", \"M{i}\", {e}"))
        .collect()
}

// ── public entry point ────────────────────────────────────────────────────────

pub fn mdx_to_dax(query: &MdxQuery) -> Result<DaxTranslation, MdxError> {
    let (cube, subquery_key_members) = match &query.from {
        FromClause::Cube(name) => (name.clone(), vec![]),
        FromClause::SubqueryCube { cube, key_members } => (cube.clone(), key_members.clone()),
        FromClause::System { .. } => {
            return Err(MdxError::Translate(
                "expected cube query, got $system".into(),
            ));
        }
    };

    let measure_name = query
        .slicer
        .iter()
        .find(|m| m.member.is_measure())
        .and_then(|m| m.member.parts.last().cloned());

    let slicer_key_filters: Vec<&MemberExpr> = query
        .slicer
        .iter()
        .filter(|m| !m.member.is_measure() && m.member.key.is_some())
        .collect();

    let subquery_member_exprs: Vec<MemberExpr> = subquery_key_members
        .into_iter()
        .map(|r| MemberExpr { member: r, traversal: None })
        .collect();

    let mut key_filters: Vec<&MemberExpr> = slicer_key_filters;
    key_filters.extend(subquery_member_exprs.iter());

    let groups = key_filter_groups(&key_filters);
    let filter_args = filter_all_args(&groups);

    // Scalar: no axes, measure in WHERE.
    if query.axes.is_empty() {
        if let Some(ref mn) = measure_name {
            let lower = mn.to_lowercase();
            let measure_expr = query
                .calc_measures
                .iter()
                .find(|(n, _)| n == &lower)
                .map(|(_, e)| e.clone())
                .unwrap_or_else(|| format!("[{}]", mn));
            let scalar_dax = if groups.is_empty() {
                format!("EVALUATE {{ CALCULATE({}) }}", measure_expr)
            } else {
                let fc = filter_calc_args(&groups).join(", ");
                format!("EVALUATE {{ CALCULATE({}, {}) }}", measure_expr, fc)
            };
            return Ok(DaxTranslation {
                cube,
                cell_dax: Some(scalar_dax),
                total_dax: None,
                non_empty: false,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::Scalar { measure_name: mn.clone() },
            });
        }
    }

    let ax0 = query.axes.iter().find(|a| a.id == 0);
    let ax1 = query.axes.iter().find(|a| a.id == 1);
    let non_empty =
        ax0.map(|a| a.non_empty).unwrap_or(false) | ax1.map(|a| a.non_empty).unwrap_or(false);

    // GenerateAscendants: Hierarchize(Generate(NamedSetRef, Ascendants(member))) on COLUMNS.
    if let Some(col_ax) = ax0 {
        if let Some((gen_axis, gen_key_members)) =
            try_extract_generate_ascendants_axis(col_ax, &query.named_sets)
        {
            let gen_measure_name = ax1
                .and_then(|ax| unwrap_to_member(&ax.set))
                .filter(|m| m.member.is_measure())
                .and_then(|m| m.member.parts.last().cloned())
                .or_else(|| measure_name.clone());

            let mut combined_filters: Vec<&MemberExpr> = key_filters.clone();
            combined_filters.extend(gen_key_members.iter());

            let (cell_dax, _) = build_dax_strs(
                &gen_axis,
                gen_measure_name.as_deref(),
                &combined_filters,
                &query.calc_measures,
            );
            // total_dax is deliberately unfiltered — the named-set key filter scopes
            // only the leaf rows, not the grand-total All row.
            let total_dax = gen_measure_name.as_deref().map(|mn| {
                let lower = mn.to_lowercase();
                let measure_expr = query
                    .calc_measures
                    .iter()
                    .find(|(n, _)| n == &lower)
                    .map(|(_, e)| e.clone())
                    .unwrap_or_else(|| format!("[{}]", mn));
                format!("EVALUATE {{ CALCULATE({}) }}", measure_expr)
            });
            let has_measure_axis = ax1.is_some();
            return Ok(DaxTranslation {
                cube,
                cell_dax,
                total_dax,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleDim {
                    axis: gen_axis,
                    measure_name: gen_measure_name,
                    has_measure_axis,
                },
            });
        }
    }

    // Classify each axis and match on the combination.
    let content0 = ax0
        .map(|a| classify_axis(a, &query.calc_measures))
        .transpose()?;
    let content1 = ax1
        .map(|a| classify_axis(a, &query.calc_measures))
        .transpose()?;

    match (content0, content1) {
        // ── MeasuresOnly: {measures} ON COLUMNS, no row axis ─────────────────
        (Some(AxisContent::Measures(measures)), None) => {
            let measure_cols: String = measures
                .iter()
                .enumerate()
                .map(|(i, (_, expr))| {
                    if i == 0 {
                        format!("\"M{i}\", {expr}")
                    } else {
                        format!(", \"M{i}\", {expr}")
                    }
                })
                .collect();
            let cell_dax = format!("EVALUATE ROW({})", measure_cols);
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::MeasuresOnly { measures },
            })
        }

        // ── SingleAxisMultiDimCrossJoin ON COLUMNS (no ROWS) ─────────────────
        (Some(AxisContent::MultiDimCrossJoin { dims, measures, measures_position }), None) => {
            let meas = format_measure_cols(&measures);
            let groupby: Vec<(&str, &str)> = dims
                .iter()
                .map(|d| (d.table.as_str(), d.level.as_str()))
                .collect();
            let cell_dax = build_summarize_dax(&groupby, &filter_args, &meas);
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleAxisMultiDimCrossJoin {
                    dims,
                    measures,
                    measures_position,
                },
            })
        }

        // ── SingleAxisMultiDimCrossJoin ON ROWS (no COLUMNS) ─────────────────
        (None, Some(AxisContent::MultiDimCrossJoin { dims, measures, measures_position })) => {
            let meas = format_measure_cols(&measures);
            let groupby: Vec<(&str, &str)> = dims
                .iter()
                .map(|d| (d.table.as_str(), d.level.as_str()))
                .collect();
            let cell_dax = build_summarize_dax(&groupby, &filter_args, &meas);
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleAxisMultiDimCrossJoin {
                    dims,
                    measures,
                    measures_position,
                },
            })
        }

        // ── SingleAxisCrossJoin ON COLUMNS (no ROWS) ─────────────────────────
        (Some(AxisContent::CrossJoin { dim, measures, measures_first }), None) => {
            let meas = format_measure_cols(&measures);
            let cell_dax = {
                let mut groupby: Vec<(&str, &str)> = vec![(&dim.table, &dim.level)];
                if let Some(ref sh) = dim.second_hier {
                    groupby.push((&sh.table, &sh.level));
                }
                build_summarize_dax(&groupby, &filter_args, &meas)
            };
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleAxisCrossJoin { dim_axis: dim, measures, measures_first },
            })
        }

        // ── SingleAxisCrossJoin ON ROWS (no COLUMNS) ─────────────────────────
        (None, Some(AxisContent::CrossJoin { dim, measures, measures_first })) => {
            let meas = format_measure_cols(&measures);
            let cell_dax = {
                let mut groupby: Vec<(&str, &str)> = vec![(&dim.table, &dim.level)];
                if let Some(ref sh) = dim.second_hier {
                    groupby.push((&sh.table, &sh.level));
                }
                build_summarize_dax(&groupby, &filter_args, &meas)
            };
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleAxisCrossJoin { dim_axis: dim, measures, measures_first },
            })
        }

        // ── CrossJoinMatrix ON COLUMNS + plain dim ON ROWS ───────────────────
        (
            Some(AxisContent::CrossJoin { dim: cj_dim, measures, measures_first }),
            Some(AxisContent::Dim(plain_dim)),
        ) => {
            let meas = format_measure_cols(&measures);
            let cell_dax = build_summarize_dax(
                &[
                    (&cj_dim.table, &cj_dim.level),
                    (&plain_dim.table, &plain_dim.level),
                ],
                &filter_args,
                &meas,
            );
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::CrossJoinMatrix {
                    crossjoin_dim: cj_dim,
                    plain_dim,
                    measures,
                    measures_first,
                    crossjoin_on_rows: false,
                },
            })
        }

        // ── CrossJoinMatrix ON ROWS + plain dim ON COLUMNS ───────────────────
        (
            Some(AxisContent::Dim(plain_dim)),
            Some(AxisContent::CrossJoin { dim: cj_dim, measures, measures_first }),
        ) => {
            let meas = format_measure_cols(&measures);
            let cell_dax = build_summarize_dax(
                &[
                    (&cj_dim.table, &cj_dim.level),
                    (&plain_dim.table, &plain_dim.level),
                ],
                &filter_args,
                &meas,
            );
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::CrossJoinMatrix {
                    crossjoin_dim: cj_dim,
                    plain_dim,
                    measures,
                    measures_first,
                    crossjoin_on_rows: true,
                },
            })
        }

        // ── Measures ON COLUMNS, dim ON ROWS ─────────────────────────────────
        // Two-hier dim → TwoHierWithMeasures; single-hier → DimMeasureMatrix.
        (Some(AxisContent::Measures(measures)), Some(AxisContent::Dim(dim_plan))) => {
            let meas = format_measure_cols(&measures);
            let cell_dax = if let Some(ref sh) = dim_plan.second_hier {
                build_summarize_dax(
                    &[(&dim_plan.table, &dim_plan.level), (&sh.table, &sh.level)],
                    &filter_args,
                    &meas,
                )
            } else {
                build_summarize_dax(&[(&dim_plan.table, &dim_plan.level)], &filter_args, &meas)
            };
            let shape = if dim_plan.second_hier.is_some() {
                QueryShape::TwoHierWithMeasures { dim_axis: dim_plan, measures }
            } else {
                QueryShape::DimMeasureMatrix {
                    dim_axis: dim_plan,
                    measures,
                    measures_on_rows: false,
                }
            };
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape,
            })
        }

        // ── Dim ON COLUMNS, measures ON ROWS → DimMeasureMatrix ──────────────
        (Some(AxisContent::Dim(dim_plan)), Some(AxisContent::Measures(measures))) => {
            let meas = format_measure_cols(&measures);
            let cell_dax =
                build_summarize_dax(&[(&dim_plan.table, &dim_plan.level)], &filter_args, &meas);
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::DimMeasureMatrix {
                    dim_axis: dim_plan,
                    measures,
                    measures_on_rows: true,
                },
            })
        }

        // ── Two dim axes (no CrossJoin on either) ────────────────────────────
        (Some(AxisContent::Dim(col_plan)), Some(AxisContent::Dim(row_plan)))
            if col_plan.second_hier.is_none() && row_plan.second_hier.is_none() =>
        {
            let cell_dax = if let Some(ref mn) = measure_name {
                let lower = mn.to_lowercase();
                let measure_expr = query
                    .calc_measures
                    .iter()
                    .find(|(n, _)| n == &lower)
                    .map(|(_, e)| e.clone())
                    .unwrap_or_else(|| format!("[{}]", mn));
                build_summarize_dax(
                    &[
                        (&col_plan.table, &col_plan.level),
                        (&row_plan.table, &row_plan.level),
                    ],
                    &filter_args,
                    &format!(", \"Value\", {}", measure_expr),
                )
            } else {
                build_summarize_dax(
                    &[
                        (&col_plan.table, &col_plan.level),
                        (&row_plan.table, &row_plan.level),
                    ],
                    &filter_args,
                    "",
                )
            };
            Ok(DaxTranslation {
                cube,
                cell_dax: Some(cell_dax),
                total_dax: None,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::TwoDimAxes {
                    col_axis: col_plan,
                    row_axis: row_plan,
                    measure_name,
                },
            })
        }

        // ── Single dim axis only (no row axis) ───────────────────────────────
        // Handles both single-hier (SingleDim) and two-hier (TwoHierDim) cases.
        (Some(AxisContent::Dim(plan)), None) => {
            let (cell_dax, total_dax) = build_dax_strs(
                &plan,
                measure_name.as_deref(),
                &key_filters,
                &query.calc_measures,
            );
            let shape = if plan.second_hier.is_some() {
                QueryShape::TwoHierDim { axis: plan, measure_name }
            } else {
                QueryShape::SingleDim { axis: plan, measure_name, has_measure_axis: false }
            };
            Ok(DaxTranslation {
                cube,
                cell_dax,
                total_dax,
                non_empty,
                cell_props: query.cell_props.clone(),
                shape,
            })
        }

        // ── Fallback: unrecognised combination — take the first axis ──────────
        _ => {
            let axis_clause = query
                .axes
                .first()
                .ok_or_else(|| MdxError::Translate("cube query has no axes".into()))?;
            let axis = extract_axis(axis_clause)?;
            let (cell_dax, total_dax) = build_dax_strs(
                &axis,
                measure_name.as_deref(),
                &key_filters,
                &query.calc_measures,
            );
            Ok(DaxTranslation {
                cube,
                cell_dax,
                total_dax,
                non_empty: axis_clause.non_empty,
                cell_props: query.cell_props.clone(),
                shape: QueryShape::SingleDim { axis, measure_name, has_measure_axis: false },
            })
        }
    }
}

// ── axis extraction ───────────────────────────────────────────────────────────

fn extract_axis(axis_clause: &Axis) -> Result<AxisPlan, MdxError> {
    if let Some(plan) = try_extract_two_hier_axis(axis_clause) {
        return Ok(plan);
    }

    let has_drilldown = contains_drilldown(&axis_clause.set);
    let member_expr = unwrap_to_member(&axis_clause.set)
        .ok_or_else(|| MdxError::Translate("no member expression found in axis set".into()))?;

    let has_children_traversal = member_expr.traversal == Some(Traversal::Children);

    let parts = &member_expr.member.parts;
    if parts.is_empty() {
        return Err(MdxError::Translate("empty member reference in axis".into()));
    }

    if has_children_traversal && member_expr.member.key.is_some() {
        let table = parts[0].clone();
        let hier = parts.get(1).cloned().unwrap_or_else(|| table.clone());
        return Ok(AxisPlan {
            axis_id: axis_clause.id,
            dax_column: TableCol::new(&table, &hier).to_string(),
            level: hier.clone(),
            table,
            hier,
            dim_props: axis_clause.dim_props.clone(),
            include_all: false,
            all_only: true,
            second_hier: None,
        });
    }

    let treats_as_drilldown = has_drilldown || has_children_traversal;

    let table = parts[0].clone();
    let hier = parts.get(1).cloned().unwrap_or_else(|| table.clone());
    let level = if treats_as_drilldown {
        let raw = parts.get(2).cloned().unwrap_or_else(|| hier.clone());
        if is_all_level(&raw) {
            hier.clone()
        } else {
            raw
        }
    } else {
        parts.get(2).cloned().unwrap_or_else(|| hier.clone())
    };

    let all_only = !treats_as_drilldown && is_all_level(&level);
    let dax_col_name = if all_only { &hier } else { &level };
    let dax_column = TableCol::new(&table, dax_col_name).to_string();

    Ok(AxisPlan {
        axis_id: axis_clause.id,
        table,
        hier,
        level,
        dax_column,
        dim_props: axis_clause.dim_props.clone(),
        include_all: !has_children_traversal,
        all_only,
        second_hier: None,
    })
}

fn try_extract_two_hier_axis(axis_clause: &Axis) -> Option<AxisPlan> {
    let (base, _members, hier_arg) = find_drilldown_member(&axis_clause.set)?;
    let (left_set, right_set) = find_crossjoin(base)?;

    let m1 = unwrap_to_member(left_set)?;
    let parts1 = &m1.member.parts;
    let table1 = parts1.first()?.clone();
    let hier1 = parts1.get(1).cloned().unwrap_or_else(|| table1.clone());
    let raw1 = parts1.get(2).cloned().unwrap_or_else(|| hier1.clone());
    let level1 = if is_all_level(&raw1) {
        hier1.clone()
    } else {
        raw1
    };

    let (table2, hier2, level2) = if let Some(parts2) = hier_arg {
        let t2 = parts2.first()?.clone();
        let h2 = parts2.get(1).cloned().unwrap_or_else(|| t2.clone());
        let r2 = parts2.get(2).cloned().unwrap_or_else(|| h2.clone());
        let l2 = if is_all_level(&r2) { h2.clone() } else { r2 };
        (t2, h2, l2)
    } else {
        let m2 = unwrap_to_member(right_set)?;
        let parts2 = &m2.member.parts;
        let t2 = parts2.first()?.clone();
        let h2 = parts2.get(1).cloned().unwrap_or_else(|| t2.clone());
        let r2 = parts2.get(2).cloned().unwrap_or_else(|| h2.clone());
        let l2 = if is_all_level(&r2) { h2.clone() } else { r2 };
        (t2, h2, l2)
    };

    Some(AxisPlan {
        axis_id: axis_clause.id,
        table: table1.clone(),
        hier: hier1.clone(),
        level: level1.clone(),
        dax_column: TableCol::new(&table1, &level1).to_string(),
        dim_props: axis_clause.dim_props.clone(),
        include_all: true,
        all_only: false,
        second_hier: Some(SecondHierPlan {
            dax_column: TableCol::new(&table2, &level2).to_string(),
            table: table2,
            hier: hier2,
            level: level2,
        }),
    })
}

fn find_drilldown_member(set: &SetExpr) -> Option<(&SetExpr, &SetExpr, Option<&Vec<String>>)> {
    match set {
        SetExpr::DrilldownMember { base, members, hier } => {
            Some((base.as_ref(), members.as_ref(), hier.as_ref()))
        }
        SetExpr::Hierarchize(inner) | SetExpr::AddCalculatedMembers(inner) => {
            find_drilldown_member(inner)
        }
        SetExpr::Literal(items) => {
            for item in items {
                if let SetItem::Set(s) = item {
                    if let Some(found) = find_drilldown_member(s) {
                        return Some(found);
                    }
                }
            }
            None
        }
        SetExpr::CrossJoin(left, right) => {
            if is_all_measures_set(right) {
                find_drilldown_member(left)
            } else if is_all_measures_set(left) {
                find_drilldown_member(right)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn find_crossjoin(set: &SetExpr) -> Option<(&SetExpr, &SetExpr)> {
    match set {
        SetExpr::CrossJoin(l, r) => Some((l.as_ref(), r.as_ref())),
        SetExpr::Hierarchize(inner) | SetExpr::AddCalculatedMembers(inner) => find_crossjoin(inner),
        _ => None,
    }
}

fn is_all_level(level: &str) -> bool {
    let stripped = level.trim_matches(|c| c == '(' || c == ')');
    stripped.eq_ignore_ascii_case("all")
}

fn unwrap_to_member(set: &SetExpr) -> Option<&MemberExpr> {
    match set {
        SetExpr::Hierarchize(inner)
        | SetExpr::AddCalculatedMembers(inner)
        | SetExpr::DrilldownLevel(inner) => unwrap_to_member(inner),
        SetExpr::CrossJoin(left, right) => {
            if is_all_measures_set(left) {
                unwrap_to_member(right)
            } else {
                unwrap_to_member(left)
            }
        }
        SetExpr::DrilldownMember { base, .. } => unwrap_to_member(base),
        SetExpr::Generate(set, _) => unwrap_to_member(set),
        SetExpr::Ascendants(m) => Some(m.as_ref()),
        SetExpr::Literal(items) => {
            // Excel's "GTOPT" idiom puts a grand-total section (the bare
            // [Hier].[All] member) and a detail section (Hierarchize/
            // .AllMembers/DrilldownLevel over real level members) side by
            // side in one union: { {[Hier].[All]} , {Hierarchize({...})} }.
            // The detail section's member carries the actual level name
            // needed to resolve a DAX column — prefer it, falling back to
            // an All member only when no detail section is present.
            let mut fallback: Option<&MemberExpr> = None;
            for item in items {
                let found = match item {
                    SetItem::Member(m) => Some(m),
                    SetItem::Set(s) => unwrap_to_member(s),
                };
                if let Some(m) = found {
                    let is_all = m
                        .member
                        .parts
                        .last()
                        .map(|p| is_all_level(p))
                        .unwrap_or(false);
                    if is_all {
                        fallback.get_or_insert(m);
                    } else {
                        return Some(m);
                    }
                }
            }
            fallback
        }
        SetExpr::NamedSetRef(_) => None,
    }
}

fn contains_drilldown(set: &SetExpr) -> bool {
    match set {
        SetExpr::DrilldownLevel(_) => true,
        SetExpr::Hierarchize(inner) | SetExpr::AddCalculatedMembers(inner) => {
            contains_drilldown(inner)
        }
        SetExpr::CrossJoin(l, r) => contains_drilldown(l) || contains_drilldown(r),
        SetExpr::DrilldownMember { base, .. } => contains_drilldown(base),
        SetExpr::Generate(set, body) => contains_drilldown(set) || contains_drilldown(body),
        SetExpr::Literal(items) => items.iter().any(|item| match item {
            SetItem::Set(s) => contains_drilldown(s),
            SetItem::Member(_) => false,
        }),
        SetExpr::Ascendants(_) | SetExpr::NamedSetRef(_) => false,
    }
}

// ── DAX filter helpers ────────────────────────────────────────────────────────

fn key_filter_groups(key_filters: &[&MemberExpr]) -> Vec<((String, String), Vec<String>)> {
    let mut groups: Vec<((String, String), Vec<String>)> = Vec::new();
    for m in key_filters.iter() {
        let Some(key) = m.member.key.as_deref() else {
            continue;
        };
        let parts = &m.member.parts;
        if parts.len() < 2 {
            continue;
        }
        let table = parts[0].clone();
        let col = parts[1].clone();
        if let Some(g) = groups
            .iter_mut()
            .find(|((t, c), _)| t == &table && c == &col)
        {
            g.1.push(key.to_string());
        } else {
            groups.push(((table, col), vec![key.to_string()]));
        }
    }
    groups
}

fn format_key_literal(key: &str) -> String {
    let s = key.trim();
    let is_int = !s.is_empty()
        && s != "-"
        && s.bytes().enumerate().all(|(i, b)| {
            if i == 0 {
                b == b'-' || b.is_ascii_digit()
            } else {
                b.is_ascii_digit()
            }
        });
    if is_int {
        s.to_string()
    } else {
        format!("\"{}\"", s)
    }
}

fn filter_all_args(groups: &[((String, String), Vec<String>)]) -> Vec<String> {
    groups
        .iter()
        .map(|((t, c), keys)| {
            if keys.len() == 1 {
                format!(
                    "FILTER(ALL('{t}'[{c}]), '{t}'[{c}] = {})",
                    format_key_literal(&keys[0])
                )
            } else {
                let vals = keys
                    .iter()
                    .map(|k| format_key_literal(k))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("FILTER(ALL('{t}'[{c}]), '{t}'[{c}] IN {{{vals}}})")
            }
        })
        .collect()
}

fn filter_calc_args(groups: &[((String, String), Vec<String>)]) -> Vec<String> {
    groups
        .iter()
        .map(|((t, c), keys)| {
            if keys.len() == 1 {
                format!("'{t}'[{c}] = {}", format_key_literal(&keys[0]))
            } else {
                let vals = keys
                    .iter()
                    .map(|k| format_key_literal(k))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("'{t}'[{c}] IN {{{vals}}}")
            }
        })
        .collect()
}

// ── DAX string construction ───────────────────────────────────────────────────

fn build_dax_strs(
    axis: &AxisPlan,
    measure: Option<&str>,
    key_filters: &[&MemberExpr],
    calc_measures: &[(String, String)],
) -> (Option<String>, Option<String>) {
    if axis.all_only {
        return (None, None);
    }

    let Some(measure) = measure else {
        let cell_dax = if let Some(ref sh) = axis.second_hier {
            format!(
                "EVALUATE SUMMARIZECOLUMNS('{}'[{}], '{}'[{}])",
                axis.table, axis.level, sh.table, sh.level
            )
        } else {
            format!("EVALUATE VALUES('{}'[{}])", axis.table, axis.level)
        };
        return (Some(cell_dax), None);
    };

    let lower = measure.to_lowercase();
    let measure_dax = match calc_measures.iter().find(|(n, _)| n == &lower) {
        Some((_, expr)) => expr.clone(),
        None => format!("[{}]", measure),
    };

    let groups = key_filter_groups(key_filters);

    if groups.is_empty() {
        let cell_dax = if let Some(ref sh) = axis.second_hier {
            format!(
                "EVALUATE SUMMARIZECOLUMNS('{}'[{}], '{}'[{}], \"Value\", {})",
                axis.table, axis.level, sh.table, sh.level, measure_dax
            )
        } else {
            format!(
                "EVALUATE SUMMARIZECOLUMNS('{}'[{}], \"Value\", {})",
                axis.table, axis.level, measure_dax
            )
        };
        let total_dax = format!("EVALUATE {{ CALCULATE({}) }}", measure_dax);
        (Some(cell_dax), Some(total_dax))
    } else {
        let filter_args = filter_all_args(&groups).join(", ");
        let filter_calc = filter_calc_args(&groups).join(", ");

        let cell_dax = if let Some(ref sh) = axis.second_hier {
            format!(
                "EVALUATE SUMMARIZECOLUMNS('{}'[{}], '{}'[{}], {}, \"Value\", {})",
                axis.table, axis.level, sh.table, sh.level, filter_args, measure_dax
            )
        } else {
            format!(
                "EVALUATE SUMMARIZECOLUMNS('{}'[{}], {}, \"Value\", {})",
                axis.table, axis.level, filter_args, measure_dax
            )
        };
        let total_dax = format!("EVALUATE {{ CALCULATE({}, {}) }}", measure_dax, filter_calc);
        (Some(cell_dax), Some(total_dax))
    }
}

// ── matrix helpers ────────────────────────────────────────────────────────────

fn is_all_measures_set(set: &SetExpr) -> bool {
    match set {
        SetExpr::Literal(items) => {
            !items.is_empty()
                && items.iter().all(|item| match item {
                    SetItem::Member(m) => m.member.is_measure(),
                    SetItem::Set(s) => is_all_measures_set(s),
                })
        }
        _ => false,
    }
}

fn collect_measure_names(
    set: &SetExpr,
    calc_measures: &[(String, String)],
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_measures_inner(set, calc_measures, &mut out);
    out
}

fn collect_measures_inner(
    set: &SetExpr,
    calc_measures: &[(String, String)],
    out: &mut Vec<(String, String)>,
) {
    if let SetExpr::Literal(items) = set {
        for item in items {
            match item {
                SetItem::Member(m) if m.member.is_measure() => {
                    if let Some(name) = m.member.parts.last() {
                        let lower = name.to_lowercase();
                        let expr = calc_measures
                            .iter()
                            .find(|(n, _)| n == &lower)
                            .map(|(_, e)| e.clone())
                            .unwrap_or_else(|| format!("[{}]", name));
                        out.push((name.clone(), expr));
                    }
                }
                SetItem::Set(s) => collect_measures_inner(s, calc_measures, out),
                _ => {}
            }
        }
    }
}

fn try_extract_col_crossjoin_dim_measures(
    set: &SetExpr,
    calc_measures: &[(String, String)],
) -> Option<(Vec<(String, String)>, bool)> {
    let (left, right) = find_crossjoin(set)?;
    let (meas_side, is_meas_left) = if is_all_measures_set(left) {
        (left, true)
    } else if is_all_measures_set(right) {
        (right, false)
    } else {
        return None;
    };
    let measures = collect_measure_names(meas_side, calc_measures);
    if measures.is_empty() {
        None
    } else {
        Some((measures, is_meas_left))
    }
}

fn flatten_crossjoin_parts(set: &SetExpr) -> Vec<&SetExpr> {
    match set {
        SetExpr::CrossJoin(l, r) => {
            let mut v = flatten_crossjoin_parts(l);
            v.extend(flatten_crossjoin_parts(r));
            v
        }
        other => vec![other],
    }
}

fn extract_plan_from_set(set: &SetExpr, axis_id: u32, dim_props: &[String]) -> Option<AxisPlan> {
    let has_drilldown = contains_drilldown(set);
    let member_expr = unwrap_to_member(set)?;
    let has_children_traversal = member_expr.traversal == Some(Traversal::Children);
    let parts = &member_expr.member.parts;
    if parts.is_empty() {
        return None;
    }
    let table = parts[0].clone();
    let hier = parts.get(1).cloned().unwrap_or_else(|| table.clone());
    let treats_as_drilldown = has_drilldown || has_children_traversal;
    let level = if treats_as_drilldown {
        let raw = parts.get(2).cloned().unwrap_or_else(|| hier.clone());
        if is_all_level(&raw) {
            hier.clone()
        } else {
            raw
        }
    } else {
        parts.get(2).cloned().unwrap_or_else(|| hier.clone())
    };
    let all_only = !treats_as_drilldown && is_all_level(&level);
    let dax_col_name = if all_only { &hier } else { &level };
    let dax_column = TableCol::new(&table, dax_col_name).to_string();
    Some(AxisPlan {
        axis_id,
        table,
        hier,
        level,
        dax_column,
        dim_props: dim_props.to_vec(),
        include_all: !has_children_traversal,
        all_only,
        second_hier: None,
    })
}

#[allow(clippy::type_complexity)]
fn try_extract_multi_dim_crossjoin(
    set: &SetExpr,
    calc_measures: &[(String, String)],
    axis_id: u32,
    dim_props: &[String],
) -> Option<(Vec<AxisPlan>, Vec<(String, String)>, usize)> {
    let parts = flatten_crossjoin_parts(set);
    if parts.len() < 3 {
        return None;
    }

    let mut meas_idx_in_parts: Option<usize> = None;
    for (i, part) in parts.iter().enumerate() {
        if is_all_measures_set(part) {
            if meas_idx_in_parts.is_some() {
                return None;
            }
            meas_idx_in_parts = Some(i);
        }
    }
    let meas_idx = meas_idx_in_parts?;

    let measures_position = parts[..meas_idx]
        .iter()
        .filter(|p| !is_all_measures_set(p))
        .count();

    let measures = collect_measure_names(parts[meas_idx], calc_measures);
    if measures.is_empty() {
        return None;
    }

    let dims: Option<Vec<AxisPlan>> = parts
        .iter()
        .filter(|p| !is_all_measures_set(p))
        .map(|part| extract_plan_from_set(part, axis_id, dim_props))
        .collect();
    let dims = dims?;
    if dims.len() < 2 {
        return None;
    }

    Some((dims, measures, measures_position))
}

// ── Generate / Ascendants axis detection ──────────────────────────────────────

fn try_extract_generate_ascendants_axis(
    axis_clause: &Axis,
    named_sets: &[(String, SetExpr)],
) -> Option<(AxisPlan, Vec<MemberExpr>)> {
    let inner = match &axis_clause.set {
        SetExpr::Hierarchize(inner) => inner.as_ref(),
        other => other,
    };

    let (set_ref_name, asc_member) = match inner {
        SetExpr::Generate(set_box, body_box) => {
            let name = match set_box.as_ref() {
                SetExpr::NamedSetRef(n) => n,
                _ => return None,
            };
            let asc_member = match body_box.as_ref() {
                SetExpr::Ascendants(m) => m.as_ref(),
                _ => return None,
            };
            (name, asc_member)
        }
        _ => return None,
    };

    let lower = set_ref_name.to_lowercase();
    let (_, named_set) = named_sets.iter().find(|(n, _)| n == &lower)?;

    let key_members = collect_key_members_from_set(named_set);

    let parts = &asc_member.member.parts;
    if parts.is_empty() {
        return None;
    }
    let table = parts[0].clone();
    let hier = parts.get(1).cloned().unwrap_or_else(|| table.clone());
    let level = hier.clone();

    let axis_plan = AxisPlan {
        axis_id: axis_clause.id,
        table: table.clone(),
        hier: hier.clone(),
        level: level.clone(),
        dax_column: TableCol::new(&table, &level).to_string(),
        dim_props: axis_clause.dim_props.clone(),
        include_all: true,
        all_only: false,
        second_hier: None,
    };

    Some((axis_plan, key_members))
}

fn collect_key_members_from_set(set: &SetExpr) -> Vec<MemberExpr> {
    let mut out = Vec::new();
    collect_key_members_inner(set, &mut out);
    out
}

fn collect_key_members_inner(set: &SetExpr, out: &mut Vec<MemberExpr>) {
    if let SetExpr::Literal(items) = set {
        for item in items {
            match item {
                SetItem::Member(m) if m.member.key.is_some() => out.push(m.clone()),
                SetItem::Member(_) => {}
                SetItem::Set(s) => collect_key_members_inner(s, out),
            }
        }
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::mdx::parser::parse_mdx;

    #[test]
    fn translate_drilldown_level_expands_all_to_column() {
        let q = parse_mdx(
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(t.cube, "Model");
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(axis.level, "Color");
        assert_eq!(axis.dax_column, "Product[Color]");
        assert!(!axis.all_only);
        assert!(t.non_empty);
        assert_eq!(
            t.cell_dax.as_deref(),
            Some("EVALUATE VALUES('Product'[Color])")
        );
    }

    #[test]
    fn translate_q6_measure_slicer() {
        let q = parse_mdx(
            "SELECT NON EMPTY {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME, HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(t.cube, "Model");
        assert_eq!(measure_name.as_deref(), Some("TotalAmount"));
        assert!(t.non_empty);
        assert_eq!(axis.table, "vtest_product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(axis.level, "Color");
        assert_eq!(axis.dax_column, "vtest_product[Color]");
        assert_eq!(
            axis.dim_props,
            ["PARENT_UNIQUE_NAME", "HIERARCHY_UNIQUE_NAME"]
        );
        assert!(axis.include_all);
        assert!(!axis.all_only);
        assert_eq!(
            t.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
        assert_eq!(
            t.cell_dax.as_deref(),
            Some(r#"EVALUATE SUMMARIZECOLUMNS('vtest_product'[Color], "Value", [TotalAmount])"#)
        );
        assert_eq!(
            t.total_dax.as_deref(),
            Some("EVALUATE { CALCULATE([TotalAmount]) }")
        );
    }

    #[test]
    fn translate_all_children_enumerates_leaf_members() {
        let q = parse_mdx(
            "SELECT {AddCalculatedMembers({[Product].[Color].[All].Children})} \
             DIMENSION PROPERTIES MEMBER_TYPE ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES CELL_ORDINAL",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(axis.level, "Color");
        assert_eq!(axis.dax_column, "Product[Color]");
        assert!(!axis.all_only, "all_only must be false for .Children");
        assert!(
            !axis.include_all,
            "include_all must be false: .Children excludes the All member"
        );
        assert_eq!(
            t.cell_dax.as_deref(),
            Some("EVALUATE VALUES('Product'[Color])")
        );
        assert!(t.total_dax.is_none());
    }

    #[test]
    fn translate_leaf_key_children_yields_empty_axis() {
        let q = parse_mdx(
            "SELECT {AddCalculatedMembers({[Product].[Color].&[Blue].Children})} \
             DIMENSION PROPERTIES MEMBER_TYPE ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES CELL_ORDINAL",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert!(
            axis.all_only,
            "all_only must be true: leaf members have no children"
        );
        assert!(
            !axis.include_all,
            "include_all must be false: no All member emitted"
        );
        assert!(t.cell_dax.is_none(), "no DAX query should be generated");
    }

    #[test]
    fn translate_q7_dimension_all_slicer_ignored() {
        let q = parse_mdx(
            "SELECT NON EMPTY {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME, HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([vtest_product].[ProductType].[All],[Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(
            t.cell_dax.as_deref(),
            Some(r#"EVALUATE SUMMARIZECOLUMNS('vtest_product'[Color], "Value", [TotalAmount])"#)
        );
        assert_eq!(measure_name.as_deref(), Some("TotalAmount"));
        assert_eq!(
            axis.dim_props,
            ["PARENT_UNIQUE_NAME", "HIERARCHY_UNIQUE_NAME"]
        );
        assert_eq!(
            t.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
        assert!(t.non_empty);
    }

    #[test]
    fn translate_q8_all_only_no_measure() {
        let q = parse_mdx(
            "SELECT {AddCalculatedMembers({[vtest_product].[ProductType].[(All)].Members})} \
             DIMENSION PROPERTIES MEMBER_TYPE ON COLUMNS \
             FROM [Model] \
             CELL PROPERTIES CELL_ORDINAL",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert!(axis.all_only);
        assert!(measure_name.is_none());
        assert!(t.cell_dax.is_none());
        assert!(t.total_dax.is_none());
        assert_eq!(axis.table, "vtest_product");
        assert_eq!(axis.hier, "ProductType");
        assert_eq!(axis.dax_column, "vtest_product[ProductType]");
        assert_eq!(axis.dim_props, ["MEMBER_TYPE"]);
        assert!(axis.include_all);
        assert!(!t.non_empty);
        assert_eq!(t.cell_props, ["CELL_ORDINAL"]);
    }

    #[test]
    fn translate_q9_key_filter_slicer() {
        let q = parse_mdx(
            "SELECT NON EMPTY {Hierarchize({[vtest_product].[Color].[Color].AllMembers})} \
             DIMENSION PROPERTIES PARENT_UNIQUE_NAME, HIERARCHY_UNIQUE_NAME ON COLUMNS \
             FROM [Model] \
             WHERE ([vtest_product].[ProductType].&[Widget],[Measures].[TotalAmount]) \
             CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        )
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(measure_name.as_deref(), Some("TotalAmount"));
        assert!(t.non_empty);
        assert_eq!(
            axis.dim_props,
            ["PARENT_UNIQUE_NAME", "HIERARCHY_UNIQUE_NAME"]
        );
        assert_eq!(axis.dax_column, "vtest_product[Color]");
        assert_eq!(
            t.cell_props,
            [
                "VALUE",
                "FORMAT_STRING",
                "LANGUAGE",
                "BACK_COLOR",
                "FORE_COLOR",
                "FONT_FLAGS"
            ]
        );
        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('vtest_product'[Color], ",
                "FILTER(ALL('vtest_product'[ProductType]), 'vtest_product'[ProductType] = \"Widget\"), ",
                "\"Value\", [TotalAmount])"
            ))
        );
        assert_eq!(
            t.total_dax.as_deref(),
            Some(concat!(
                "EVALUATE { CALCULATE([TotalAmount], ",
                "'vtest_product'[ProductType] = \"Widget\") }"
            ))
        );
    }

    #[test]
    fn translate_two_hier_crossjoin_drilldown() {
        let q = parse_mdx(concat!(
            "SELECT NON EMPTY Hierarchize(DrilldownMember(",
            "CrossJoin(",
            "{[vtest_product].[Color].[All],[vtest_product].[Color].[Color].AllMembers},",
            "{([vtest_product].[ProductType].[All])}",
            "),",
            "[vtest_product].[Color].[Color].AllMembers, [vtest_product].[ProductType]",
            "))",
            " DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS",
            " FROM [Model]",
            " CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::TwoHierDim { ref axis, ref measure_name } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(t.cube, "Model");
        assert_eq!(axis.table, "vtest_product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(axis.level, "Color");
        assert!(axis.second_hier.is_some());
        assert!(measure_name.is_none());
        let sh = axis.second_hier.as_ref().unwrap();
        assert_eq!(sh.table, "vtest_product");
        assert_eq!(sh.hier, "ProductType");
        assert_eq!(sh.level, "ProductType");
        assert_eq!(sh.dax_column, "vtest_product[ProductType]");
        assert_eq!(
            t.cell_dax.as_deref(),
            Some("EVALUATE SUMMARIZECOLUMNS('vtest_product'[Color], 'vtest_product'[ProductType])")
        );
        assert!(t.total_dax.is_none());
        assert!(t.non_empty);
    }

    #[test]
    fn translate_generate_ascendants_axis() {
        let q = parse_mdx(concat!(
            "WITH MEMBER [Measures].cChildren As ",
            "'AddCalculatedMembers([Product].[Color].currentmember.children).count' ",
            "Set FilteredMembers As '{[Product].[Color].&[Blue]}' ",
            "Select {[Measures].cChildren} on ROWS, ",
            "Hierarchize(Generate(FilteredMembers, Ascendants([Product].[Color].currentmember))) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME, MEMBER_TYPE ON COLUMNS FROM [Model]",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, has_measure_axis } = t.shape else {
            panic!("wrong shape")
        };

        assert_eq!(t.cube, "Model");
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(axis.level, "Color");
        assert_eq!(axis.dax_column, "Product[Color]");
        assert!(axis.include_all);
        assert!(!axis.all_only);
        assert_eq!(axis.dim_props, ["PARENT_UNIQUE_NAME", "MEMBER_TYPE"]);
        assert_eq!(measure_name.as_deref(), Some("cChildren"));
        assert!(
            has_measure_axis,
            "Generate/Ascendants puts measure on ROWS → has_measure_axis must be true"
        );

        let measure_dax = "IF(ISINSCOPE('Product'[Color]), 0, COUNTROWS(VALUES('Product'[Color])))";
        let expected_cell = format!(
            "EVALUATE SUMMARIZECOLUMNS('Product'[Color], \
             FILTER(ALL('Product'[Color]), 'Product'[Color] = \"Blue\"), \
             \"Value\", {})",
            measure_dax
        );
        let expected_total = format!("EVALUATE {{ CALCULATE({}) }}", measure_dax);
        assert_eq!(t.cell_dax.as_deref(), Some(expected_cell.as_str()));
        assert_eq!(t.total_dax.as_deref(), Some(expected_total.as_str()));
    }

    #[test]
    fn translate_drilldown_where_measure_has_no_measure_axis() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMAmount] = SUM('Sales'[Amount]) ",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS ",
            "FROM [Model] WHERE ([Measures].[CIMAmount]) ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, has_measure_axis } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert!(
            !has_measure_axis,
            "slicer measure must not produce a measures axis"
        );
        assert_eq!(measure_name.as_deref(), Some("CIMAmount"));
    }

    #[test]
    fn translate_crossjoin_measures_first_col_matrix() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]},",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "NON EMPTY Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::CrossJoinMatrix {
            ref crossjoin_dim,
            ref plain_dim,
            ref measures,
            measures_first,
            crossjoin_on_rows,
        } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert_eq!(measures[0].0, "CIMSummen af Amount");
        assert_eq!(measures[1].0, "CIMSummen af Quantity");
        assert_eq!(crossjoin_dim.table, "Product");
        assert_eq!(crossjoin_dim.hier, "Color");
        assert_eq!(plain_dim.table, "Product");
        assert_eq!(plain_dim.hier, "ProductType");
        assert!(
            measures_first,
            "measures is LEFT arm → measures_first must be true"
        );
        assert!(
            !crossjoin_on_rows,
            "CrossJoin is on COLUMNS → crossjoin_on_rows must be false"
        );

        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[Color], 'Product'[ProductType], ",
                r#""M0", SUM('Sales'[Amount]), "M1", SUM('Sales'[Quantity]))"#,
            ))
        );
    }

    #[test]
    fn translate_crossjoin_dim_first_clears_flag() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "NON EMPTY Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::CrossJoinMatrix {
            ref crossjoin_dim,
            ref plain_dim,
            ref measures,
            measures_first,
            crossjoin_on_rows,
        } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert_eq!(crossjoin_dim.hier, "Color");
        assert_eq!(plain_dim.hier, "ProductType");
        assert!(
            !measures_first,
            "dim is LEFT arm → measures_first must be false"
        );
        assert!(!crossjoin_on_rows);
    }

    #[test]
    fn translate_crossjoin_measures_first_on_rows() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "NON EMPTY CrossJoin(",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]},",
            "Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)})) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::CrossJoinMatrix {
            ref crossjoin_dim,
            ref plain_dim,
            ref measures,
            measures_first,
            crossjoin_on_rows,
        } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert!(
            measures_first,
            "measures is LEFT arm → measures_first must be true"
        );
        assert!(
            crossjoin_on_rows,
            "CrossJoin is on ROWS → crossjoin_on_rows must be true"
        );
        assert_eq!(
            crossjoin_dim.hier, "ProductType",
            "crossjoin_dim must be CrossJoin dim"
        );
        assert_eq!(plain_dim.hier, "Color", "plain_dim must be simple col dim");

        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[ProductType], 'Product'[Color], ",
                r#""M0", SUM('Sales'[Amount]), "M1", SUM('Sales'[Quantity]))"#,
            ))
        );
    }

    #[test]
    fn translate_crossjoin_dim_first_on_rows() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "NON EMPTY CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::CrossJoinMatrix {
            ref crossjoin_dim,
            ref plain_dim,
            ref measures,
            measures_first,
            crossjoin_on_rows,
        } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert!(
            !measures_first,
            "dim is LEFT arm → measures_first must be false"
        );
        assert!(
            crossjoin_on_rows,
            "CrossJoin is on ROWS → crossjoin_on_rows must be true"
        );
        assert_eq!(
            crossjoin_dim.hier, "ProductType",
            "crossjoin_dim must be CrossJoin dim"
        );
        assert_eq!(plain_dim.hier, "Color", "plain_dim must be simple col dim");
    }

    #[test]
    fn translate_measures_on_col_dim_on_rows_with_filter() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT {[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]} ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "WHERE ([Product].[ProductType].[All]) ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::DimMeasureMatrix { ref dim_axis, ref measures, measures_on_rows } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert_eq!(measures[0].0, "CIMSummen af Amount");
        assert_eq!(measures[1].0, "CIMSummen af Quantity");
        assert!(
            !measures_on_rows,
            "measures on COLUMNS → measures_on_rows must be false"
        );
        assert!(dim_axis.second_hier.is_none(), "single-hier dim on ROWS");
        assert_eq!(dim_axis.table, "Product");
        assert_eq!(dim_axis.hier, "Color");

        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[Color]"),
            "must group by Color: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "must have M0 alias: {dax}");
        assert!(dax.contains(r#""M1""#), "must have M1 alias: {dax}");
        assert!(
            !dax.contains("KEEPFILTERS"),
            "All-level WHERE must not produce a filter: {dax}"
        );
    }

    #[test]
    fn translate_dim_on_col_measures_on_rows_with_filter() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS , ",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]} ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "WHERE ([Product].[Color].&[Red]) ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::DimMeasureMatrix { ref dim_axis, ref measures, measures_on_rows } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "expected 2 matrix measures");
        assert_eq!(measures[0].0, "CIMSummen af Amount");
        assert_eq!(measures[1].0, "CIMSummen af Quantity");
        assert!(
            measures_on_rows,
            "measures on ROWS → measures_on_rows must be true"
        );
        assert_eq!(dim_axis.table, "Product");
        assert_eq!(dim_axis.hier, "ProductType");

        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[ProductType]"),
            "must group by ProductType: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "must have M0 alias: {dax}");
        assert!(dax.contains(r#""M1""#), "must have M1 alias: {dax}");
        assert!(dax.contains("Color"), "must filter by Color: {dax}");
        assert!(dax.contains("Red"), "must filter by Red: {dax}");
    }

    #[test]
    fn translate_integer_where_key_filter_unquoted_in_dax() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS,\n",
            "NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS\n",
            "FROM [Model]\n",
            "WHERE ([Product].[ProductSK].&[2],[Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::TwoDimAxes { ref col_axis, ref row_axis, ref measure_name } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measure_name.as_deref(), Some("CIMSummen af Amount"));
        assert_eq!(col_axis.hier, "ProductType");
        assert_eq!(row_axis.hier, "Color");

        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[ProductSK] = 2"),
            "integer key must be unquoted in DAX filter, got: {dax}"
        );
        assert!(
            !dax.contains("= \"2\""),
            "integer key must NOT be quoted as string, got: {dax}"
        );
    }

    #[test]
    fn translate_two_dim_subquery_from_includes_filter_in_dax() {
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
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::TwoDimAxes { ref col_axis, ref row_axis, ref measure_name } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(t.cube, "Model");
        assert_eq!(col_axis.table, "Product");
        assert_eq!(col_axis.hier, "ProductType");
        assert_eq!(row_axis.table, "Product");
        assert_eq!(row_axis.hier, "Color");
        assert_eq!(measure_name.as_deref(), Some("CIMSummen af Amount"));

        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[ProductType], 'Product'[Color], ",
                "FILTER(ALL('Product'[Color]), 'Product'[Color] = \"Red\"), ",
                r#""Value", SUM('Sales'[Amount]))"#,
            ))
        );
    }

    #[test]
    fn translate_subquery_from_two_members_same_column_uses_in() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS\n",
            "FROM (SELECT ({[Product].[Color].&[Red], [Product].[Color].&[Blue]}) ON COLUMNS FROM [Model])\n",
            "WHERE ([Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, .. } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(t.cube, "Model");
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(measure_name.as_deref(), Some("CIMSummen af Amount"));

        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[Color], ",
                r#"FILTER(ALL('Product'[Color]), 'Product'[Color] IN {"Red", "Blue"}), "#,
                r#""Value", SUM('Sales'[Amount]))"#,
            ))
        );
        assert_eq!(
            t.total_dax.as_deref(),
            Some(
                r#"EVALUATE { CALCULATE(SUM('Sales'[Amount]), 'Product'[Color] IN {"Red", "Blue"}) }"#
            )
        );
    }

    #[test]
    fn translate_gtopt_dual_section_row_axis_resolves_to_detail_level() {
        // Excel's "GTOPT" idiom: a row axis unioning a bare grand-total
        // section ({[Hier].[All]}) with a detail section (Hierarchize over
        // .AllMembers). unwrap_to_member must prefer the detail section so
        // the axis resolves to a real DAX column, not the literal string
        // "All" — this was the exact shape from a real blank-response bug
        // report (a Country-drilldown column axis crossed with a
        // Category grand-total+detail row axis, sliced by a ChannelName
        // member and a WITH MEASURE-defined measure in the WHERE tuple).
        let q = parse_mdx(concat!(
            "WITH MEASURE 'FactSales'[CIMSummen af Quantity] = SUM('FactSales'[Quantity])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[FactSales].[Country].[All]},,,INCLUDE_CALC_MEMBERS)}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS,\n",
            "NON EMPTY\n",
            "{\n",
            "{[DimProduct].[Category].[All]}\n",
            ",\n",
            "{Hierarchize({[DimProduct].[Category].[Category].AllMembers})}\n",
            "}\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS\n",
            "FROM [Model]\n",
            "WHERE ([DimChannel].[ChannelName].&[Store],[Measures].[CIMSummen af Quantity])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::TwoDimAxes { ref col_axis, ref row_axis, ref measure_name } = t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measure_name.as_deref(), Some("CIMSummen af Quantity"));
        assert_eq!(col_axis.table, "FactSales");
        assert_eq!(col_axis.hier, "Country");
        assert_eq!(col_axis.level, "Country");
        assert!(!col_axis.all_only);

        assert_eq!(row_axis.table, "DimProduct");
        assert_eq!(row_axis.hier, "Category");
        assert_eq!(
            row_axis.level, "Category",
            "row axis must resolve to the detail level, not the literal 'All' section"
        );
        assert!(
            !row_axis.all_only,
            "row axis must not collapse to all_only when a detail section is present"
        );

        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'FactSales'[Country]"),
            "must group by Country: {dax}"
        );
        assert!(
            dax.contains("'DimProduct'[Category]"),
            "must group by Category: {dax}"
        );
        assert!(
            !dax.contains("'DimProduct'[All]"),
            "must not reference a literal 'All' column: {dax}"
        );
        assert!(
            dax.contains("'DimChannel'[ChannelName] = \"Store\""),
            "must filter by ChannelName: {dax}"
        );
        assert!(
            dax.contains("SUM('FactSales'[Quantity])"),
            "must substitute the WITH MEASURE expression: {dax}"
        );
    }

    #[test]
    fn translate_subquery_from_adds_key_filter_to_dax() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount])\n",
            "SELECT NON EMPTY Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)})\n",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS\n",
            "FROM (SELECT ({[Product].[Color].&[Red]}) ON COLUMNS FROM [Model])\n",
            "WHERE ([Measures].[CIMSummen af Amount])\n",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleDim { ref axis, ref measure_name, has_measure_axis } = t.shape else {
            panic!("wrong shape")
        };
        assert_eq!(t.cube, "Model");
        assert_eq!(axis.table, "Product");
        assert_eq!(axis.hier, "Color");
        assert_eq!(measure_name.as_deref(), Some("CIMSummen af Amount"));
        assert!(!has_measure_axis, "measure is in WHERE slicer, not on axis");

        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[Color], ",
                "FILTER(ALL('Product'[Color]), 'Product'[Color] = \"Red\"), ",
                r#""Value", SUM('Sales'[Amount]))"#,
            ))
        );
        assert_eq!(
            t.total_dax.as_deref(),
            Some(r#"EVALUATE { CALCULATE(SUM('Sales'[Amount]), 'Product'[Color] = "Red") }"#)
        );
    }

    #[test]
    fn translate_scalar_query_no_axes() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "SELECT FROM [Model] ",
            "WHERE ([Measures].[CIMSummen af Amount])",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::Scalar { ref measure_name } = t.shape else {
            panic!("wrong shape")
        };

        assert_eq!(measure_name.as_str(), "CIMSummen af Amount");
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.starts_with("EVALUATE { CALCULATE("),
            "must use EVALUATE {{ CALCULATE }}: {dax}"
        );
        assert!(
            dax.contains("SUM('Sales'[Amount])"),
            "must embed the measure expr: {dax}"
        );
    }

    #[test]
    fn translate_scalar_query_with_key_filter() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[Amount] = SUM('Sales'[Amount]) ",
            "SELECT FROM [Model] ",
            "WHERE ([Product].[Color].&[Red], [Measures].[Amount])",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        assert!(
            matches!(t.shape, QueryShape::Scalar { .. }),
            "must be Scalar"
        );
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(dax.contains("Color"), "must filter by Color: {dax}");
        assert!(dax.contains("Red"), "must filter by Red: {dax}");
    }

    #[test]
    fn translate_measures_only_columns() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[Qty] = SUM('Sales'[Quantity]) ",
            "SELECT {[Measures].[Amount],[Measures].[Qty]} ON COLUMNS ",
            "FROM [Model]",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::MeasuresOnly { ref measures } = t.shape else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2, "must have 2 measures");
        assert_eq!(measures[0].0, "Amount");
        assert_eq!(measures[1].0, "Qty");

        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.starts_with("EVALUATE ROW("),
            "must use EVALUATE ROW: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "must have M0 alias: {dax}");
        assert!(dax.contains(r#""M1""#), "must have M1 alias: {dax}");
    }

    #[test]
    fn translate_single_axis_crossjoin_on_columns() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleAxisCrossJoin { ref dim_axis, ref measures, measures_first } =
            t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2);
        assert_eq!(measures[0].0, "CIMSummen af Amount");
        assert_eq!(measures[1].0, "CIMSummen af Quantity");
        assert!(!measures_first, "dim is LEFT arm → measures_first false");
        assert_eq!(dim_axis.table, "Product");
        assert_eq!(dim_axis.hier, "Color");
        assert_eq!(dim_axis.level, "Color");
        assert!(t.non_empty);
        assert_eq!(
            t.cell_dax.as_deref(),
            Some(concat!(
                "EVALUATE SUMMARIZECOLUMNS('Product'[Color], ",
                r#""M0", SUM('Sales'[Amount]), "M1", SUM('Sales'[Quantity]))"#,
            ))
        );
    }

    #[test]
    fn translate_single_axis_crossjoin_subquery_filter() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS ",
            "FROM (SELECT ({[Product].[Color].[All], [Product].[Color].&[Blue]}) ON COLUMNS FROM [Model]) ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        assert!(matches!(t.shape, QueryShape::SingleAxisCrossJoin { .. }));
        let QueryShape::SingleAxisCrossJoin { ref measures, .. } = t.shape else {
            unreachable!()
        };
        assert_eq!(measures.len(), 2);
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("FILTER"),
            "subquery key filter must appear in DAX: {dax}"
        );
        assert!(dax.contains("Blue"), "filter must reference Blue: {dax}");
    }

    #[test]
    fn translate_single_axis_crossjoin_on_rows() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[CIMSummen af Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[CIMSummen af Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[CIMSummen af Amount],[Measures].[CIMSummen af Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON ROWS ",
            "FROM [Model] ",
            "CELL PROPERTIES VALUE, FORMAT_STRING, LANGUAGE, BACK_COLOR, FORE_COLOR, FONT_FLAGS",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleAxisCrossJoin { ref dim_axis, ref measures, measures_first } =
            t.shape
        else {
            panic!("wrong shape")
        };

        assert_eq!(measures.len(), 2);
        assert!(!measures_first, "dim is LEFT arm → measures_first false");
        assert_eq!(dim_axis.table, "Product");
        assert_eq!(dim_axis.hier, "Color");
        assert_eq!(dim_axis.level, "Color");
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[Color]"),
            "must group by Color: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "must have M0 alias: {dax}");
    }

    #[test]
    fn translate_multi_dim_crossjoin_col_meas_col() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(CrossJoin(",
            "Hierarchize({DrilldownLevel({[Product].[Color].[All]},,,INCLUDE_CALC_MEMBERS)}),",
            "{[Measures].[Amount],[Measures].[Quantity]}),",
            "Hierarchize({DrilldownLevel({[Product].[ProductType].[All]},,,INCLUDE_CALC_MEMBERS)})) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS ",
            "FROM [Model]",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleAxisMultiDimCrossJoin { ref dims, ref measures, measures_position } =
            t.shape
        else {
            panic!("wrong shape: {:?}", t.shape)
        };
        assert_eq!(dims.len(), 2);
        assert_eq!(measures.len(), 2);
        assert_eq!(measures_position, 1, "measures appear after dim[0] Color");
        assert_eq!(dims[0].table, "Product");
        assert_eq!(dims[0].hier, "Color");
        assert_eq!(dims[1].table, "Product");
        assert_eq!(dims[1].hier, "ProductType");
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[Color]"),
            "DAX must group by Color: {dax}"
        );
        assert!(
            dax.contains("'Product'[ProductType]"),
            "DAX must group by ProductType: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "DAX must have M0 alias: {dax}");
    }

    #[test]
    fn translate_single_axis_two_hier_crossjoin() {
        let q = parse_mdx(concat!(
            "WITH MEASURE 'Sales'[Amount] = SUM('Sales'[Amount]) ",
            "MEASURE 'Sales'[Quantity] = SUM('Sales'[Quantity]) ",
            "SELECT NON EMPTY CrossJoin(",
            "Hierarchize(DrilldownMember(",
            "CrossJoin({[Product].[Color].[All],[Product].[Color].[Color].AllMembers},",
            "{([Product].[ProductType].[All])}),",
            "[Product].[Color].[Color].AllMembers,[Product].[ProductType])),",
            "{[Measures].[Amount],[Measures].[Quantity]}) ",
            "DIMENSION PROPERTIES PARENT_UNIQUE_NAME,HIERARCHY_UNIQUE_NAME ON COLUMNS ",
            "FROM [Model]",
        ))
        .unwrap();
        let t = mdx_to_dax(&q).unwrap();
        let QueryShape::SingleAxisCrossJoin { ref dim_axis, ref measures, measures_first } =
            t.shape
        else {
            panic!("wrong shape: {:?}", t.shape)
        };
        assert!(!measures_first, "dim is left arm → measures_first false");
        assert_eq!(measures.len(), 2);
        assert_eq!(dim_axis.table, "Product");
        assert_eq!(dim_axis.hier, "Color");
        assert!(
            dim_axis.second_hier.is_some(),
            "must detect ProductType as second_hier"
        );
        let sh = dim_axis.second_hier.as_ref().unwrap();
        assert_eq!(sh.table, "Product");
        assert_eq!(sh.hier, "ProductType");
        let dax = t.cell_dax.as_deref().unwrap();
        assert!(
            dax.contains("'Product'[Color]"),
            "must group by Color: {dax}"
        );
        assert!(
            dax.contains("'Product'[ProductType]"),
            "must group by ProductType: {dax}"
        );
        assert!(dax.contains(r#""M0""#), "must have M0 alias: {dax}");
    }
}
