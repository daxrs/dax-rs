//! MDX-specific adapter mapping `crate::mdxtranslator`'s per-axis
//! `EvaluatedSet` representation into a `super::cellset::CellsetResponse`.
//!
//! Scope (first pass): at most two non-slicer axes (Axis0/Axis1). Measures
//! must be resolvable to exactly one set of names, either because one axis
//! carries the Measures hierarchy as its entire shape (same boundary
//! `mdx_tabular` draws), or because no axis does and the `WHERE`-clause
//! slicer supplies a single measure instead (the common GTOPT grand-total
//! shape: a dimension-only axis, measure only in the slicer) — mirroring
//! exactly what `daxgen::generate_dax` already does internally via
//! `slicer::slicer_measure_names`. Two further simplifications, not yet
//! validated against a real capture:
//!   - `SlicerAxis` always renders empty; the WHERE-clause slicer's actual
//!     member context isn't reflected yet.
//!   - `DISPLAY_INFO` is approximated per-tuple (last tuple on an axis gets
//!     the "last child" bit) rather than tracked per-hierarchy-position
//!     within crossjoin tuples, which is what real Fabric's `NormTupleSet`
//!     captures show for multi-hierarchy axes.
//!
//! `NON EMPTY` is handled here, not in `mdxtranslator::eval` — a tuple on an
//! axis marked non-empty is only kept if it combines with at least one tuple
//! from every other axis to produce a real DAX row. `eval_set` itself stays
//! unaware of emptiness (that's a data property, not a pure MDX-evaluation
//! one), matching the split `mdx_tabular` already keeps.

use super::cellset::{
    CellProp, CellsetAxis, CellsetCell, CellsetHierarchyInfo, CellsetMember, CellsetResponse,
    CellsetTuple,
};
use super::xml_util::member_unique_name;

use crate::mdxtranslator::ast::AxisId;
use crate::mdxtranslator::daxgen::distinct_measure_names;
use crate::mdxtranslator::eval::{combine_sets, EvaluatedSet, HierarchyRef, Member, Tuple};
use crate::mdxtranslator::slicer::slicer_measure_names;
use crate::mdxtranslator::{AxisTranslation, TranslatedQuery};
use crate::server::provider::QueryResult;
use std::collections::HashMap;

fn axis_number(id: &AxisId) -> u32 {
    match id {
        AxisId::Columns => 0,
        AxisId::Rows => 1,
        AxisId::Pages => 2,
        AxisId::Chapters => 3,
        AxisId::Sections => 4,
        AxisId::Index(n) => *n,
    }
}

fn combined_shape(axes: &[AxisTranslation]) -> EvaluatedSet {
    let identity = EvaluatedSet {
        shape: Vec::new(),
        tuples: vec![Tuple { members: Vec::new() }],
    };
    axes.iter()
        .fold(identity, |acc, axis| combine_sets(acc, axis.set.clone()))
}

fn hier_uname_of(table: &str, hier: &str) -> String {
    format!("[{table}].[{hier}]")
}

/// The "row identity" value for a member as it appears in a DAX result
/// column: blank for the All/rollup member, the leaf key otherwise. Matches
/// the convention `mdx_tabular::member_unique_name` relies on. Ambiguous by
/// itself when the same position can hold both variants (see `dim_key`).
fn dim_value_of(member: &Member) -> Option<&str> {
    match member {
        Member::All { .. } => Some(""),
        Member::Leaf { key, .. } => Some(key.as_str()),
        Member::Measure { .. } => None,
    }
}

fn measure_name_of(member: &Member) -> Option<&str> {
    match member {
        Member::Measure { name } => Some(name.as_str()),
        _ => None,
    }
}

