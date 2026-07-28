use std::collections::HashMap;

use super::ast::{Condition, ConditionOp, ConditionValue};

/// A single schema row: ordered list of (column_name, value) pairs.
pub type Row = Vec<(String, String)>;

/// Keep only the named columns in each row, in the requested order.
/// If `columns` is empty all columns are returned unchanged.
pub fn apply_projection(rows: Vec<Row>, columns: &[String]) -> Vec<Row> {
    if columns.is_empty() {
        return rows;
    }
    rows.into_iter()
        .map(|row| {
            columns
                .iter()
                .map(|col| {
                    let val = row
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case(col))
                        .map(|(_, v)| v.clone())
                        .unwrap_or_default();
                    (col.clone(), val)
                })
                .collect()
        })
        .collect()
}

/// Retain only rows where every condition holds (AND semantics).
/// `params` resolves `@Name` references in condition values.
pub fn apply_conditions(
    rows: Vec<Row>,
    conditions: &[Condition],
    params: &HashMap<String, String>,
) -> Vec<Row> {
    if conditions.is_empty() {
        return rows;
    }
    rows.into_iter()
        .filter(|row| conditions.iter().all(|c| eval_condition(row, c, params)))
        .collect()
}

fn eval_condition(row: &Row, cond: &Condition, params: &HashMap<String, String>) -> bool {
    let cell = row
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&cond.column))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");

    match cond.op {
        ConditionOp::IsTrue => {
            !cell.is_empty() && cell != "0" && !cell.eq_ignore_ascii_case("false")
        }
        ConditionOp::Eq => cell.eq_ignore_ascii_case(&resolve_value(&cond.value, params)),
        ConditionOp::Ne => !cell.eq_ignore_ascii_case(&resolve_value(&cond.value, params)),
    }
}

