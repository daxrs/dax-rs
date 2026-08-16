//! Purpose-built Tabular-format XMLA rowset response type. Shared by any
//! adapter — MDX (`mdx_tabular`) or a future direct-DAX one — that needs to
//! render query results as a flat `urn:...:rowset` response.

use super::xml_util::{
    execute_xml, make_tabular_schema, member_unique_name, rowset, tabular_col, xml_escape_value,
};
use axum::response::Response;

struct TabularColumn {
    field: String,
    xsd_type: Option<&'static str>,
}

#[derive(Default)]
pub struct TabularResponse {
    columns: Vec<TabularColumn>,
    rows: Vec<Vec<Option<String>>>,
}

pub struct DimColumnPlan {
    pub field: String,
    pub hier_uname: String,
    pub prop: String,
    pub result_col: usize,
}

pub struct MeasureColumnPlan {
    pub name: String,
    pub result_col: usize,
}

impl TabularResponse {
    pub fn add_column(&mut self, field: impl Into<String>, xsd_type: Option<&'static str>) {
        self.columns
            .push(TabularColumn { field: field.into(), xsd_type });
    }

    /// Builds and sets the response's rows from raw DAX result rows plus the
    /// column plans describing how to pull and format each cell. Replaces
    /// any previously set rows.
    pub fn set_rows(
        &mut self,
        result_rows: &[Vec<Option<String>>],
        dim_plans: &[DimColumnPlan],
        measure_plans: &[MeasureColumnPlan],
    ) -> Result<(), String> {
        let expected = dim_plans.len() + measure_plans.len();
        if expected != self.columns.len() {
            return Err(format!(
                "{expected} column plans given but {} columns were declared",
                self.columns.len()
            ));
        }

        let mut rows = Vec::with_capacity(result_rows.len());
        for row in result_rows {
            let mut out = Vec::with_capacity(expected);
            for p in dim_plans {
                let raw = row.get(p.result_col).and_then(|v| v.clone());
                match p.prop.as_str() {
                    "MEMBER_UNIQUE_NAME" => {
                        out.push(Some(member_unique_name(
                            &p.hier_uname,
                            &raw.unwrap_or_default(),
                        )));
                    }
                    _ => out.push(raw),
                }
            }
            for p in measure_plans {
                out.push(row.get(p.result_col).and_then(|v| v.clone()));
            }
            rows.push(out);
        }
        self.rows = rows;
        Ok(())
    }

    pub fn render(&self, session_id: Option<&str>) -> (String, Response) {
        let columns: Vec<(&str, Option<&str>)> = self
            .columns
            .iter()
            .map(|c| (c.field.as_str(), c.xsd_type))
            .collect();
        let schema = make_tabular_schema(&columns);

        let total = self.columns.len();
        let mut rows_xml = String::new();
        for row in &self.rows {
            rows_xml.push_str("<row>");
            for (i, value) in row.iter().enumerate() {
                if let Some(v) = value {
                    let tag = tabular_col(i, total);
                    rows_xml.push_str(&format!("<{tag}>{}</{tag}>", xml_escape_value(v)));
                }
            }
            rows_xml.push_str("</row>");
        }

        execute_xml(session_id, rowset(&schema, &rows_xml))
    }
}