/// Maps the raw `CELL PROPERTIES` token list off the MDX statement to the
/// typed `CellProp`s `CellsetResponse` renders. Unrecognized tokens are
/// dropped rather than rejected - real clients occasionally ask for
/// properties we don't model (e.g. `FORE_COLOR` variants), and the existing
/// XMLA handlers ignore rather than error on those too.
fn map_cell_props(raw: &[String]) -> Vec<CellProp> {
    raw.iter()
        .filter_map(|p| match p.to_ascii_uppercase().as_str() {
            "VALUE" => Some(CellProp::Value),
            "FORMATTED_VALUE" => Some(CellProp::FormattedValue),
            "FORMAT_STRING" => Some(CellProp::FormatString),
            "LANGUAGE" => Some(CellProp::Language),
            "BACK_COLOR" => Some(CellProp::BackColor),
            "FORE_COLOR" => Some(CellProp::ForeColor),
            "FONT_FLAGS" => Some(CellProp::FontFlags),
            "CELL_ORDINAL" => Some(CellProp::CellOrdinal),
            _ => None,
        })
        .collect()
}

/// A dimension hierarchy's DAX result column, plus - when
/// `daxgen::generate_dax` rolled it up via `ROLLUPADDISSUBTOTAL` because the
/// combined axis shape mixes `Member::All` with `Member::Leaf` at this
/// position - the result column of the accompanying `_IsTotal_N` flag.
/// Needed because a rollup/grand-total row and a genuine blank-key leaf row
/// both show up as a blank dimension value; only the flag tells them apart.
struct DimResultCol {
    table: String,
    hier: String,
    value_col: usize,
    is_total_col: Option<usize>,
}

/// A join-key component that can never collide with a real dimension key
/// (leaf keys are real data; this starts with a control character no such
/// key would contain), used to mark "this position is the All/rollup
/// member" distinctly from "this position is a genuinely blank leaf".
const ROLLUP_SENTINEL: &str = "\u{2}rollup";

/// Mirrors `daxgen::classify_groupby_positions`'s mixed-hierarchy detection
/// and `_IsTotal_N` flag-index assignment (0, 1, 2... in shape order,
/// skipping non-mixed positions), so this adapter can look up the exact
/// flag column `generate_dax` produced for a given dimension position.
fn rollup_flag_cols(combined: &EvaluatedSet) -> Vec<Option<String>> {
    let mut flag_idx = 0usize;
    combined
        .shape
        .iter()
        .enumerate()
        .map(|(i, href)| {
            if matches!(href, HierarchyRef::Measures) {
                return None;
            }
            let all_all = combined
                .tuples
                .iter()
                .all(|t| matches!(t.members[i], Member::All { .. }));
            let all_leaf = combined
                .tuples
                .iter()
                .all(|t| matches!(t.members[i], Member::Leaf { .. }));
            if all_all || all_leaf {
                return None;
            }
            let flag = format!("_IsTotal_{flag_idx}");
            flag_idx += 1;
            Some(flag)
        })
        .collect()
}