fn resolve_value(value: &ConditionValue, params: &HashMap<String, String>) -> String {
    match value {
        ConditionValue::Literal(s) => s.clone(),
        ConditionValue::Param(name) => params.get(name).cloned().unwrap_or_default(),
    }
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::mdx::ast::{ConditionOp, ConditionValue};

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn cond(column: &str, op: ConditionOp, value: ConditionValue) -> Condition {
        Condition { column: column.to_string(), op, value }
    }

    fn lit(s: &str) -> ConditionValue {
        ConditionValue::Literal(s.to_string())
    }
    fn param(s: &str) -> ConditionValue {
        ConditionValue::Param(s.to_string())
    }

    // ── apply_projection ─────────────────────────────────────────────────────

    #[test]
    fn projection_empty_columns_returns_all() {
        let rows = vec![row(&[("A", "1"), ("B", "2")])];
        let result = apply_projection(rows.clone(), &[]);
        assert_eq!(result, rows);
    }

    #[test]
    fn projection_single_column() {
        let rows = vec![row(&[("CUBE_NAME", "Model"), ("CUBE_SOURCE", "1")])];
        let result = apply_projection(rows, &["CUBE_NAME".to_string()]);
        assert_eq!(
            result,
            vec![vec![("CUBE_NAME".to_string(), "Model".to_string())]]
        );
    }

    #[test]
    fn projection_preserves_requested_order() {
        let rows = vec![row(&[("A", "1"), ("B", "2"), ("C", "3")])];
        let cols: Vec<String> = ["C", "A"].iter().map(|s| s.to_string()).collect();
        let result = apply_projection(rows, &cols);
        assert_eq!(result[0][0].0, "C");
        assert_eq!(result[0][1].0, "A");
    }

    #[test]
    fn projection_missing_column_yields_empty_string() {
        let rows = vec![row(&[("A", "1")])];
        let result = apply_projection(rows, &["MISSING".to_string()]);
        assert_eq!(result[0][0].1, "");
    }

    #[test]
    fn projection_case_insensitive_match() {
        let rows = vec![row(&[("CUBE_NAME", "Model")])];
        let result = apply_projection(rows, &["cube_name".to_string()]);
        assert_eq!(result[0][0].1, "Model");
    }

    // ── apply_conditions ─────────────────────────────────────────────────────

    #[test]
    fn conditions_empty_returns_all() {
        let rows = vec![row(&[("A", "1")]), row(&[("A", "2")])];
        let result = apply_conditions(rows.clone(), &[], &HashMap::new());
        assert_eq!(result, rows);
    }

    #[test]
    fn condition_eq_literal() {
        let rows = vec![row(&[("CUBE_SOURCE", "1")]), row(&[("CUBE_SOURCE", "2")])];
        let result = apply_conditions(
            rows,
            &[cond("CUBE_SOURCE", ConditionOp::Eq, lit("1"))],
            &HashMap::new(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0]
                .iter()
                .find(|(k, _)| k == "CUBE_SOURCE")
                .unwrap()
                .1,
            "1"
        );
    }

    #[test]
    fn condition_ne_literal() {
        let rows = vec![
            row(&[("DIMENSION_UNIQUE_NAME", "[Measures]")]),
            row(&[("DIMENSION_UNIQUE_NAME", "[Color]")]),
        ];
        let result = apply_conditions(
            rows,
            &[cond(
                "DIMENSION_UNIQUE_NAME",
                ConditionOp::Ne,
                lit("[Measures]"),
            )],
            &HashMap::new(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].1, "[Color]");
    }

    #[test]
    fn condition_is_true() {
        let rows = vec![
            row(&[("DIMENSION_IS_VISIBLE", "true")]),
            row(&[("DIMENSION_IS_VISIBLE", "false")]),
            row(&[("DIMENSION_IS_VISIBLE", "0")]),
            row(&[("DIMENSION_IS_VISIBLE", "")]),
        ];
        let result = apply_conditions(
            rows,
            &[cond("DIMENSION_IS_VISIBLE", ConditionOp::IsTrue, lit(""))],
            &HashMap::new(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].1, "true");
    }

    #[test]
    fn condition_param_resolved() {
        let rows = vec![
            row(&[("CUBE_NAME", "Sales")]),
            row(&[("CUBE_NAME", "Model")]),
        ];
        let mut params = HashMap::new();
        params.insert("CatalogName".to_string(), "Model".to_string());
        let result = apply_conditions(
            rows,
            &[cond("CUBE_NAME", ConditionOp::Eq, param("CatalogName"))],
            &params,
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].1, "Model");
    }

    #[test]
    fn condition_unresolved_param_matches_empty() {
        let rows = vec![row(&[("CUBE_NAME", "")]), row(&[("CUBE_NAME", "Model")])];
        let result = apply_conditions(
            rows,
            &[cond("CUBE_NAME", ConditionOp::Eq, param("Missing"))],
            &HashMap::new(),
        );
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].1, "");
    }

    #[test]
    fn multiple_conditions_and_semantics() {
        let rows = vec![
            row(&[("CUBE_NAME", "Model"), ("CUBE_SOURCE", "1")]),
            row(&[("CUBE_NAME", "Model"), ("CUBE_SOURCE", "2")]),
            row(&[("CUBE_NAME", "Other"), ("CUBE_SOURCE", "1")]),
        ];
        let mut params = HashMap::new();
        params.insert("CatalogName".to_string(), "Model".to_string());
        let conditions = vec![
            cond("CUBE_NAME", ConditionOp::Eq, param("CatalogName")),
            cond("CUBE_SOURCE", ConditionOp::Eq, lit("1")),
        ];
        let result = apply_conditions(rows, &conditions, &params);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0][0].1, "Model");
        assert_eq!(result[0][1].1, "1");
    }

    #[test]
    fn condition_case_insensitive_column_lookup() {
        let rows = vec![row(&[("CUBE_NAME", "Model")])];
        let result = apply_conditions(
            rows,
            &[cond("cube_name", ConditionOp::Eq, lit("Model"))],
            &HashMap::new(),
        );
        assert_eq!(result.len(), 1);
    }
}
