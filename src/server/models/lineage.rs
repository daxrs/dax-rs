use crate::engine::dax::ast::DaxExpr;
use crate::engine::dax::parser::parse_expression;
use std::collections::{HashMap, HashSet, VecDeque};

pub struct DependencyGraph {
    forward_measures: HashMap<String, Vec<String>>,
    forward_columns: HashMap<String, Vec<(String, String)>>,
    reverse: HashMap<String, Vec<String>>,
    col_to_measures: HashMap<(String, String), Vec<String>>,
}

impl DependencyGraph {
    pub fn build(measures: &HashMap<String, String>) -> Self {
        let mut forward_measures: HashMap<String, Vec<String>> = HashMap::new();
        let mut forward_columns: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut reverse: HashMap<String, Vec<String>> = HashMap::new();
        let mut col_to_measures: HashMap<(String, String), Vec<String>> = HashMap::new();

        for name in measures.keys() {
            forward_measures.entry(name.clone()).or_default();
            forward_columns.entry(name.clone()).or_default();
            reverse.entry(name.clone()).or_default();
        }

        for (name, expr_str) in measures {
            let Ok(ast) = parse_expression(expr_str) else {
                continue;
            };

            let mut m_refs: Vec<String> = Vec::new();
            let mut c_refs: Vec<(String, String)> = Vec::new();
            collect_refs(&ast, &mut m_refs, &mut c_refs);

            let m_entry = forward_measures.entry(name.clone()).or_default();
            for dep in m_refs {
                if !m_entry.contains(&dep) {
                    m_entry.push(dep.clone());
                }
                reverse.entry(dep).or_default().push(name.clone());
            }

            let c_entry = forward_columns.entry(name.clone()).or_default();
            for col in c_refs {
                if !c_entry.contains(&col) {
                    c_entry.push(col.clone());
                }
                col_to_measures.entry(col).or_default().push(name.clone());
            }
        }

        Self { forward_measures, forward_columns, reverse, col_to_measures }
    }

    pub fn upstream_of_measure(&self, name: &str) -> (Vec<String>, Vec<(String, String)>) {
        let measures = self.forward_measures.get(name).cloned().unwrap_or_default();
        let columns = self.forward_columns.get(name).cloned().unwrap_or_default();
        (measures, columns)
    }

    pub fn direct_downstream_of_measure(&self, name: &str) -> Vec<String> {
        self.reverse.get(name).cloned().unwrap_or_default()
    }

    pub fn transitive_downstream(&self, name: &str) -> Vec<String> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        for dep in self.reverse.get(name).into_iter().flatten() {
            if visited.insert(dep.clone()) {
                queue.push_back(dep.clone());
            }
        }

        while let Some(current) = queue.pop_front() {
            for dep in self.reverse.get(&current).into_iter().flatten() {
                if visited.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }

        let mut result: Vec<String> = visited.into_iter().collect();
        result.sort();
        result
    }

    pub fn measures_using_column(&self, table: &str, column: &str) -> Vec<String> {
        self.col_to_measures
            .get(&(table.to_string(), column.to_string()))
            .cloned()
            .unwrap_or_default()
    }

    pub fn impacted_by_column(&self, table: &str, column: &str) -> Vec<String> {
        let direct = self.measures_using_column(table, column);
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        for m in direct {
            if visited.insert(m.clone()) {
                queue.push_back(m);
            }
        }

        while let Some(current) = queue.pop_front() {
            for dep in self.reverse.get(&current).into_iter().flatten() {
                if visited.insert(dep.clone()) {
                    queue.push_back(dep.clone());
                }
            }
        }

        let mut result: Vec<String> = visited.into_iter().collect();
        result.sort();
        result
    }
}

pub fn parse_column_object(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let bracket = s.find('[')?;
    let table = s[..bracket].trim().trim_matches('\'').to_string();
    let column = s[bracket + 1..].trim_end_matches(']').to_string();
    if table.is_empty() || column.is_empty() {
        return None;
    }
    Some((table, column))
}