/// Builds the composite join key used to match a combination of members
/// (drawn from across all axes) back to the one DAX result row that
/// computed it: each dimension hierarchy's value, in a fixed column order,
/// joined by a separator that can't appear in a real key.
fn dim_key(combined_members: &[&Member], dim_result_cols: &[DimResultCol]) -> String {
    dim_result_cols
        .iter()
        .map(|d| {
            let member = combined_members.iter().copied().find(|m| match m {
                Member::All { table, hier } => table == &d.table && hier == &d.hier,
                Member::Leaf { table, hier, .. } => table == &d.table && hier == &d.hier,
                Member::Measure { .. } => false,
            });
            match member {
                Some(Member::All { .. }) if d.is_total_col.is_some() => ROLLUP_SENTINEL.to_string(),
                Some(m) => dim_value_of(m).unwrap_or("").to_string(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("\u{1}")
}

/// Drops tuples that don't combine with any tuple from `other_tuples` to
/// produce a real DAX row — the effect `NON EMPTY` has on an axis's own
/// member list (as distinct from just leaving gaps in `CellData`).
fn filter_non_empty(
    tuples: &[Tuple],
    other_tuples: &[Tuple],
    dim_result_cols: &[DimResultCol],
    row_by_dim_key: &HashMap<String, usize>,
) -> Vec<Tuple> {
    tuples
        .iter()
        .filter(|t| {
            other_tuples.iter().any(|o| {
                let combined: Vec<&Member> = t.members.iter().chain(o.members.iter()).collect();
                row_by_dim_key.contains_key(&dim_key(&combined, dim_result_cols))
            })
        })
        .cloned()
        .collect()
}

fn build_hierarchy_infos(
    shape: &[HierarchyRef],
    dim_props: &[String],
) -> Vec<CellsetHierarchyInfo> {
    let has_parent = dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("PARENT_UNIQUE_NAME"));
    let has_hier = dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("HIERARCHY_UNIQUE_NAME"));
    let has_member_type = dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("MEMBER_TYPE"));

    shape
        .iter()
        .map(|h| match h {
            HierarchyRef::Dimension { table, hier } => CellsetHierarchyInfo {
                hier_uname: hier_uname_of(table, hier),
                has_parent_unique_name: has_parent,
                has_hierarchy_unique_name: has_hier,
                has_member_type,
            },
            // Matches old handlers.rs's build_measures_hier_info: Measures
            // unconditionally declares PARENT_UNIQUE_NAME/HIERARCHY_UNIQUE_NAME
            // regardless of the requested dim_props.
            HierarchyRef::Measures => CellsetHierarchyInfo {
                hier_uname: "[Measures]".to_string(),
                has_parent_unique_name: true,
                has_hierarchy_unique_name: true,
                has_member_type: false,
            },
        })
        .collect()
}

fn cellset_member_from(
    member: &Member,
    display_info: u32,
    include_parent: bool,
    include_hierarchy: bool,
    include_member_type: bool,
) -> CellsetMember {
    match member {
        Member::All { table, hier } => {
            let hier_uname = hier_uname_of(table, hier);
            CellsetMember {
                uname: format!("{hier_uname}.[All]"),
                caption: "All".to_string(),
                lname: format!("{hier_uname}.[(All)]"),
                lnum: 0,
                display_info,
                parent_uname: None, // the All member has no parent
                hierarchy_uname: include_hierarchy.then(|| hier_uname.clone()),
                member_type: include_member_type.then_some(2u8),
            }
        }
        Member::Leaf { table, hier, level, key, caption } => {
            let hier_uname = hier_uname_of(table, hier);
            CellsetMember {
                uname: member_unique_name(&hier_uname, key),
                caption: caption.clone(),
                lname: format!("{hier_uname}.[{level}]"),
                lnum: 1,
                display_info,
                parent_uname: include_parent.then(|| format!("{hier_uname}.[All]")),
                hierarchy_uname: include_hierarchy.then(|| hier_uname.clone()),
                member_type: include_member_type.then_some(1u8),
            }
        }
        Member::Measure { name } => CellsetMember {
            uname: format!("[Measures].[{name}]"),
            caption: name.clone(),
            lname: "[Measures].[MeasuresLevel]".to_string(),
            lnum: 0,
            display_info,
            parent_uname: None,
            hierarchy_uname: Some("[Measures]".to_string()),
            member_type: None, // old code never emits MEMBER_TYPE for measures
        },
    }
}

fn build_axis(axis: &AxisTranslation, tuples: &[Tuple]) -> CellsetAxis {
    let hierarchies = build_hierarchy_infos(&axis.set.shape, &axis.dim_props);
    let has_all = tuples
        .iter()
        .any(|t| t.members.iter().any(|m| matches!(m, Member::All { .. })));
    let last = tuples.len().saturating_sub(1);
    let has_parent = axis
        .dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("PARENT_UNIQUE_NAME"));
    let has_hier = axis
        .dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("HIERARCHY_UNIQUE_NAME"));
    let has_member_type = axis
        .dim_props
        .iter()
        .any(|p| p.eq_ignore_ascii_case("MEMBER_TYPE"));

    let cellset_tuples = tuples
        .iter()
        .enumerate()
        .map(|(i, tuple)| {
            let is_last_tuple = i == last;
            let members = tuple
                .members
                .iter()
                .map(|m| {
                    let display_info = match m {
                        Member::All { .. } => {
                            if has_all && tuples.len() > 1 {
                                66536
                            } else {
                                1000
                            }
                        }
                        _ => {
                            if is_last_tuple {
                                131072
                            } else {
                                0
                            }
                        }
                    };
                    cellset_member_from(m, display_info, has_parent, has_hier, has_member_type)
                })
                .collect();
            CellsetTuple { members }
        })
        .collect();

    CellsetAxis {
        name: format!("Axis{}", axis_number(&axis.id)),
        hierarchies,
        tuples: cellset_tuples,
    }
}

