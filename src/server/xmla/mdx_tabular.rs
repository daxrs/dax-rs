//! MDX-specific adapter mapping `crate::mdxtranslator`'s per-axis
//! `EvaluatedSet` representation into a `super::tabular::TabularResponse`.

use super::tabular::{DimColumnPlan, MeasureColumnPlan, TabularResponse};

use crate::mdxtranslator::daxgen::distinct_measure_names;
use crate::mdxtranslator::eval::{combine_sets, EvaluatedSet, HierarchyRef, Member};
use crate::mdxtranslator::{AxisTranslation, TranslatedQuery};
use crate::server::provider::QueryResult;
use std::collections::HashMap;

fn combined_shape(axes: &[AxisTranslation]) -> Option<EvaluatedSet> {
    let mut combined: Option<EvaluatedSet> = None;
    for axis in axes {
        combined = Some(match combined {
            None => axis.set.clone(),
            Some(acc) => combine_sets(acc, axis.set.clone()),
        });
    }
    combined
}

pub fn build_response(
    translated: &TranslatedQuery,
    result: &QueryResult,
) -> Result<TabularResponse, String> {
    let mut dim_axes: Vec<&AxisTranslation> = Vec::new();
    let mut measures_axis: Option<&AxisTranslation> = None;
    for axis in &translated.axes {
        let has_measures = axis
            .set
            .shape
            .iter()
            .any(|h| matches!(h, HierarchyRef::Measures));
        if has_measures {
            if axis.set.shape.len() != 1 {
                return Err(
                    "Format=Tabular does not yet support an axis mixing Measures with a dimension"
                        .to_string(),
                );
            }
            if measures_axis.is_some() {
                return Err(
                    "Format=Tabular does not yet support Measures on more than one axis"
                        .to_string(),
                );
            }
            measures_axis = Some(axis);
        } else {
            dim_axes.push(axis);
        }
    }

    let col_index: HashMap<&str, usize> = result
        .columns
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (name.as_str(), i))
        .collect();

    let mut dim_plans: Vec<DimColumnPlan> = Vec::new();
    for axis in &dim_axes {
        for href in &axis.set.shape {
            let HierarchyRef::Dimension { table, hier } = href else {
                continue;
            };
            let hier_uname = format!("[{table}].[{hier}]");
            let qualified = format!("{table}[{hier}]");
            let result_col = *col_index
                .get(qualified.as_str())
                .ok_or_else(|| format!("column '{qualified}' not found in DAX result"))?;
            for prop in &axis.dim_props {
                if prop == "MEMBER_UNIQUE_NAME" || prop == "MEMBER_CAPTION" {
                    dim_plans.push(DimColumnPlan {
                        hier_uname: hier_uname.clone(),
                        field: format!("{hier_uname}.[{hier}].[{prop}]"),
                        prop: prop.clone(),
                        result_col,
                    });
                }
            }
        }
    }

    let mut measure_plans: Vec<MeasureColumnPlan> = Vec::new();
    if let Some(axis) = measures_axis {
        let combined =
            combined_shape(&translated.axes).ok_or_else(|| "no combined axis shape".to_string())?;
        let measure_position = combined
            .shape
            .iter()
            .position(|h| matches!(h, HierarchyRef::Measures))
            .ok_or_else(|| "no measure hierarchy in combined shape".to_string())?;
        let sorted_names = distinct_measure_names(&combined, measure_position);
        for t in &axis.set.tuples {
            let Member::Measure { name } = &t.members[0] else {
                return Err("expected a measure tuple on the measures axis".to_string());
            };
            let m_index = sorted_names
                .iter()
                .position(|n| n == name)
                .ok_or_else(|| format!("measure '{name}' not resolved in generated DAX"))?;
            let dax_col = format!("[M{m_index}]");
            let result_col = *col_index
                .get(dax_col.as_str())
                .ok_or_else(|| format!("column '{dax_col}' not found in DAX result"))?;
            measure_plans.push(MeasureColumnPlan { name: name.clone(), result_col });
        }
    }

    let mut response = TabularResponse::default();
    for p in &dim_plans {
        response.add_column(p.field.clone(), Some("string"));
    }
    for p in &measure_plans {
        response.add_column(format!("[Measures].[{}]", p.name), None);
    }

    response.set_rows(&result.rows, &dim_plans, &measure_plans)?;

    Ok(response)
}