pub fn collect_refs(
    expr: &DaxExpr,
    measures: &mut Vec<String>,
    columns: &mut Vec<(String, String)>,
) {
    match expr {
        DaxExpr::MeasureRef(name) => {
            if !measures.contains(name) {
                measures.push(name.clone());
            }
        }
        DaxExpr::ColumnRef { table, column } => {
            let pair = (table.clone(), column.clone());
            if !columns.contains(&pair) {
                columns.push(pair);
            }
        }
        DaxExpr::FunctionCall { args, .. } => {
            args.iter().for_each(|a| collect_refs(a, measures, columns));
        }
        DaxExpr::BinaryOp { lhs, rhs, .. } => {
            collect_refs(lhs, measures, columns);
            collect_refs(rhs, measures, columns);
        }
        DaxExpr::UnaryOp { expr, .. } => collect_refs(expr, measures, columns),
        DaxExpr::VarExpr { bindings, result } => {
            bindings
                .iter()
                .for_each(|(_, e)| collect_refs(e, measures, columns));
            collect_refs(result, measures, columns);
        }
        DaxExpr::TableConstructor(rows) => {
            rows.iter()
                .flat_map(|r| r.iter())
                .for_each(|e| collect_refs(e, measures, columns));
        }
        DaxExpr::Literal(_) | DaxExpr::Identifier(_) => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn make_graph(pairs: &[(&str, &str)]) -> DependencyGraph {
        let measures: HashMap<String, String> = pairs
            .iter()
            .map(|(n, e)| (n.to_string(), e.to_string()))
            .collect();
        DependencyGraph::build(&measures)
    }

    #[test]
    fn upstream_direct_measure_and_column() {
        let g = make_graph(&[("Revenue", "'Sales'[Amount]"), ("YTD Revenue", "[Revenue]")]);
        let (m, c) = g.upstream_of_measure("Revenue");
        assert!(m.is_empty(), "Revenue has no measure deps");
        assert_eq!(c, vec![("Sales".to_string(), "Amount".to_string())]);

        let (m2, c2) = g.upstream_of_measure("YTD Revenue");
        assert_eq!(m2, vec!["Revenue"]);
        assert!(c2.is_empty());
    }

    #[test]
    fn downstream_and_transitive() {
        let g = make_graph(&[("Base", "'T'[Col]"), ("Mid", "[Base]"), ("Top", "[Mid]")]);
        let down = g.direct_downstream_of_measure("Base");
        assert_eq!(down, vec!["Mid"]);

        let mut impact = g.transitive_downstream("Base");
        impact.sort();
        assert_eq!(impact, vec!["Mid", "Top"]);
    }

    #[test]
    fn column_downstream_and_impacted() {
        let g = make_graph(&[("Amount", "'Sales'[RawAmount]"), ("Revenue", "[Amount]")]);
        let direct = g.measures_using_column("Sales", "RawAmount");
        assert_eq!(direct, vec!["Amount"]);

        let mut impacted = g.impacted_by_column("Sales", "RawAmount");
        impacted.sort();
        assert_eq!(impacted, vec!["Amount", "Revenue"]);
    }

    #[test]
    fn parse_column_object_formats() {
        assert_eq!(
            parse_column_object("Sales[Amount]"),
            Some(("Sales".into(), "Amount".into()))
        );
        assert_eq!(
            parse_column_object("'My Table'[Col Name]"),
            Some(("My Table".into(), "Col Name".into()))
        );
        assert_eq!(parse_column_object("NoColumn"), None);
    }

    #[test]
    fn parse_error_expression_skipped_silently() {
        let g = make_graph(&[("Good", "'T'[Col]"), ("Bad", "%%%invalid dax%%%")]);
        let (_, cols) = g.upstream_of_measure("Good");
        assert_eq!(cols, vec![("T".into(), "Col".into())]);
        let (m, c) = g.upstream_of_measure("Bad");
        assert!(m.is_empty() && c.is_empty());
    }
}
