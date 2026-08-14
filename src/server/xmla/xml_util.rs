//! Generic, shape-agnostic XMLA rowset/XML rendering primitives shared by
//! the old per-`QueryShape` handlers and the new `tabular` response type.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

fn execute_envelope(session_id: Option<&str>, inner: &str) -> String {
    let session_header = match session_id {
        Some(id) => format!(
            r#"  <soap:Header>
    <Session xmlns="urn:schemas-microsoft-com:xml-analysis" SessionId="{id}" />
  </soap:Header>
"#,
            id = xml_escape_attr(id),
        ),
        None => String::new(),
    };
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
{session_header}  <soap:Body>
    <ExecuteResponse xmlns="urn:schemas-microsoft-com:xml-analysis">
      <return>
        {inner}
      </return>
    </ExecuteResponse>
  </soap:Body>
</soap:Envelope>"#
    )
}

/// Matches real Fabric's column naming: `C0`.."C9" under 10 total columns,
/// zero-padded to the widest index (`C00`.."C10", etc.) at 10 or more.
pub(super) fn tabular_col(i: usize, total: usize) -> String {
    let width = total.saturating_sub(1).to_string().len().max(1);
    format!("C{i:0width$}")
}

pub(super) fn make_tabular_schema(columns: &[(&str, Option<&str>)]) -> String {
    let total = columns.len();
    let cols: String = columns
        .iter()
        .enumerate()
        .map(|(i, (field, typ))| {
            let type_attr = match typ {
                Some(t) => format!(r#" type="xsd:{t}""#),
                None => String::new(),
            };
            let name = tabular_col(i, total);
            format!(r#"<xsd:element sql:field="{field}" name="{name}"{type_attr} minOccurs="0"/>"#)
        })
        .collect();
    format!(
        r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified"><xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded"><xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/></xsd:sequence></xsd:complexType></xsd:element><xsd:complexType name="row"><xsd:sequence>{cols}</xsd:sequence></xsd:complexType></xsd:schema>"#
    )
}

pub(super) fn rowset(schema: &str, inner: &str) -> String {
    format!(
        r#"<root xmlns="urn:schemas-microsoft-com:xml-analysis:rowset" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">{schema}{inner}</root>"#
    )
}

pub(super) fn execute_xml(session_id: Option<&str>, body: String) -> (String, Response) {
    let xml = execute_envelope(session_id, &body);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

pub(super) fn xml_escape_value(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