pub fn build_response(
    translated: &TranslatedQuery,
    result: &QueryResult,
    cube_name: &str,
    last_data_update: &str,
    last_schema_update: &str,
) -> Result<CellsetResponse, String> {
    let mut axes: Vec<&AxisTranslation> = translated.axes.iter().collect();
    axes.sort_by_key(|a| axis_number(&a.id));

    if axes.len() > 2 {
        return Err("Format=Multidimensional does not yet support more than two axes".to_string());
    }

    let mut measures_axis_pos: Option<usize> = None;
    for (i, axis) in axes.iter().enumerate() {
        let has_measures = axis
            .set
            .shape
            .iter()
            .any(|h| matches!(h, HierarchyRef::Measures));
        if has_measures {
            if measures_axis_pos.is_some() {
                return Err(
                    "Format=Multidimensional does not yet support Measures on more than one axis"
                        .to_string(),
                );
            }
            measures_axis_pos = Some(i);
        }
    }
    // Classification only: confirms at most one axis carries Measures. An
    // axis may freely crossjoin Measures with a dimension - every function
    // below (build_hierarchy_infos, cellset_member_from, build_axis's
    // DisplayInfo heuristic, dim_key/filter_non_empty, the cell-placement
    // loop) already treats shape positions generically and skips Measures
    // when building dimension join keys, so no position/axis-exclusivity
    // assumption needs to hold. Cell placement below reads the measure
    // straight out of each combined tuple when an axis carries it; when none
    // does (measures_axis_pos == None), every cell falls back to the single
    // WHERE-clause slicer measure resolved below, matching daxgen's own
    // fallback.
    let _measures_axis_pos = measures_axis_pos;

    let col_index: HashMap<&str, usize> = result
        .columns
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();

    let combined = combined_shape(&translated.axes);
    let measure_position = combined
        .shape
        .iter()
        .position(|h| matches!(h, HierarchyRef::Measures));
    let sorted_measure_names = match measure_position {
        Some(pos) => distinct_measure_names(&combined, pos),
        None => slicer_measure_names(&translated.slicer)?,
    };
    if sorted_measure_names.is_empty() {
        return Err(
            "Format=Multidimensional requires Measures on an axis or in the WHERE clause"
                .to_string(),
        );
    }

    // Every distinct dimension hierarchy across the whole query (both axes),
    // each mapped to its DAX result column - the join key used to match a
    // combined axis-tuple position back to the one DAX row that computed it.
    let flag_cols = rollup_flag_cols(&combined);
    let mut dim_result_cols: Vec<DimResultCol> = Vec::new();
    for (i, href) in combined.shape.iter().enumerate() {
        if let HierarchyRef::Dimension { table, hier } = href {
            let qualified = format!("{table}[{hier}]");
            let value_col = *col_index
                .get(qualified.as_str())
                .ok_or_else(|| format!("column '{qualified}' not found in DAX result"))?;
            let is_total_col = flag_cols[i]
                .as_ref()
                .map(|flag| {
                    col_index
                        .get(format!("[{flag}]").as_str())
                        .copied()
                        .ok_or_else(|| format!("column '[{flag}]' not found in DAX result"))
                })
                .transpose()?;
            dim_result_cols.push(DimResultCol {
                table: table.clone(),
                hier: hier.clone(),
                value_col,
                is_total_col,
            });
        }
    }

    let mut row_by_dim_key: HashMap<String, usize> = HashMap::new();
    for (i, row) in result.rows.iter().enumerate() {
        let key = dim_result_cols
            .iter()
            .map(|d| {
                let raw = row
                    .get(d.value_col)
                    .and_then(|v| v.clone())
                    .unwrap_or_default();
                match d.is_total_col {
                    Some(flag_col) => {
                        let is_total = row.get(flag_col).and_then(|v| v.as_deref()) == Some("true");
                        if is_total {
                            ROLLUP_SENTINEL.to_string()
                        } else {
                            raw
                        }
                    }
                    None => raw,
                }
            })
            .collect::<Vec<_>>()
            .join("\u{1}");
        row_by_dim_key.insert(key, i);
    }

    let empty: Vec<Tuple> = Vec::new();
    let axis0_full: &[Tuple] = axes
        .first()
        .map(|a| a.set.tuples.as_slice())
        .unwrap_or(&empty);
    let axis1_full: &[Tuple] = axes
        .get(1)
        .map(|a| a.set.tuples.as_slice())
        .unwrap_or(&empty);
    // A single-tuple-with-no-members placeholder to combine against when
    // there's no second axis, so filter_non_empty's "does this tuple
    // combine with something to produce a real DAX row" check degenerates
    // to "does this tuple alone match a row" - needed now that a lone axis
    // can be a pure dimension axis (measure resolved from the WHERE-clause
    // slicer instead), not just always the Measures axis.
    let no_other_axis = [Tuple { members: Vec::new() }];

    let axis0_tuples: Vec<Tuple> = match axes.first() {
        Some(a) if a.non_empty && axes.len() > 1 => {
            filter_non_empty(axis0_full, axis1_full, &dim_result_cols, &row_by_dim_key)
        }
        Some(a) if a.non_empty => filter_non_empty(
            axis0_full,
            &no_other_axis,
            &dim_result_cols,
            &row_by_dim_key,
        ),
        Some(_) => axis0_full.to_vec(),
        None => vec![Tuple { members: Vec::new() }],
    };
    let axis1_tuples: Vec<Tuple> = match axes.get(1) {
        Some(a) if a.non_empty => {
            filter_non_empty(axis1_full, axis0_full, &dim_result_cols, &row_by_dim_key)
        }
        Some(_) => axis1_full.to_vec(),
        None => Vec::new(),
    };

    let mut response = CellsetResponse::new(cube_name, last_data_update, last_schema_update);
    response.set_cell_props(map_cell_props(&translated.cell_props));
    if let Some(a) = axes.first() {
        response.add_axis(build_axis(a, &axis0_tuples));
    }
    if let Some(a) = axes.get(1) {
        response.add_axis(build_axis(a, &axis1_tuples));
    }
    response.add_axis(CellsetAxis {
        name: "SlicerAxis".to_string(),
        hierarchies: Vec::new(),
        tuples: Vec::new(),
    });

    let empty_tuple = Tuple { members: Vec::new() };
    let mut cells = Vec::new();
    for (i0, t0) in axis0_tuples.iter().enumerate() {
        let iter1: Box<dyn Iterator<Item = (usize, &Tuple)>> = if axes.len() > 1 {
            Box::new(axis1_tuples.iter().enumerate())
        } else {
            Box::new(std::iter::once((0, &empty_tuple)))
        };
        for (i1, t1) in iter1 {
            let ordinal = (i0 + i1 * axis0_tuples.len().max(1)) as u32;

            let combined_members: Vec<&Member> =
                t0.members.iter().chain(t1.members.iter()).collect();

            // Axis-driven when an axis carries Measures; otherwise every
            // cell reads the same single slicer-resolved measure.
            let measure_name = combined_members
                .iter()
                .copied()
                .find_map(measure_name_of)
                .or_else(|| sorted_measure_names.first().map(String::as_str))
                .ok_or_else(|| {
                    "no measure member in combined tuple and no slicer measure".to_string()
                })?;
            let m_index = sorted_measure_names
                .iter()
                .position(|n| n.as_str() == measure_name)
                .ok_or_else(|| format!("measure '{measure_name}' not resolved in generated DAX"))?;
            let measure_col = *col_index
                .get(format!("[M{m_index}]").as_str())
                .ok_or_else(|| format!("column '[M{m_index}]' not found in DAX result"))?;

            let key = dim_key(&combined_members, &dim_result_cols);
            if let Some(&row_idx) = row_by_dim_key.get(&key) {
                let value = result.rows[row_idx]
                    .get(measure_col)
                    .and_then(|v| v.clone());
                cells.push(CellsetCell { ordinal, value });
            }
        }
    }

    cells.sort_by_key(|c| c.ordinal);
    response.set_cells(cells);

    Ok(response)
}
