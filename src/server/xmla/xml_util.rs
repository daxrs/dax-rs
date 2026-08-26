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

/// A leaf member's XMLA unique name: `{hier_uname}.&[{key}]` — except the
/// synthesized blank/unknown member (empty key), whose real Fabric-captured
/// unique name is the bare `{hier_uname}.&` with no brackets at all.
pub(super) fn member_unique_name(hier_uname: &str, key: &str) -> String {
    if key.is_empty() {
        format!("{hier_uname}.&")
    } else {
        format!("{hier_uname}.&[{key}]")
    }
}

// XSD schema embedded in every mddataset (Multidimensional) response — same
// for every query shape. Uses r###"..."### so the "## in ##targetNamespace
// does not close the literal.
pub(super) const MDDATASET_SCHEMA: &str = r###"<xs:schema targetNamespace="urn:schemas-microsoft-com:xml-analysis:mddataset" elementFormDefault="qualified"><xs:import namespace="http://schemas.microsoft.com/analysisservices/2003/xmla" /><xs:complexType name="MemberType"><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="Hierarchy" type="xs:string" /></xs:complexType><xs:complexType name="PropType"><xs:sequence><xs:element name="Default" minOccurs="0" /></xs:sequence><xs:attribute name="name" type="xs:string" use="required" /><xs:attribute name="type" type="xs:QName" /></xs:complexType><xs:complexType name="TupleType"><xs:sequence><xs:element name="Member" type="MemberType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence></xs:complexType><xs:complexType name="MembersType"><xs:sequence><xs:element name="Member" type="MemberType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence><xs:attribute name="Hierarchy" type="xs:string" use="required" /></xs:complexType><xs:complexType name="TuplesType"><xs:sequence><xs:element name="Tuple" type="TupleType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence></xs:complexType><xs:group name="SetType"><xs:choice><xs:element name="Members" type="MembersType" /><xs:element name="Tuples" type="TuplesType" /><xs:element name="CrossProduct" type="SetListType" /><xs:element ref="msxmla:NormTupleSet" /><xs:element name="Union"><xs:complexType><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /></xs:complexType></xs:element></xs:choice></xs:group><xs:complexType name="SetListType"><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /><xs:attribute name="Size" type="xs:unsignedInt" /></xs:complexType><xs:complexType name="OlapInfo"><xs:sequence><xs:element name="CubeInfo"><xs:complexType><xs:sequence><xs:element name="Cube" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:element name="CubeName" type="xs:string" /><xs:element name="LastDataUpdate" minOccurs="0" type="xs:dateTime" /><xs:element name="LastSchemaUpdate" minOccurs="0" type="xs:dateTime" /></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element><xs:element name="AxesInfo"><xs:complexType><xs:sequence><xs:element name="AxisInfo" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:element name="HierarchyInfo" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="name" type="xs:string" use="required" /></xs:complexType></xs:element></xs:sequence><xs:attribute name="name" type="xs:string" /></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element><xs:element name="CellInfo"><xs:complexType><xs:choice minOccurs="0" maxOccurs="unbounded"><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:choice></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:complexType name="Axes"><xs:sequence><xs:element name="Axis" maxOccurs="unbounded"><xs:complexType><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /><xs:attribute name="name" type="xs:string" /></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:complexType name="CellData"><xs:sequence><xs:element name="Cell" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="CellOrdinal" type="xs:unsignedInt" use="required" /></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:element name="root"><xs:complexType><xs:sequence><xs:any namespace="http://www.w3.org/2001/XMLSchema" processContents="strict" minOccurs="0" /><xs:element name="OlapInfo" type="OlapInfo" minOccurs="0" /><xs:element name="Axes" type="Axes" minOccurs="0" /><xs:element name="CellData" type="CellData" minOccurs="0" /></xs:sequence></xs:complexType></xs:element></xs:schema>"###;

fn cellset_envelope(session_id: Option<&str>, inner: &str) -> String {
    let session_header = match session_id {
        Some(id) => format!(
            "  <soap:Header>\n    <Session xmlns=\"urn:schemas-microsoft-com:xml-analysis\" SessionId=\"{id}\" />\n  </soap:Header>\n",
            id = xml_escape_attr(id),
        ),
        None => String::new(),
    };
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n",
            "<soap:Envelope",
            " xmlns:ns2=\"urn:schemas-microsoft-com:xml-analysis:mddataset\"",
            " xmlns:ns4=\"http://schemas.microsoft.com/analysisservices/2003/engine\"",
            " xmlns:ns5=\"http://schemas.microsoft.com/analysisservices/2003/xmla\"",
            " xmlns:soap=\"http://schemas.xmlsoap.org/soap/envelope/\"",
            " xmlns:xa=\"urn:schemas-microsoft-com:xml-analysis\"",
            " xmlns:xs=\"http://www.w3.org/2001/XMLSchema\">\n",
            "{session_header}",
            "  <soap:Body>\n",
            "    <xa:ExecuteResponse>\n",
            "      <xa:return>\n",
            "        {inner}\n",
            "      </xa:return>\n",
            "    </xa:ExecuteResponse>\n",
            "  </soap:Body>\n",
            "</soap:Envelope>",
        ),
        session_header = session_header,
        inner = inner,
    )
}

/// Wraps a cellset (Multidimensional) response body in its SOAP envelope.
/// Distinct from `execute_xml` (Tabular) because the mddataset envelope
/// declares extra namespaces (`ns2`/`ns4`/`ns5`/`xa`/`xs`) that the plain
/// Tabular rowset envelope doesn't need.
pub(super) fn cellset_xml(session_id: Option<&str>, body: String) -> (String, Response) {
    let xml = cellset_envelope(session_id, &body);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}
