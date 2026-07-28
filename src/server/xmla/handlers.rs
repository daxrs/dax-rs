use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::mdx::Row;
use crate::server::config::ServerConfig;
use crate::server::provider::{
    ColumnMeta, DatabaseMeta, MeasureMeta, ModelMeta, QueryResult, RelationshipMeta, TableMeta,
};

const CATALOG_COMPAT_LEVEL: u32 = 1604;
const SERVER_VERSION: &str = "17.0.67.18";
const CUBE_NAME: &str = "Model";
fn xml_envelope(session_id: Option<&str>, inner: &str) -> String {
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
    <DiscoverResponse xmlns="urn:schemas-microsoft-com:xml-analysis">
      <return>
        {inner}
      </return>
    </DiscoverResponse>
  </soap:Body>
</soap:Envelope>"#
    )
}

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

fn make_schema(columns: &[(&str, &str)]) -> String {
    let has_uuid = columns.iter().any(|(_, t)| *t == "uuid");
    let uuid_def = if has_uuid {
        r#"<xsd:simpleType name="uuid"><xsd:restriction base="xsd:string"><xsd:pattern value="[0-9a-zA-Z]{8}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{12}"/></xsd:restriction></xsd:simpleType>"#
    } else {
        ""
    };
    let cols: String = columns
        .iter()
        .map(|(name, typ)| {
            let type_attr = if *typ == "uuid" {
                r#"type="uuid""#.to_string()
            } else {
                format!(r#"type="xsd:{typ}""#)
            };
            format!(r#"<xsd:element sql:field="{name}" name="{name}" {type_attr} minOccurs="0"/>"#)
        })
        .collect();
    format!(
        r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified"><xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded"><xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/></xsd:sequence></xsd:complexType></xsd:element>{uuid_def}<xsd:complexType name="row"><xsd:sequence>{cols}</xsd:sequence></xsd:complexType></xsd:schema>"#
    )
}

fn make_dax_schema(columns: &[(&str, &str)]) -> String {
    let cols: String = columns
        .iter()
        .enumerate()
        .map(|(i, (field, typ))| {
            format!(
                r#"<xsd:element sql:field="{field}" name="C{i}" type="xsd:{typ}" minOccurs="0"/>"#
            )
        })
        .collect();
    format!(
        r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified"><xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded"><xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/></xsd:sequence></xsd:complexType></xsd:element><xsd:complexType name="row"><xsd:sequence>{cols}</xsd:sequence></xsd:complexType></xsd:schema>"#
    )
}

fn make_xmldoc_schema(field_name: &str) -> String {
    format!(
        concat!(
            r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql""#,
            r#" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified">"#,
            r#"<xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded">"#,
            r#"<xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/>"#,
            r#"</xsd:sequence></xsd:complexType></xsd:element>"#,
            r#"<xsd:complexType name="xmlDocument"><xsd:sequence><xsd:any/></xsd:sequence></xsd:complexType>"#,
            r#"<xsd:complexType name="row"><xsd:sequence>"#,
            r#"<xsd:element sql:field="{field}" name="{field}" type="xmlDocument" minOccurs="0"/>"#,
            r#"</xsd:sequence></xsd:complexType>"#,
            r#"</xsd:schema>"#,
        ),
        field = field_name,
    )
}

const SCHEMA_GENERIC: &str = r###"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified"><xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded"><xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/></xsd:sequence></xsd:complexType></xsd:element><xsd:complexType name="row"><xsd:sequence><xsd:any namespace="##any" minOccurs="0" maxOccurs="unbounded" processContents="lax"/></xsd:sequence></xsd:complexType></xsd:schema>"###;

fn rowset(schema: &str, inner: &str) -> String {
    format!(
        r#"<root xmlns="urn:schemas-microsoft-com:xml-analysis:rowset" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">{schema}{inner}</root>"#
    )
}

fn ok_xml(session_id: Option<&str>, body: String) -> (String, Response) {
    let xml = xml_envelope(session_id, &body);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn execute_xml(session_id: Option<&str>, body: String) -> (String, Response) {
    let xml = execute_envelope(session_id, &body);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

pub fn empty_ok(session_id: Option<&str>) -> (String, Response) {
    ok_xml(session_id, rowset(SCHEMA_GENERIC, ""))
}

pub fn discover_mdschema_sets(session_id: Option<&str>) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("SET_NAME", "string"),
        ("SCOPE", "int"),
        ("DESCRIPTION", "string"),
        ("EXPRESSION", "string"),
        ("DIMENSIONS", "string"),
        ("SET_CAPTION", "string"),
        ("SET_DISPLAY_FOLDER", "string"),
        ("SET_EVALUATION_CONTEXT", "int"),
    ]);
    ok_xml(session_id, rowset(&schema, ""))
}

pub fn discover_literals(session_id: Option<&str>) -> (String, Response) {
    let schema = make_schema(&[
        ("LiteralName", "string"),
        ("LiteralValue", "string"),
        ("LiteralInvalidChars", "string"),
        ("LiteralInvalidStartingChars", "string"),
        ("LiteralMaxLength", "int"),
        ("LiteralNameEnumValue", "int"),
    ]);

    let literals: &[(&str, &str, &str, &str, i32, i32)] = &[
        ("DBLITERAL_CATALOG_NAME", "", ".", "0123456789 ", 24, 2),
        ("DBLITERAL_CATALOG_SEPARATOR", ".", "", "", 1, 3),
        ("DBLITERAL_COLUMN_ALIAS", "", "'\"[]", "0123456789 ", 255, 5),
        ("DBLITERAL_COLUMN_NAME", "", ".", "0123456789 ", 14, 6),
        (
            "DBLITERAL_CORRELATION_NAME",
            "",
            "'\"[]",
            "0123456789 ",
            255,
            7,
        ),
        ("DBLITERAL_PROCEDURE_NAME", "", ".", "0123456789 ", 255, 14),
        ("DBLITERAL_TABLE_NAME", "", ".", "0123456789 ", 24, 17),
        ("DBLITERAL_TEXT_COMMAND", "", "", "", 0, 18),
        ("DBLITERAL_USER_NAME", "", "", "", 0, 19),
        ("DBLITERAL_QUOTE_PREFIX", "[", "", "", 1, 15),
        ("DBLITERAL_CUBE_NAME", "", ".", "0123456789 ", 24, 21),
        ("DBLITERAL_DIMENSION_NAME", "", ".", "0123456789 ", 14, 22),
        ("DBLITERAL_HIERARCHY_NAME", "", ".", "0123456789 ", 10, 23),
        ("DBLITERAL_LEVEL_NAME", "", ".", "0123456789 ", 255, 24),
        ("DBLITERAL_MEMBER_NAME", "", ".", "0123456789 ", 255, 25),
        ("DBLITERAL_PROPERTY_NAME", "", ".", "0123456789 ", 255, 26),
        ("DBLITERAL_QUOTE_SUFFIX", "]", "", "", 1, 28),
        ("DBLITERAL_SCHEMA_NAME", "", ".", "0123456789 ", 24, 16),
        ("DBLITERAL_SCHEMA_SEPARATOR", ".", "", "", 1, 27),
    ];

    let rows: String = literals
        .iter()
        .map(|(name, value, invalid, invalid_start, max, enum_val)| {
            let v = if value.is_empty() {
                String::new()
            } else {
                format!("<LiteralValue>{value}</LiteralValue>")
            };
            let ic = if invalid.is_empty() {
                String::new()
            } else {
                format!("<LiteralInvalidChars>{invalid}</LiteralInvalidChars>")
            };
            let is = if invalid_start.is_empty() {
                String::new()
            } else {
                format!("<LiteralInvalidStartingChars>{invalid_start}</LiteralInvalidStartingChars>")
            };
            format!(
                "<row><LiteralName>{name}</LiteralName>{v}{ic}{is}<LiteralMaxLength>{max}</LiteralMaxLength><LiteralNameEnumValue>{enum_val}</LiteralNameEnumValue></row>"
            )
        })
        .collect();

    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn execute_empty_rowset(session_id: Option<&str>) -> (String, Response) {
    execute_xml(session_id, rowset(SCHEMA_GENERIC, ""))
}

pub fn execute_fault(session_id: Option<&str>, message: &str) -> (String, Response) {
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
    let text = xml_escape_value(message);
    let attr = xml_escape_attr(message);
    let xml = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/">
{session_header}  <soap:Body>
    <soap:Fault>
      <faultcode>soap:Server</faultcode>
      <faultstring>{text}</faultstring>
      <faultactor>DAX-RS</faultactor>
      <detail>
        <Error xmlns="urn:schemas-microsoft-com:xml-analysis:exception" ErrorCode="3238002580" Description="{attr}" Source="DAX-RS" HelpFile=""/>
      </detail>
    </soap:Fault>
  </soap:Body>
</soap:Envelope>"#
    );
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

pub fn execute_ok(session_id: Option<&str>) -> (String, Response) {
    let inner = r#"<root xmlns="urn:schemas-microsoft-com:xml-analysis:empty"/>"#;
    execute_xml(session_id, inner.to_string())
}

pub fn discover_datasources(session_id: Option<&str>, config: &ServerConfig) -> (String, Response) {
    let schema = make_schema(&[
        ("DataSourceName", "string"),
        ("DataSourceDescription", "string"),
        ("URL", "string"),
        ("DataSourceInfo", "string"),
        ("ProviderName", "string"),
        ("ProviderType", "string"),
        ("AuthenticationMode", "string"),
    ]);
    let row = format!(
        r#"<row><DataSourceName>{name}</DataSourceName><DataSourceDescription>Rust XMLA Server</DataSourceDescription><URL>{url}</URL><DataSourceInfo>{dsi}</DataSourceInfo><ProviderName>MSOLAP</ProviderName><ProviderType>MDP,TDP</ProviderType><AuthenticationMode>Unauthenticated</AuthenticationMode></row>"#,
        name = xml_escape_value(&config.server_name),
        url = xml_escape_value(&config.xmla_url()),
        dsi = xml_escape_value(&config.data_source_info()),
    );
    ok_xml(session_id, rowset(&schema, &row))
}

pub fn discover_properties(
    session_id: Option<&str>,
    filter: Option<&[String]>,
    catalog: Option<&str>,
    config: &ServerConfig,
) -> (String, Response) {
    let activity_id = Uuid::new_v4().to_string().to_uppercase();
    let current_activity_id = Uuid::new_v4().to_string().to_uppercase();
    let catalog_value = catalog.unwrap_or("");
    let locale_str = config.locale_identifier.to_string();
    let all_rows: &[(&str, &str, &str, &str)] = &[
        ("ServerVersion", "string", "Read", SERVER_VERSION),
        ("DBMSVersion", "string", "Read", SERVER_VERSION),
        ("ProviderVersion", "string", "Read", SERVER_VERSION),
        ("Catalog", "string", "ReadWrite", catalog_value),
        ("Format", "string", "Write", "Native"),
        ("Content", "string", "Write", "SchemaData"),
        ("DbpropMsmdSubqueries", "int", "ReadWrite", "63"),
        ("DbpropMsmdMDXCompatibility", "int", "ReadWrite", "1"),
        ("DbpropMsmdMDXUniqueNameStyle", "int", "ReadWrite", "6"),
        ("MDXMissingMemberMode", "string", "ReadWrite", "Error"),
        ("VisualMode", "int", "ReadWrite", "0"),
        ("DbpropMsmdOptimizeResponse", "int", "Read", "9"),
        ("DbpropMsmdMaxProtocolVersion", "int", "Read", "0"),
        ("DeploymentMode", "int", "Read", "2"),
        ("ApplicationContext", "string", "Write", ""),
        ("MdpropMdxSubqueries", "int", "Read", "63"),
        ("MdpropMdxDdlExtensions", "int", "Read", "23"),
        ("MdpropMdxDrillFunctions", "int", "Read", "7"),
        ("MdpropMdxNamedSets", "int", "Read", "15"),
    ];
    let dynamic_rows = [
        ("ServerName", "string", "Read", config.server_name.as_str()),
        ("LocaleIdentifier", "int", "ReadWrite", locale_str.as_str()),
        (
            "DbpropMsmdActivityID",
            "string",
            "ReadWrite",
            activity_id.as_str(),
        ),
        (
            "DbpropMsmdCurrentActivityID",
            "string",
            "ReadWrite",
            current_activity_id.as_str(),
        ),
    ];

    let mut rows = String::new();
    for (name, ptype, access, value) in all_rows.iter().chain(dynamic_rows.iter()) {
        let name_matches = filter.is_none_or(|f| f.iter().any(|n| n == name));
        let include = name_matches && (!value.is_empty() || filter.is_some());
        if include {
            rows.push_str(&format!(
                r#"<row><PropertyName>{name}</PropertyName><PropertyDescription/><PropertyType>{ptype}</PropertyType><PropertyAccessType>{access}</PropertyAccessType><IsRequired>false</IsRequired><Value>{value}</Value></row>"#
            ));
        }
    }

    let schema = make_schema(&[
        ("PropertyName", "string"),
        ("PropertyDescription", "string"),
        ("PropertyType", "string"),
        ("PropertyAccessType", "string"),
        ("IsRequired", "boolean"),
        ("Value", "string"),
    ]);
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn discover_schema_rowsets(
    session_id: Option<&str>,
    schema_name: Option<&str>,
) -> (String, Response) {
    type R = &'static [(&'static str, &'static str)];
    let schemas: &[(&str, &str, R, u64)] = &[
        (
            "DBSCHEMA_CATALOGS",
            "c8b52211-5cf3-11ce-ade5-00aa0044773d",
            &[("CATALOG_NAME", "xsd:string")],
            1,
        ),
        (
            "DBSCHEMA_TABLES",
            "c8b52229-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("TABLE_CATALOG", "xsd:string"),
                ("TABLE_SCHEMA", "xsd:string"),
                ("TABLE_NAME", "xsd:string"),
                ("TABLE_TYPE", "xsd:string"),
                ("TABLE_OLAP_TYPE", "xsd:string"),
            ],
            31,
        ),
        (
            "DBSCHEMA_COLUMNS",
            "c8b52214-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("TABLE_CATALOG", "xsd:string"),
                ("TABLE_SCHEMA", "xsd:string"),
                ("TABLE_NAME", "xsd:string"),
                ("COLUMN_NAME", "xsd:string"),
                ("COLUMN_OLAP_TYPE", "xsd:string"),
            ],
            31,
        ),
        (
            "DBSCHEMA_PROVIDER_TYPES",
            "c8b5222c-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("DATA_TYPE", "xsd:unsignedShort"),
                ("BEST_MATCH", "xsd:boolean"),
            ],
            3,
        ),
        (
            "MDSCHEMA_CUBES",
            "c8b522d8-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("BASE_CUBE_NAME", "xsd:string"),
            ],
            31,
        ),
        (
            "MDSCHEMA_DIMENSIONS",
            "c8b522d9-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("DIMENSION_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("DIMENSION_VISIBILITY", "xsd:unsignedShort"),
            ],
            127,
        ),
        (
            "MDSCHEMA_HIERARCHIES",
            "c8b522da-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("HIERARCHY_NAME", "xsd:string"),
                ("HIERARCHY_UNIQUE_NAME", "xsd:string"),
                ("HIERARCHY_ORIGIN", "xsd:unsignedShort"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("HIERARCHY_VISIBILITY", "xsd:unsignedShort"),
            ],
            511,
        ),
        (
            "MDSCHEMA_LEVELS",
            "c8b522db-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("HIERARCHY_UNIQUE_NAME", "xsd:string"),
                ("LEVEL_NAME", "xsd:string"),
                ("LEVEL_UNIQUE_NAME", "xsd:string"),
                ("LEVEL_ORIGIN", "xsd:unsignedShort"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("LEVEL_VISIBILITY", "xsd:unsignedShort"),
            ],
            1023,
        ),
        (
            "MDSCHEMA_MEASURES",
            "c8b522dc-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_NAME", "xsd:string"),
                ("MEASURE_UNIQUE_NAME", "xsd:string"),
                ("MEASUREGROUP_NAME", "xsd:string"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("MEASURE_VISIBILITY", "xsd:unsignedShort"),
            ],
            255,
        ),
        (
            "MDSCHEMA_PROPERTIES",
            "c8b522dd-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("HIERARCHY_UNIQUE_NAME", "xsd:string"),
                ("LEVEL_UNIQUE_NAME", "xsd:string"),
                ("MEMBER_UNIQUE_NAME", "xsd:string"),
                ("PROPERTY_NAME", "xsd:string"),
                ("PROPERTY_TYPE", "xsd:short"),
                ("PROPERTY_CONTENT_TYPE", "xsd:short"),
                ("PROPERTY_ORIGIN", "xsd:unsignedShort"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("PROPERTY_VISIBILITY", "xsd:unsignedShort"),
            ],
            8191,
        ),
        (
            "MDSCHEMA_MEMBERS",
            "c8b522de-5cf3-11ce-ade5-00aa0044773d",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("HIERARCHY_UNIQUE_NAME", "xsd:string"),
                ("LEVEL_UNIQUE_NAME", "xsd:string"),
                ("LEVEL_NUMBER", "xsd:unsignedInt"),
                ("MEMBER_NAME", "xsd:string"),
                ("MEMBER_UNIQUE_NAME", "xsd:string"),
                ("MEMBER_CAPTION", "xsd:string"),
                ("MEMBER_TYPE", "xsd:int"),
                ("TREE_OP", "xsd:int"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("SCOPE", "xsd:int"),
            ],
            16383,
        ),
        (
            "MDSCHEMA_FUNCTIONS",
            "a07ccd07-8148-11d0-87bb-00c04fc33942",
            &[
                ("LIBRARY_NAME", "xsd:string"),
                ("INTERFACE_NAME", "xsd:string"),
                ("FUNCTION_NAME", "xsd:string"),
                ("ORIGIN", "xsd:int"),
                ("CATALOG_NAME", "xsd:string"),
            ],
            31,
        ),
        (
            "MDSCHEMA_ACTIONS",
            "a07ccd08-8148-11d0-87bb-00c04fc33942",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("ACTION_NAME", "xsd:string"),
                ("ACTION_TYPE", "xsd:int"),
                ("COORDINATE", "xsd:string"),
                ("COORDINATE_TYPE", "xsd:int"),
                ("INVOCATION", "xsd:int"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
            ],
            511,
        ),
        (
            "MDSCHEMA_SETS",
            "a07ccd0b-8148-11d0-87bb-00c04fc33942",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("SET_NAME", "xsd:string"),
                ("SCOPE", "xsd:int"),
                ("HIERARCHY_UNIQUE_NAME", "xsd:string"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("SET_EVALUATION_CONTEXT", "xsd:int"),
            ],
            255,
        ),
        (
            "DISCOVER_INSTANCES",
            "20518699-2474-4c15-9885-0e947ec7a7e3",
            &[("INSTANCE_NAME", "xsd:string")],
            1,
        ),
        (
            "MDSCHEMA_KPIS",
            "2ae44109-ed3d-4842-b16f-b694d1cb0e3f",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("KPI_NAME", "xsd:string"),
                ("CUBE_SOURCE", "xsd:unsignedShort"),
                ("SCOPE", "xsd:int"),
            ],
            63,
        ),
        (
            "MDSCHEMA_MEASUREGROUPS",
            "e1625ebf-fa96-42fd-bea6-db90adafd96b",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASUREGROUP_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "MDSCHEMA_MEASUREGROUP_DIMENSIONS",
            "a07ccd33-8148-11d0-87bb-00c04fc33942",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASUREGROUP_NAME", "xsd:string"),
                ("DIMENSION_UNIQUE_NAME", "xsd:string"),
                ("DIMENSION_VISIBILITY", "xsd:unsignedShort"),
            ],
            63,
        ),
        (
            "MDSCHEMA_INPUT_DATASOURCES",
            "a07ccd32-8148-11d0-87bb-00c04fc33942",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("SCHEMA_NAME", "xsd:string"),
                ("DATASOURCE_NAME", "xsd:string"),
                ("DATASOURCE_TYPE", "xsd:string"),
            ],
            15,
        ),
        (
            "DMSCHEMA_MINING_SERVICES",
            "3add8a95-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("SERVICE_NAME", "xsd:string"),
                ("SERVICE_TYPE_ID", "xsd:unsignedInt"),
            ],
            3,
        ),
        (
            "DMSCHEMA_MINING_SERVICE_PARAMETERS",
            "3add8a75-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("SERVICE_NAME", "xsd:string"),
                ("PARAMETER_NAME", "xsd:string"),
            ],
            3,
        ),
        (
            "DMSCHEMA_MINING_FUNCTIONS",
            "3add8a79-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("SERVICE_NAME", "xsd:string"),
                ("FUNCTION_NAME", "xsd:string"),
            ],
            3,
        ),
        (
            "DMSCHEMA_MINING_MODEL_CONTENT",
            "3add8a76-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("MODEL_CATALOG", "xsd:string"),
                ("MODEL_SCHEMA", "xsd:string"),
                ("MODEL_NAME", "xsd:string"),
                ("ATTRIBUTE_NAME", "xsd:string"),
                ("NODE_NAME", "xsd:string"),
                ("NODE_UNIQUE_NAME", "xsd:string"),
                ("NODE_TYPE", "xsd:int"),
                ("NODE_GUID", "xsd:string"),
                ("NODE_CAPTION", "xsd:string"),
                ("TREE_OPERATION", "xsd:unsignedInt"),
            ],
            1023,
        ),
        (
            "DMSCHEMA_MINING_MODEL_XML",
            "4290b2d5-0e9c-4aa7-9369-98c95cfd9d13",
            &[
                ("MODEL_CATALOG", "xsd:string"),
                ("MODEL_SCHEMA", "xsd:string"),
                ("MODEL_NAME", "xsd:string"),
                ("MODEL_TYPE", "xsd:string"),
            ],
            15,
        ),
        (
            "DMSCHEMA_MINING_MODEL_CONTENT_PMML",
            "4290b2d5-0e9c-4aa7-9369-98c95cfd9d13",
            &[
                ("MODEL_CATALOG", "xsd:string"),
                ("MODEL_SCHEMA", "xsd:string"),
                ("MODEL_NAME", "xsd:string"),
                ("MODEL_TYPE", "xsd:string"),
            ],
            15,
        ),
        (
            "DMSCHEMA_MINING_MODELS",
            "3add8a77-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("MODEL_CATALOG", "xsd:string"),
                ("MODEL_SCHEMA", "xsd:string"),
                ("MODEL_NAME", "xsd:string"),
                ("MODEL_TYPE", "xsd:string"),
                ("SERVICE_NAME", "xsd:string"),
                ("SERVICE_TYPE_ID", "xsd:unsignedInt"),
                ("MINING_STRUCTURE", "xsd:string"),
            ],
            127,
        ),
        (
            "DMSCHEMA_MINING_COLUMNS",
            "3add8a78-d8b9-11d2-8d2a-00e029154fde",
            &[
                ("MODEL_CATALOG", "xsd:string"),
                ("MODEL_SCHEMA", "xsd:string"),
                ("MODEL_NAME", "xsd:string"),
                ("COLUMN_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "DMSCHEMA_MINING_STRUCTURES",
            "883269f3-0cad-462f-b6f5-e88a72418c4b",
            &[
                ("STRUCTURE_CATALOG", "xsd:string"),
                ("STRUCTURE_SCHEMA", "xsd:string"),
                ("STRUCTURE_NAME", "xsd:string"),
            ],
            7,
        ),
        (
            "DMSCHEMA_MINING_STRUCTURE_COLUMNS",
            "9952e836-bfbf-4d1f-8535-9b67dbd9ddfe",
            &[
                ("STRUCTURE_CATALOG", "xsd:string"),
                ("STRUCTURE_SCHEMA", "xsd:string"),
                ("STRUCTURE_NAME", "xsd:string"),
                ("COLUMN_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "DISCOVER_DATASOURCES",
            "06c03d41-f66d-49f3-b1b8-987f7af4cf18",
            &[
                ("DataSourceName", "xsd:string"),
                ("URL", "xsd:string"),
                ("ProviderName", "xsd:string"),
                ("ProviderType", "xsd:string"),
                ("AuthenticationMode", "xsd:string"),
            ],
            31,
        ),
        (
            "DISCOVER_PROPERTIES",
            "4b40adfb-8b09-4758-97bb-636e8ae97bcf",
            &[("PropertyName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_SCHEMA_ROWSETS",
            "eea0302b-7922-4992-8991-0e605d0e5593",
            &[("SchemaName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_ENUMERATORS",
            "55a9e78b-accb-45b4-95a6-94c5065617a7",
            &[("EnumName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_KEYWORDS",
            "1426c443-4cdd-4a40-8f45-572fab9bbaa1",
            &[("Keyword", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_LITERALS",
            "c3ef5ecb-0a07-4665-a140-b075722dbdc2",
            &[("LiteralName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_XML_METADATA",
            "3444b255-171e-4cb9-ad98-19e57888a75f",
            &[
                ("DatabaseID", "xsd:string"),
                ("DimensionID", "xsd:string"),
                ("CubeID", "xsd:string"),
                ("MeasureGroupID", "xsd:string"),
                ("PartitionID", "xsd:string"),
                ("PerspectiveID", "xsd:string"),
                ("DimensionPermissionID", "xsd:string"),
                ("RoleID", "xsd:string"),
                ("DatabasePermissionID", "xsd:string"),
                ("DataSourceID", "xsd:string"),
                ("AggregationDesignID", "xsd:string"),
                ("TraceID", "xsd:string"),
                ("CubePermissionID", "xsd:string"),
                ("AssemblyID", "xsd:string"),
                ("MdxScriptID", "xsd:string"),
                ("DataSourceViewID", "xsd:string"),
                ("DataSourcePermissionID", "xsd:string"),
                ("CalculatedColumns", "xsd:string"),
                ("ObjectExpansion", "xsd:string"),
                ("DBWorkloadGroupID", "xsd:string"),
                ("ResourcePoolID", "xsd:string"),
                ("ModifiedAfter", "xsd:dateTime"),
            ],
            67108863,
        ),
        (
            "DISCOVER_TRACES",
            "a07ccd1a-8148-11d0-87bb-00c04fc33942",
            &[("TraceID", "xsd:string"), ("Type", "xsd:string")],
            3,
        ),
        (
            "DISCOVER_TRACE_DEFINITION_PROVIDERINFO",
            "a07ccd1b-8148-11d0-87bb-00c04fc33942",
            &[("Data", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_XEVENT_PACKAGES",
            "a07ccd1c-8148-11d0-87bb-00c04fc33942",
            &[("NAME", "xsd:string"), ("ID", "uuid")],
            3,
        ),
        (
            "DISCOVER_XEVENT_OBJECTS",
            "a07ccd1d-8148-11d0-87bb-00c04fc33942",
            &[("NAME", "xsd:string"), ("OBJECT_TYPE", "xsd:string")],
            3,
        ),
        (
            "DISCOVER_XEVENT_OBJECT_COLUMNS",
            "a07ccd1e-8148-11d0-87bb-00c04fc33942",
            &[("OBJECT_NAME", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_XEVENT_SESSION_TARGETS",
            "a07ccd1f-8148-11d0-87bb-00c04fc33942",
            &[("XESessionName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_XEVENT_SESSIONS",
            "a07ccd20-8148-11d0-87bb-00c04fc33942",
            &[("XESessionName", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_TRACE_COLUMNS",
            "a07ccd18-8148-11d0-87bb-00c04fc33942",
            &[("Data", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_TRACE_EVENT_CATEGORIES",
            "a07ccd19-8148-11d0-87bb-00c04fc33942",
            &[("Data", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_MEMORYUSAGE",
            "a07ccd21-8148-11d0-87bb-00c04fc33942",
            &[
                ("SPID", "xsd:unsignedInt"),
                ("MemoryUsed", "xsd:long"),
                ("BaseObjectType", "xsd:unsignedInt"),
                ("Shrinkable", "xsd:boolean"),
            ],
            15,
        ),
        (
            "DISCOVER_MEMORYGRANT",
            "a07ccd23-8148-11d0-87bb-00c04fc33942",
            &[("SPID", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_LOCKS",
            "a07ccd24-8148-11d0-87bb-00c04fc33942",
            &[
                ("SPID", "xsd:int"),
                ("LOCK_TRANSACTION_ID", "uuid"),
                ("LOCK_OBJECT_ID", "xsd:string"),
                ("LOCK_STATUS", "xsd:int"),
                ("LOCK_TYPE", "xsd:int"),
                ("LOCK_MIN_TOTAL_MS", "xsd:long"),
            ],
            63,
        ),
        (
            "DISCOVER_CONNECTIONS",
            "a07ccd25-8148-11d0-87bb-00c04fc33942",
            &[
                ("CONNECTION_ID", "xsd:int"),
                ("CONNECTION_USER_NAME", "xsd:string"),
                ("CONNECTION_IMPERSONATED_USER_NAME", "xsd:string"),
                ("CONNECTION_HOST_NAME", "xsd:string"),
                ("CONNECTION_ELAPSED_TIME_MS", "xsd:long"),
                ("CONNECTION_LAST_COMMAND_ELAPSED_TIME_MS", "xsd:long"),
                ("CONNECTION_IDLE_TIME_MS", "xsd:long"),
            ],
            127,
        ),
        (
            "DISCOVER_SESSIONS",
            "a07ccd26-8148-11d0-87bb-00c04fc33942",
            &[
                ("SESSION_ID", "xsd:string"),
                ("SESSION_SPID", "xsd:int"),
                ("SESSION_CONNECTION_ID", "xsd:int"),
                ("SESSION_USER_NAME", "xsd:string"),
                ("SESSION_CURRENT_DATABASE", "xsd:string"),
                ("SESSION_ELAPSED_TIME_MS", "xsd:unsignedLong"),
                ("SESSION_CPU_TIME_MS", "xsd:unsignedLong"),
                ("SESSION_IDLE_TIME_MS", "xsd:unsignedLong"),
                ("SESSION_STATUS", "xsd:int"),
                ("RESTRICT_CATALOG_ID", "xsd:string"),
                ("REQUEST_ACTIVITY_ID", "uuid"),
                ("CLIENT_ACTIVITY_ID", "uuid"),
            ],
            4095,
        ),
        (
            "DISCOVER_JOBS",
            "a07ccd27-8148-11d0-87bb-00c04fc33942",
            &[
                ("SPID", "xsd:int"),
                ("JOB_ID", "xsd:int"),
                ("JOB_DESCRIPTION", "xsd:string"),
                ("JOB_THREADPOOL_ID", "xsd:int"),
                ("JOB_MIN_TOTAL_TIME_MS", "xsd:long"),
            ],
            31,
        ),
        (
            "DISCOVER_TRANSACTIONS",
            "a07ccd28-8148-11d0-87bb-00c04fc33942",
            &[
                ("TRANSACTION_ID", "xsd:string"),
                ("TRANSACTION_SESSION_ID", "xsd:string"),
            ],
            3,
        ),
        (
            "DISCOVER_DB_CONNECTIONS",
            "a07ccd2a-8148-11d0-87bb-00c04fc33942",
            &[
                ("CONNECTION_ID", "xsd:int"),
                ("CONNECTION_IN_USE", "xsd:int"),
                ("CONNECTION_SERVER_NAME", "xsd:string"),
                ("CONNECTION_CATALOG_NAME", "xsd:string"),
                ("CONNECTION_SPID", "xsd:int"),
            ],
            31,
        ),
        (
            "DISCOVER_MASTER_KEY",
            "a07ccd29-8148-11d0-87bb-00c04fc33942",
            &[("KEY", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_PERFORMANCE_COUNTERS",
            "a07ccd2e-8148-11d0-87bb-00c04fc33942",
            &[("PERF_COUNTER_NAME", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_LOCATIONS",
            "a07ccd92-8148-11d0-87bb-00c04fc33942",
            &[
                ("LOCATION_BACKUP_FILE_PATHNAME", "xsd:string"),
                ("LOCATION_PASSWORD", "xsd:string"),
            ],
            3,
        ),
        (
            "DISCOVER_POWERBI_ROLES",
            "a07ccd8b-8148-11d0-87bb-00c04fc33942",
            &[
                ("ID", "xsd:string"),
                ("LINEAGE_NAME", "xsd:string"),
                ("NAME", "xsd:string"),
            ],
            7,
        ),
        (
            "DISCOVER_POWERBI_DATASOURCES",
            "a07ccd8d-8148-11d0-87bb-00c04fc33942",
            &[("ID", "xsd:string"), ("NAME", "xsd:string")],
            3,
        ),
        (
            "DISCOVER_PARTITION_DIMENSION_STAT",
            "a07ccd8e-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_GROUP_NAME", "xsd:string"),
                ("PARTITION_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "DISCOVER_PARTITION_STAT",
            "a07ccd8f-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_GROUP_NAME", "xsd:string"),
                ("PARTITION_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "DISCOVER_DIMENSION_STAT",
            "a07ccd90-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("DIMENSION_NAME", "xsd:string"),
            ],
            3,
        ),
        (
            "DISCOVER_M_EXPRESSIONS",
            "a07ccd93-8148-11d0-87bb-00c04fc33942",
            &[],
            0,
        ),
        (
            "DISCOVER_MODEL_SECURITY",
            "a07ccd88-8148-11d0-87bb-00c04fc33942",
            &[("DatabaseID", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_OBJECT_COUNTERS",
            "a07ccd89-8148-11d0-87bb-00c04fc33942",
            &[],
            0,
        ),
        (
            "DISCOVER_MEM_STATS",
            "a07ccd8a-8148-11d0-87bb-00c04fc33942",
            &[("CATALOG_NAME", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_DB_MEM_STATS",
            "a07ccd8c-8148-11d0-87bb-00c04fc33942",
            &[("CATALOG_NAME", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_COMMANDS",
            "a07ccd34-8148-11d0-87bb-00c04fc33942",
            &[("SESSION_SPID", "xsd:int")],
            1,
        ),
        (
            "DISCOVER_COMMAND_OBJECTS",
            "a07ccd35-8148-11d0-87bb-00c04fc33942",
            &[
                ("SESSION_SPID", "xsd:int"),
                ("SESSION_ID", "xsd:string"),
                ("OBJECT_PARENT_PATH", "xsd:string"),
                ("OBJECT_ID", "xsd:string"),
            ],
            15,
        ),
        (
            "DISCOVER_OBJECT_ACTIVITY",
            "a07ccd36-8148-11d0-87bb-00c04fc33942",
            &[
                ("OBJECT_PARENT_PATH", "xsd:string"),
                ("OBJECT_ID", "xsd:string"),
            ],
            3,
        ),
        (
            "DISCOVER_OBJECT_MEMORY_USAGE",
            "a07ccd37-8148-11d0-87bb-00c04fc33942",
            &[
                ("OBJECT_PARENT_PATH", "xsd:string"),
                ("OBJECT_ID", "xsd:string"),
            ],
            3,
        ),
        (
            "DISCOVER_STORAGE_TABLES",
            "a07ccd43-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_GROUP_NAME", "xsd:string"),
            ],
            7,
        ),
        (
            "DISCOVER_STORAGE_TABLE_COLUMNS",
            "a07ccd44-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_GROUP_NAME", "xsd:string"),
            ],
            7,
        ),
        (
            "DISCOVER_STORAGE_TABLE_COLUMN_SEGMENTS",
            "a07ccd45-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("CUBE_NAME", "xsd:string"),
                ("MEASURE_GROUP_NAME", "xsd:string"),
                ("PARTITION_NAME", "xsd:string"),
            ],
            15,
        ),
        (
            "DISCOVER_CALC_DEPENDENCY",
            "a07ccd46-8148-11d0-87bb-00c04fc33942",
            &[
                ("DATABASE_NAME", "xsd:string"),
                ("OBJECT_TYPE", "xsd:string"),
                ("QUERY", "xsd:string"),
                ("KIND", "xsd:string"),
                ("OBJECT_CATEGORY", "xsd:string"),
            ],
            31,
        ),
        (
            "DISCOVER_CSDL_METADATA",
            "87b86062-21c3-460f-b4f8-5be98394f13b",
            &[
                ("CATALOG_NAME", "xsd:string"),
                ("PERSPECTIVE_NAME", "xsd:string"),
                ("VERSION", "xsd:string"),
                ("IGNORE_TRANSLATIONS", "xsd:boolean"),
                ("PRINT_ALL_TRANSLATIONS", "xsd:boolean"),
            ],
            31,
        ),
        (
            "DISCOVER_RESOURCE_POOLS",
            "a07ccd47-8148-11d0-87bb-00c04fc33942",
            &[("ResourcePoolID", "xsd:string")],
            1,
        ),
        (
            "DISCOVER_RING_BUFFERS",
            "a07ccd48-8148-11d0-87bb-00c04fc33942",
            &[("XESessionName", "xsd:string")],
            1,
        ),
        // Tabular Model schemas — presence signals to PBI that this is a Tabular/DirectQuery server
        (
            "TMSCHEMA_MODEL",
            "a07ccd49-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("StorageLocation", "xsd:string"),
                ("DefaultMode", "xsd:long"),
                ("DefaultDataView", "xsd:long"),
                ("Culture", "xsd:string"),
                ("Collation", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
                ("DefaultMeasureID", "xsd:unsignedLong"),
                ("DefaultPowerBIDataSourceVersion", "xsd:long"),
                ("ForceUniqueNames", "xsd:boolean"),
                ("DiscourageImplicitMeasures", "xsd:boolean"),
                ("DataSourceVariablesOverrideBehavior", "xsd:long"),
                ("DataSourceDefaultMaxConnections", "xsd:int"),
                ("SourceQueryCulture", "xsd:string"),
                ("MAttributes", "xsd:string"),
                ("DiscourageCompositeModels", "xsd:boolean"),
                ("MaxParallelismPerRefresh", "xsd:int"),
            ],
            4194303,
        ),
        (
            "TMSCHEMA_DATA_SOURCES",
            "a07ccd4a-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("Type", "xsd:long"),
                ("ImpersonationMode", "xsd:long"),
                ("Account", "xsd:string"),
                ("MaxConnections", "xsd:int"),
                ("Isolation", "xsd:long"),
                ("Timeout", "xsd:int"),
                ("Provider", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("ConnectionDetails", "xsd:string"),
                ("Options", "xsd:string"),
            ],
            32767,
        ),
        (
            "TMSCHEMA_TABLES",
            "a07ccd4b-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("SystemObjectType", "xsd:int"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("DataCategory", "xsd:string"),
                ("Description", "xsd:string"),
                ("IsHidden", "xsd:boolean"),
                ("TableStorageID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
                ("SystemFlags", "xsd:long"),
                ("ShowAsVariationsOnly", "xsd:boolean"),
                ("IsPrivate", "xsd:boolean"),
                ("DefaultDetailRowsDefinitionID", "xsd:unsignedLong"),
                ("AlternateSourcePrecedence", "xsd:int"),
                ("RefreshPolicyID", "xsd:unsignedLong"),
                ("CalculationGroupID", "xsd:unsignedLong"),
                ("ExcludeFromModelRefresh", "xsd:boolean"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
                ("SystemManaged", "xsd:boolean"),
                ("ExcludeFromAutomaticAggregations", "xsd:boolean"),
                ("DirectLakeIndexingBehavior", "xsd:long"),
            ],
            33554431,
        ),
        (
            "TMSCHEMA_COLUMNS",
            "a07ccd4c-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("SystemObjectType", "xsd:int"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("ExplicitName", "xsd:string"),
                ("InferredName", "xsd:string"),
                ("ExplicitDataType", "xsd:long"),
                ("InferredDataType", "xsd:long"),
                ("DataCategory", "xsd:string"),
                ("Description", "xsd:string"),
                ("IsHidden", "xsd:boolean"),
                ("State", "xsd:long"),
                ("IsUnique", "xsd:boolean"),
                ("IsKey", "xsd:boolean"),
                ("IsNullable", "xsd:boolean"),
                ("Alignment", "xsd:long"),
                ("TableDetailPosition", "xsd:int"),
                ("IsDefaultLabel", "xsd:boolean"),
                ("IsDefaultImage", "xsd:boolean"),
                ("SummarizeBy", "xsd:long"),
                ("ColumnStorageID", "xsd:unsignedLong"),
                ("Type", "xsd:long"),
                ("SourceColumn", "xsd:string"),
                ("ColumnOriginID", "xsd:unsignedLong"),
                ("Expression", "xsd:string"),
                ("FormatString", "xsd:string"),
                ("IsAvailableInMDX", "xsd:boolean"),
                ("SortByColumnID", "xsd:unsignedLong"),
                ("AttributeHierarchyID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
                ("RefreshedTime", "xsd:dateTime"),
                ("RefreshedTimeOp", "xsd:int"),
                ("SystemFlags", "xsd:long"),
                ("KeepUniqueRows", "xsd:boolean"),
                ("DisplayOrdinal", "xsd:int"),
                ("SourceProviderType", "xsd:string"),
                ("DisplayFolder", "xsd:string"),
                ("EncodingHint", "xsd:long"),
                ("RelatedColumnDetailsID", "xsd:unsignedLong"),
                ("AlternateOfID", "xsd:unsignedLong"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
                ("ExpressionContext", "xsd:long"),
                ("StringIndexingBehavior", "xsd:long"),
            ],
            140737488355327,
        ),
        (
            "TMSCHEMA_ATTRIBUTE_HIERARCHIES",
            "a07ccd4d-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ColumnID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("State", "xsd:long"),
                ("AttributeHierarchyStorageID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("RefreshedTime", "xsd:dateTime"),
                ("RefreshedTimeOp", "xsd:int"),
            ],
            1023,
        ),
        (
            "TMSCHEMA_PARTITIONS",
            "a07ccd4e-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("SystemObjectType", "xsd:int"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("DataSourceID", "xsd:unsignedLong"),
                ("QueryDefinition", "xsd:string"),
                ("State", "xsd:long"),
                ("Type", "xsd:long"),
                ("PartitionStorageID", "xsd:unsignedLong"),
                ("Mode", "xsd:long"),
                ("DataView", "xsd:long"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("RefreshedTime", "xsd:dateTime"),
                ("RefreshedTimeOp", "xsd:int"),
                ("SystemFlags", "xsd:long"),
                ("RetainDataTillForceCalculate", "xsd:boolean"),
                ("RangeStart", "xsd:dateTime"),
                ("RangeEnd", "xsd:dateTime"),
                ("RangeGranularity", "xsd:long"),
                ("RefreshBookmark", "xsd:string"),
                ("QueryGroupID", "xsd:unsignedLong"),
                ("ExpressionSourceID", "xsd:unsignedLong"),
                ("MAttributes", "xsd:string"),
                ("SchemaName", "xsd:string"),
            ],
            134217727,
        ),
        (
            "TMSCHEMA_RELATIONSHIPS",
            "a07ccd4f-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("IsActive", "xsd:boolean"),
                ("Type", "xsd:long"),
                ("CrossFilteringBehavior", "xsd:long"),
                ("JoinOnDateBehavior", "xsd:long"),
                ("RelyOnReferentialIntegrity", "xsd:boolean"),
                ("FromTableID", "xsd:unsignedLong"),
                ("FromColumnID", "xsd:unsignedLong"),
                ("FromCardinality", "xsd:long"),
                ("ToTableID", "xsd:unsignedLong"),
                ("ToColumnID", "xsd:unsignedLong"),
                ("ToCardinality", "xsd:long"),
                ("State", "xsd:long"),
                ("RelationshipStorageID", "xsd:unsignedLong"),
                ("RelationshipStorage2ID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("RefreshedTime", "xsd:dateTime"),
                ("RefreshedTimeOp", "xsd:int"),
                ("SecurityFilteringBehavior", "xsd:long"),
            ],
            4194303,
        ),
        (
            "TMSCHEMA_MEASURES",
            "a07ccd50-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("DataType", "xsd:long"),
                ("Expression", "xsd:string"),
                ("FormatString", "xsd:string"),
                ("IsHidden", "xsd:boolean"),
                ("State", "xsd:long"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
                ("KPIID", "xsd:unsignedLong"),
                ("IsSimpleMeasure", "xsd:boolean"),
                ("DisplayFolder", "xsd:string"),
                ("DetailRowsDefinitionID", "xsd:unsignedLong"),
                ("DataCategory", "xsd:string"),
                ("FormatStringDefinitionID", "xsd:unsignedLong"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
            ],
            4194303,
        ),
        (
            "TMSCHEMA_HIERARCHIES",
            "a07ccd51-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("IsHidden", "xsd:boolean"),
                ("State", "xsd:long"),
                ("HierarchyStorageID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
                ("RefreshedTime", "xsd:dateTime"),
                ("RefreshedTimeOp", "xsd:int"),
                ("DisplayFolder", "xsd:string"),
                ("HideMembers", "xsd:long"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
            ],
            262143,
        ),
        (
            "TMSCHEMA_LEVELS",
            "a07ccd52-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("HierarchyID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Ordinal", "xsd:int"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("ColumnID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
            ],
            4095,
        ),
        (
            "TMSCHEMA_ANNOTATIONS",
            "a07ccd53-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("ObjectID", "xsd:unsignedLong"),
                ("ObjectType", "xsd:int"),
                ("Name", "xsd:string"),
                ("Value", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            255,
        ),
        (
            "TMSCHEMA_KPIS",
            "a07ccd5f-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("MeasureID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Description", "xsd:string"),
                ("TargetDescription", "xsd:string"),
                ("TargetExpression", "xsd:string"),
                ("TargetFormatString", "xsd:string"),
                ("StatusGraphic", "xsd:string"),
                ("StatusDescription", "xsd:string"),
                ("StatusExpression", "xsd:string"),
                ("TrendGraphic", "xsd:string"),
                ("TrendDescription", "xsd:string"),
                ("TrendExpression", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            65535,
        ),
        (
            "TMSCHEMA_CULTURES",
            "a07ccd63-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("LinguisticMetadataID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("StructureModifiedTime", "xsd:dateTime"),
                ("StructureModifiedTimeOp", "xsd:int"),
            ],
            255,
        ),
        (
            "TMSCHEMA_OBJECT_TRANSLATIONS",
            "a07ccd64-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("CultureID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("ObjectID", "xsd:unsignedLong"),
                ("ObjectType", "xsd:int"),
                ("Property", "xsd:long"),
                ("Value", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            511,
        ),
        (
            "TMSCHEMA_LINGUISTIC_METADATA",
            "a07ccd65-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("CultureID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            31,
        ),
        (
            "TMSCHEMA_PERSPECTIVES",
            "a07ccd66-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            63,
        ),
        (
            "TMSCHEMA_PERSPECTIVE_TABLES",
            "a07ccd67-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("PerspectiveID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("IncludeAll", "xsd:boolean"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_PERSPECTIVE_COLUMNS",
            "a07ccd68-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("PerspectiveTableID", "xsd:unsignedLong"),
                ("PerspectiveID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("ColumnID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_PERSPECTIVE_HIERARCHIES",
            "a07ccd69-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("PerspectiveTableID", "xsd:unsignedLong"),
                ("PerspectiveID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("HierarchyID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_PERSPECTIVE_MEASURES",
            "a07ccd6a-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("PerspectiveTableID", "xsd:unsignedLong"),
                ("PerspectiveID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("MeasureID", "xsd:unsignedLong"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_ROLES",
            "a07ccd6b-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("ModelPermission", "xsd:long"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_ROLE_MEMBERSHIPS",
            "a07ccd6c-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("RoleID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("MemberName", "xsd:string"),
                ("MemberID", "xsd:string"),
                ("IdentityProvider", "xsd:string"),
                ("MemberType", "xsd:long"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
            ],
            511,
        ),
        (
            "TMSCHEMA_TABLE_PERMISSIONS",
            "a07ccd6d-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("RoleID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("FilterExpression", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("State", "xsd:long"),
                ("MetadataPermission", "xsd:long"),
            ],
            511,
        ),
        (
            "TMSCHEMA_VARIATIONS",
            "a07ccd6e-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ColumnID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("RelationshipID", "xsd:unsignedLong"),
                ("DefaultHierarchyID", "xsd:unsignedLong"),
                ("DefaultColumnID", "xsd:unsignedLong"),
                ("IsDefault", "xsd:boolean"),
            ],
            1023,
        ),
        (
            "TMSCHEMA_EXPRESSIONS",
            "a07ccd72-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("Kind", "xsd:long"),
                ("Expression", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("QueryGroupID", "xsd:unsignedLong"),
                ("ParameterValuesColumnID", "xsd:unsignedLong"),
                ("MAttributes", "xsd:string"),
                ("LineageTag", "xsd:string"),
                ("SourceLineageTag", "xsd:string"),
                ("RemoteParameterName", "xsd:string"),
                ("ExpressionSourceID", "xsd:unsignedLong"),
            ],
            32767,
        ),
        (
            "TMSCHEMA_DETAIL_ROWS_DEFINITIONS",
            "a07ccd54-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("ID", "xsd:unsignedLong"),
                ("ObjectID", "xsd:unsignedLong"),
                ("ObjectType", "xsd:int"),
                ("Expression", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("State", "xsd:long"),
            ],
            255,
        ),
        (
            "TMSCHEMA_CALCULATION_GROUPS",
            "a07ccd76-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("Description", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("Precedence", "xsd:int"),
            ],
            127,
        ),
        (
            "TMSCHEMA_CALCULATION_ITEMS",
            "a07ccd77-8148-11d0-87bb-00c04fc33942",
            &[
                ("DatabaseName", "xsd:string"),
                ("CalculationGroupID", "xsd:unsignedLong"),
                ("TableID", "xsd:unsignedLong"),
                ("ID", "xsd:unsignedLong"),
                ("FormatStringDefinitionID", "xsd:unsignedLong"),
                ("Name", "xsd:string"),
                ("Description", "xsd:string"),
                ("ModifiedTime", "xsd:dateTime"),
                ("ModifiedTimeOp", "xsd:int"),
                ("State", "xsd:long"),
                ("Expression", "xsd:string"),
                ("Ordinal", "xsd:int"),
            ],
            4095,
        ),
        (
            "MDSCHEMA_FUNCTIONS",
            "a07ccd07-8148-11d0-87bb-00c04fc33942",
            &[
                ("LIBRARY_NAME", "xsd:string"),
                ("INTERFACE_NAME", "xsd:string"),
                ("FUNCTION_NAME", "xsd:string"),
                ("ORIGIN", "xsd:int"),
                ("CATALOG_NAME", "xsd:string"),
            ],
            31,
        ),
        (
            "DISCOVER_INSTANCES",
            "20518699-2474-4c15-9885-0e947ec7a7e3",
            &[("INSTANCE_NAME", "xsd:string")],
            1,
        ),
    ];

    let schema = concat!(
        r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql""#,
        r#" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified">"#,
        r#"<xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded">"#,
        r#"<xsd:element name="row" type="row"/>"#,
        r#"</xsd:sequence></xsd:complexType></xsd:element>"#,
        r#"<xsd:simpleType name="uuid"><xsd:restriction base="xsd:string">"#,
        r#"<xsd:pattern value="[0-9a-zA-Z]{8}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{4}-[0-9a-zA-Z]{12}"/>"#,
        r#"</xsd:restriction></xsd:simpleType>"#,
        r#"<xsd:complexType name="row"><xsd:sequence>"#,
        r#"<xsd:element sql:field="SchemaName" name="SchemaName" type="xsd:string"/>"#,
        r#"<xsd:element sql:field="SchemaGuid" name="SchemaGuid" type="uuid" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="Restrictions" name="Restrictions" minOccurs="0" maxOccurs="unbounded">"#,
        r#"<xsd:complexType><xsd:sequence>"#,
        r#"<xsd:element sql:field="Name" name="Name" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="Type" name="Type" type="xsd:string" minOccurs="0"/>"#,
        r#"</xsd:sequence></xsd:complexType>"#,
        r#"</xsd:element>"#,
        r#"<xsd:element sql:field="Description" name="Description" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="RestrictionsMask" name="RestrictionsMask" type="xsd:unsignedLong" minOccurs="0"/>"#,
        r#"</xsd:sequence></xsd:complexType>"#,
        r#"</xsd:schema>"#,
    );

    let mut rows = String::new();
    for (name, guid, restrictions, mask) in schemas {
        if let Some(filter) = schema_name {
            if !name.eq_ignore_ascii_case(filter) {
                continue;
            }
        }
        let mut restr_xml = String::new();
        for (rname, rtype) in *restrictions {
            restr_xml.push_str(&format!(
                "<Restrictions><Name>{rname}</Name><Type>{rtype}</Type></Restrictions>"
            ));
        }
        rows.push_str(&format!(
            "<row><SchemaName>{name}</SchemaName><SchemaGuid>{guid}</SchemaGuid>{restr_xml}<Description/><RestrictionsMask>{mask}</RestrictionsMask></row>"
        ));
    }

    ok_xml(session_id, rowset(schema, &rows))
}

pub fn discover_catalogs(
    session_id: Option<&str>,
    databases: &[DatabaseMeta],
) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("DESCRIPTION", "string"),
        ("ROLES", "string"),
        ("DATE_MODIFIED", "dateTime"),
        ("COMPATIBILITY_LEVEL", "int"),
        ("TYPE", "int"),
        ("VERSION", "long"),
        ("DATABASE_ID", "string"),
        ("DATABASE_GUID", "string"),
        ("DATE_QUERIED", "dateTime"),
        ("CURRENTLY_USED", "boolean"),
        ("POPULARITY", "float"),
        ("WEIGHTEDPOPULARITY", "double"),
        ("CLIENTCACHEREFRESHPOLICY", "unsignedInt"),
        ("ENCRYPTION_LEVEL", "string"),
        ("CRYPTOKEY_UPDATED", "dateTime"),
    ]);
    let mut rows = String::new();
    for db in databases {
        let name = xml_escape_value(&db.name);
        let id = xml_escape_value(&db.id);
        rows.push_str(&format!(
            "<row><CATALOG_NAME>{name}</CATALOG_NAME><DESCRIPTION/><COMPATIBILITY_LEVEL>{compat}</COMPATIBILITY_LEVEL><DATABASE_ID>{id}</DATABASE_ID></row>",
            compat = CATALOG_COMPAT_LEVEL,
        ));
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn discover_cubes(
    session_id: Option<&str>,
    cube_source_restriction: Option<u16>,
    databases: &[DatabaseMeta],
) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("CUBE_TYPE", "string"),
        ("CUBE_GUID", "string"),
        ("CREATED_ON", "dateTime"),
        ("LAST_SCHEMA_UPDATE", "dateTime"),
        ("SCHEMA_UPDATED_BY", "string"),
        ("LAST_DATA_UPDATE", "dateTime"),
        ("DATA_UPDATED_BY", "string"),
        ("DESCRIPTION", "string"),
        ("IS_DRILLTHROUGH_ENABLED", "boolean"),
        ("IS_LINKABLE", "boolean"),
        ("IS_WRITE_ENABLED", "boolean"),
        ("IS_SQL_ENABLED", "boolean"),
        ("CUBE_CAPTION", "string"),
        ("BASE_CUBE_NAME", "string"),
        ("CUBE_SOURCE", "unsignedShort"),
        ("PREFERRED_QUERY_PATTERNS", "unsignedShort"),
    ]);
    const OUR_CUBE_SOURCE: u16 = 1;
    let mut rows = String::new();
    if cube_source_restriction.is_none_or(|r| r & OUR_CUBE_SOURCE != 0) {
        for db in databases {
            let catalog_name = xml_escape_value(&db.name);
            let last_schema = xml_escape_value(&db.last_schema_update);
            let last_data = xml_escape_value(&db.last_refreshed);
            rows.push_str(&format!(
                "<row>\
                <CATALOG_NAME>{catalog_name}</CATALOG_NAME>\
                <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
                <CUBE_TYPE>CUBE</CUBE_TYPE>\
                <LAST_SCHEMA_UPDATE>{last_schema}</LAST_SCHEMA_UPDATE>\
                <LAST_DATA_UPDATE>{last_data}</LAST_DATA_UPDATE>\
                <DESCRIPTION/>\
                <IS_DRILLTHROUGH_ENABLED>true</IS_DRILLTHROUGH_ENABLED>\
                <IS_LINKABLE>false</IS_LINKABLE>\
                <IS_WRITE_ENABLED>false</IS_WRITE_ENABLED>\
                <IS_SQL_ENABLED>false</IS_SQL_ENABLED>\
                <CUBE_CAPTION>{CUBE_NAME}</CUBE_CAPTION>\
                <CUBE_SOURCE>1</CUBE_SOURCE>\
                <PREFERRED_QUERY_PATTERNS>7</PREFERRED_QUERY_PATTERNS>\
                </row>"
            ));
        }
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn discover_measures(
    session_id: Option<&str>,
    catalog: &str,
    measures: &[MeasureMeta],
) -> (String, Response) {
    let schema = make_schema(MEASURES_FULL_SCHEMA);
    let rows = measure_rows(catalog, measures);
    ok_xml(session_id, rowset(&schema, &rows))
}

fn measure_rows(catalog: &str, measures: &[MeasureMeta]) -> String {
    let cat = xml_escape_value(catalog);
    let mut rows = String::new();
    for m in measures {
        let name = xml_escape_value(&m.name);
        let display = xml_escape_value(&m.display_name);
        let tblname = xml_escape_value(&m.table_name);
        let fmt = m
            .format_string
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(xml_escape_value)
            .unwrap_or_else(|| "0".into());
        let desc_elem = match m.description.as_deref().filter(|s| !s.is_empty()) {
            Some(d) => format!("<DESCRIPTION>{}</DESCRIPTION>", xml_escape_value(d)),
            None => "<DESCRIPTION/>".into(),
        };
        let folder_elem = match m.display_folder.as_deref().filter(|s| !s.is_empty()) {
            Some(f) => format!(
                "<MEASURE_DISPLAY_FOLDER>{}</MEASURE_DISPLAY_FOLDER>",
                xml_escape_value(f)
            ),
            None => "<MEASURE_DISPLAY_FOLDER/>".into(),
        };
        let expr = xml_escape_value(&m.expression);
        rows.push_str(&format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <MEASURE_NAME>{name}</MEASURE_NAME>\
            <MEASURE_UNIQUE_NAME>[Measures].[{name}]</MEASURE_UNIQUE_NAME>\
            <MEASURE_CAPTION>{display}</MEASURE_CAPTION>\
            <MEASURE_AGGREGATOR>{agg}</MEASURE_AGGREGATOR>\
            <DATA_TYPE>{dt}</DATA_TYPE>\
            <NUMERIC_PRECISION>65535</NUMERIC_PRECISION>\
            <NUMERIC_SCALE>-1</NUMERIC_SCALE>\
            {desc_elem}\
            <EXPRESSION>{expr}</EXPRESSION>\
            <MEASURE_IS_VISIBLE>{visible}</MEASURE_IS_VISIBLE>\
            <MEASURE_NAME_SQL_COLUMN_NAME>{name}</MEASURE_NAME_SQL_COLUMN_NAME>\
            <MEASURE_UNQUALIFIED_CAPTION>{display}</MEASURE_UNQUALIFIED_CAPTION>\
            <MEASUREGROUP_NAME>{tblname}</MEASUREGROUP_NAME>\
            {folder_elem}\
            <DEFAULT_FORMAT_STRING>{fmt}</DEFAULT_FORMAT_STRING>\
            </row>",
            agg = m.aggregator,
            dt = m.data_type,
            visible = !m.is_hidden,
        ));
    }
    rows.push_str(&format!(
        "<row>\
        <CATALOG_NAME>{cat}</CATALOG_NAME>\
        <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
        <MEASURE_NAME>__Default measure</MEASURE_NAME>\
        <MEASURE_UNIQUE_NAME>[Measures].[__Default measure]</MEASURE_UNIQUE_NAME>\
        <MEASURE_CAPTION>__Default measure</MEASURE_CAPTION>\
        <MEASURE_AGGREGATOR>127</MEASURE_AGGREGATOR>\
        <DATA_TYPE>12</DATA_TYPE>\
        <NUMERIC_PRECISION>65535</NUMERIC_PRECISION>\
        <NUMERIC_SCALE>-1</NUMERIC_SCALE>\
        <DESCRIPTION/>\
        <EXPRESSION>1</EXPRESSION>\
        <MEASURE_IS_VISIBLE>false</MEASURE_IS_VISIBLE>\
        <MEASURE_NAME_SQL_COLUMN_NAME>__Default measure</MEASURE_NAME_SQL_COLUMN_NAME>\
        <MEASURE_UNQUALIFIED_CAPTION>__Default measure</MEASURE_UNQUALIFIED_CAPTION>\
        <MEASURE_DISPLAY_FOLDER/>\
        </row>"
    ));
    rows
}

pub fn discover_mdschema_properties(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    property_type: Option<u16>,
) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("DIMENSION_UNIQUE_NAME", "string"),
        ("HIERARCHY_UNIQUE_NAME", "string"),
        ("LEVEL_UNIQUE_NAME", "string"),
        ("MEMBER_UNIQUE_NAME", "string"),
        ("PROPERTY_TYPE", "short"),
        ("PROPERTY_NAME", "string"),
        ("PROPERTY_CAPTION", "string"),
        ("DATA_TYPE", "unsignedShort"),
        ("CHARACTER_MAXIMUM_LENGTH", "unsignedInt"),
        ("CHARACTER_OCTET_LENGTH", "unsignedInt"),
        ("NUMERIC_PRECISION", "unsignedShort"),
        ("NUMERIC_SCALE", "short"),
        ("DESCRIPTION", "string"),
        ("PROPERTY_CONTENT_TYPE", "short"),
        ("SQL_COLUMN_NAME", "string"),
        ("LANGUAGE", "unsignedShort"),
        ("PROPERTY_ORIGIN", "unsignedShort"),
        ("PROPERTY_ATTRIBUTE_HIERARCHY_NAME", "string"),
        ("PROPERTY_CARDINALITY", "string"),
        ("MIME_TYPE", "string"),
        ("PROPERTY_IS_VISIBLE", "boolean"),
    ]);

    static CELL_PROPS: &[(&str, u16)] = &[
        ("VALUE", 12),
        ("FORMAT_STRING", 130),
        ("BACK_COLOR", 19),
        ("FORE_COLOR", 19),
        ("FONT_NAME", 130),
        ("FONT_SIZE", 18),
        ("FONT_FLAGS", 3),
        ("LANGUAGE", 19),
        ("CELL_ORDINAL", 19),
        ("FORMATTED_VALUE", 130),
        ("ACTION_TYPE", 19),
        ("UPDATEABLE", 19),
    ];

    let rows = match property_type {
        Some(2) => CELL_PROPS
            .iter()
            .map(|(name, dt)| {
                format!(
                    "<row>\
                    <PROPERTY_TYPE>2</PROPERTY_TYPE>\
                    <PROPERTY_NAME>{name}</PROPERTY_NAME>\
                    <PROPERTY_CAPTION>{name}</PROPERTY_CAPTION>\
                    <DATA_TYPE>{dt}</DATA_TYPE>\
                    </row>"
                )
            })
            .collect::<String>(),
        Some(5) => {
            let mut out = String::new();
            out.push_str(&format!(
                "<row>\
                <CATALOG_NAME>{catalog}</CATALOG_NAME>\
                <CUBE_NAME>Model</CUBE_NAME>\
                <DIMENSION_UNIQUE_NAME>[Measures]</DIMENSION_UNIQUE_NAME>\
                <HIERARCHY_UNIQUE_NAME>[Measures]</HIERARCHY_UNIQUE_NAME>\
                <LEVEL_UNIQUE_NAME>[Measures].[MeasuresLevel]</LEVEL_UNIQUE_NAME>\
                <PROPERTY_TYPE>5</PROPERTY_TYPE>\
                <PROPERTY_NAME>MEMBER_VALUE</PROPERTY_NAME>\
                <PROPERTY_CAPTION>MEMBER_VALUE</PROPERTY_CAPTION>\
                <DATA_TYPE>130</DATA_TYPE>\
                <PROPERTY_ORIGIN>6</PROPERTY_ORIGIN>\
                <PROPERTY_IS_VISIBLE>true</PROPERTY_IS_VISIBLE>\
                </row>"
            ));
            let mut sorted_tables: Vec<&TableMeta> =
                tables.iter().filter(|t| !t.is_hidden).collect();
            sorted_tables.sort_by(|a, b| a.name.cmp(&b.name));
            for table in sorted_tables {
                let mut sorted_cols: Vec<&ColumnMeta> =
                    table.columns.iter().filter(|c| !c.is_hidden).collect();
                sorted_cols.sort_by(|a, b| a.name.cmp(&b.name));
                for col in sorted_cols {
                    let dim = format!("[{}]", table.name);
                    let hier = format!("[{}].[{}]", table.name, col.name);
                    let member_dt = xsd_to_level_dbtype(&col.data_type);
                    out.push_str(&format!(
                        "<row>\
                        <CATALOG_NAME>{catalog}</CATALOG_NAME>\
                        <CUBE_NAME>Model</CUBE_NAME>\
                        <DIMENSION_UNIQUE_NAME>{dim}</DIMENSION_UNIQUE_NAME>\
                        <HIERARCHY_UNIQUE_NAME>{hier}</HIERARCHY_UNIQUE_NAME>\
                        <LEVEL_UNIQUE_NAME>{hier}.[(All)]</LEVEL_UNIQUE_NAME>\
                        <PROPERTY_TYPE>5</PROPERTY_TYPE>\
                        <PROPERTY_NAME>MEMBER_VALUE</PROPERTY_NAME>\
                        <PROPERTY_CAPTION>MEMBER_VALUE</PROPERTY_CAPTION>\
                        <DATA_TYPE>130</DATA_TYPE>\
                        <PROPERTY_ORIGIN>2</PROPERTY_ORIGIN>\
                        <PROPERTY_IS_VISIBLE>true</PROPERTY_IS_VISIBLE>\
                        </row>"
                    ));
                    out.push_str(&format!(
                        "<row>\
                        <CATALOG_NAME>{catalog}</CATALOG_NAME>\
                        <CUBE_NAME>Model</CUBE_NAME>\
                        <DIMENSION_UNIQUE_NAME>{dim}</DIMENSION_UNIQUE_NAME>\
                        <HIERARCHY_UNIQUE_NAME>{hier}</HIERARCHY_UNIQUE_NAME>\
                        <LEVEL_UNIQUE_NAME>{hier}.[{col_name}]</LEVEL_UNIQUE_NAME>\
                        <PROPERTY_TYPE>5</PROPERTY_TYPE>\
                        <PROPERTY_NAME>MEMBER_VALUE</PROPERTY_NAME>\
                        <PROPERTY_CAPTION>MEMBER_VALUE</PROPERTY_CAPTION>\
                        <DATA_TYPE>{member_dt}</DATA_TYPE>\
                        <PROPERTY_ORIGIN>2</PROPERTY_ORIGIN>\
                        <PROPERTY_IS_VISIBLE>true</PROPERTY_IS_VISIBLE>\
                        </row>",
                        col_name = col.name
                    ));
                }
            }
            out
        }
        _ => String::new(),
    };

    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn execute_query_result(
    session_id: Option<&str>,
    catalog: &str,
    results: Vec<QueryResult>,
    elapsed_ms: Option<u64>,
) -> (String, Response) {
    let mut roots: Vec<String> = Vec::with_capacity(results.len());
    let mut total_rows = 0usize;
    for result in &results {
        let schema = make_dax_schema(
            &result
                .columns
                .iter()
                .map(|(name, xsd)| (name.as_str(), xsd.as_str()))
                .collect::<Vec<_>>(),
        );

        let mut rows_xml = String::new();
        for row in &result.rows {
            rows_xml.push_str("<row>");
            for (col_idx, value) in row.iter().enumerate() {
                let tag = format!("C{col_idx}");
                if let Some(v) = value {
                    rows_xml.push_str(&format!("<{tag}>{val}</{tag}>", val = xml_escape_value(v),));
                }
            }
            rows_xml.push_str("</row>");
        }
        total_rows += result.rows.len();
        roots.push(rowset(&schema, &rows_xml));
    }

    tracing::info!(
        catalog,
        rows = total_rows,
        resultsets = results.len(),
        "DAX query result"
    );

    let rowset_body = if roots.len() == 1 {
        roots.remove(0)
    } else {
        format!(
            r#"<xmla-m:results xmlns:xmla-m="http://schemas.microsoft.com/analysisservices/2003/xmla-multipleresults">{}</xmla-m:results>"#,
            roots.join("")
        )
    };

    let metrics_xml = match elapsed_ms {
        Some(ms) => format!(
            r#"<ExecutionMetrics xmlns="http://schemas.microsoft.com/analysisservices/2003/engine"><TotalElapsedTimeMilliseconds>{ms}</TotalElapsedTimeMilliseconds><RowsReturned>{total_rows}</RowsReturned></ExecutionMetrics>"#,
        ),
        None => String::new(),
    };

    execute_xml(session_id, format!("{rowset_body}{metrics_xml}"))
}

fn xml_escape_value(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn xml_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn to_edm_name(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() || out.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        out.insert(0, '_');
    }
    out
}

fn dimensions_content(
    catalog: &str,
    tables: &[TableMeta],
    measure_count: usize,
) -> (String, String) {
    let schema = make_schema(DIMENSIONS_SCHEMA);
    let cat = xml_escape_value(catalog);
    let mut rows = format!(
        "<row>\
        <CATALOG_NAME>{cat}</CATALOG_NAME>\
        <SCHEMA_NAME/>\
        <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
        <DIMENSION_NAME>Measures</DIMENSION_NAME>\
        <DIMENSION_UNIQUE_NAME>[Measures]</DIMENSION_UNIQUE_NAME>\
        <DIMENSION_CAPTION>Measures</DIMENSION_CAPTION>\
        <DIMENSION_ORDINAL>1</DIMENSION_ORDINAL>\
        <DIMENSION_TYPE>2</DIMENSION_TYPE>\
        <DIMENSION_CARDINALITY>{measure_count}</DIMENSION_CARDINALITY>\
        <DEFAULT_HIERARCHY>[Measures]</DEFAULT_HIERARCHY>\
        <IS_VIRTUAL>false</IS_VIRTUAL>\
        <IS_READWRITE>false</IS_READWRITE>\
        <DIMENSION_UNIQUE_SETTINGS>1</DIMENSION_UNIQUE_SETTINGS>\
        <DIMENSION_MASTER_NAME>Measures</DIMENSION_MASTER_NAME>\
        <DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>\
        </row>",
        measure_count = measure_count,
    );
    for (ordinal, t) in tables.iter().enumerate() {
        let name = xml_escape_value(&t.name);
        let default_hierarchy = t
            .columns
            .first()
            .map(|c| format!("[{}].[{}]", t.name, c.name))
            .unwrap_or_default();
        let default_hierarchy = xml_escape_value(&default_hierarchy);
        let vis = !t.is_hidden;
        let ord = ordinal + 2;
        rows.push_str(&format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <SCHEMA_NAME/>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <DIMENSION_NAME>{name}</DIMENSION_NAME>\
            <DIMENSION_UNIQUE_NAME>[{name}]</DIMENSION_UNIQUE_NAME>\
            <DIMENSION_CAPTION>{name}</DIMENSION_CAPTION>\
            <DIMENSION_ORDINAL>{ord}</DIMENSION_ORDINAL>\
            <DIMENSION_TYPE>3</DIMENSION_TYPE>\
            <DIMENSION_CARDINALITY>0</DIMENSION_CARDINALITY>\
            <DEFAULT_HIERARCHY>{default_hierarchy}</DEFAULT_HIERARCHY>\
            <DESCRIPTION/>\
            <IS_VIRTUAL>false</IS_VIRTUAL>\
            <IS_READWRITE>false</IS_READWRITE>\
            <DIMENSION_UNIQUE_SETTINGS>1</DIMENSION_UNIQUE_SETTINGS>\
            <DIMENSION_MASTER_NAME>{name}</DIMENSION_MASTER_NAME>\
            <DIMENSION_IS_VISIBLE>{vis}</DIMENSION_IS_VISIBLE>\
            </row>"
        ));
    }
    (schema, rows)
}

pub fn discover_dimensions(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    measures: &[MeasureMeta],
) -> (String, Response) {
    let visible = measures.iter().filter(|m| !m.is_hidden).count();
    let (schema, rows) = dimensions_content(catalog, tables, visible);
    ok_xml(session_id, rowset(&schema, &rows))
}

fn hierarchies_content(catalog: &str, tables: &[TableMeta]) -> (String, String) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("DIMENSION_UNIQUE_NAME", "string"),
        ("HIERARCHY_NAME", "string"),
        ("HIERARCHY_UNIQUE_NAME", "string"),
        ("HIERARCHY_GUID", "uuid"),
        ("HIERARCHY_CAPTION", "string"),
        ("DIMENSION_TYPE", "short"),
        ("HIERARCHY_CARDINALITY", "unsignedInt"),
        ("DEFAULT_MEMBER", "string"),
        ("ALL_MEMBER", "string"),
        ("DESCRIPTION", "string"),
        ("STRUCTURE", "short"),
        ("IS_VIRTUAL", "boolean"),
        ("IS_READWRITE", "boolean"),
        ("DIMENSION_UNIQUE_SETTINGS", "int"),
        ("DIMENSION_MASTER_UNIQUE_NAME", "string"),
        ("DIMENSION_IS_VISIBLE", "boolean"),
        ("HIERARCHY_ORDINAL", "unsignedInt"),
        ("DIMENSION_IS_SHARED", "boolean"),
        ("HIERARCHY_IS_VISIBLE", "boolean"),
        ("HIERARCHY_ORIGIN", "unsignedShort"),
        ("HIERARCHY_DISPLAY_FOLDER", "string"),
        ("INSTANCE_SELECTION", "unsignedShort"),
        ("GROUPING_BEHAVIOR", "unsignedShort"),
        ("STRUCTURE_TYPE", "string"),
    ]);
    let cat = xml_escape_value(catalog);
    let mut rows = String::new();

    rows.push_str(&format!(
        "<row>\
        <CATALOG_NAME>{cat}</CATALOG_NAME>\
        <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
        <DIMENSION_UNIQUE_NAME>[Measures]</DIMENSION_UNIQUE_NAME>\
        <HIERARCHY_NAME>Measures</HIERARCHY_NAME>\
        <HIERARCHY_UNIQUE_NAME>[Measures]</HIERARCHY_UNIQUE_NAME>\
        <HIERARCHY_CAPTION>Measures</HIERARCHY_CAPTION>\
        <DIMENSION_TYPE>2</DIMENSION_TYPE>\
        <HIERARCHY_CARDINALITY>0</HIERARCHY_CARDINALITY>\
        <DEFAULT_MEMBER>[Measures].[__Default measure]</DEFAULT_MEMBER>\
        <DESCRIPTION/>\
        <STRUCTURE>0</STRUCTURE>\
        <IS_VIRTUAL>false</IS_VIRTUAL>\
        <IS_READWRITE>false</IS_READWRITE>\
        <DIMENSION_UNIQUE_SETTINGS>1</DIMENSION_UNIQUE_SETTINGS>\
        <DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>\
        <HIERARCHY_ORDINAL>1</HIERARCHY_ORDINAL>\
        <DIMENSION_IS_SHARED>true</DIMENSION_IS_SHARED>\
        <HIERARCHY_IS_VISIBLE>true</HIERARCHY_IS_VISIBLE>\
        <HIERARCHY_ORIGIN>6</HIERARCHY_ORIGIN>\
        <HIERARCHY_DISPLAY_FOLDER/>\
        <INSTANCE_SELECTION>0</INSTANCE_SELECTION>\
        <GROUPING_BEHAVIOR>2</GROUPING_BEHAVIOR>\
        <STRUCTURE_TYPE>Natural</STRUCTURE_TYPE>\
        </row>"
    ));

    let mut ordinal_map: std::collections::HashMap<(&str, &str), u32> =
        std::collections::HashMap::new();
    let mut next_ord: u32 = 2;
    for table in tables {
        if table.is_hidden {
            continue;
        }
        for col in &table.columns {
            ordinal_map.insert((&table.name, &col.name), next_ord);
            next_ord += 1;
        }
    }

    let mut entries: Vec<(&TableMeta, &ColumnMeta, u32)> = Vec::new();
    for table in tables {
        if table.is_hidden {
            continue;
        }
        for col in &table.columns {
            if col.is_hidden {
                continue;
            }
            let ord = ordinal_map[&(table.name.as_str(), col.name.as_str())];
            entries.push((table, col, ord));
        }
    }

    entries.sort_by(|a, b| a.0.name.cmp(&b.0.name).then(a.1.name.cmp(&b.1.name)));

    for (table, col, ordinal) in entries {
        let tname = xml_escape_value(&table.name);
        let cname = xml_escape_value(&col.name);
        let folder = col
            .display_folder
            .as_deref()
            .map(xml_escape_value)
            .unwrap_or_default();
        let folder_elem = if folder.is_empty() {
            String::from("<HIERARCHY_DISPLAY_FOLDER/>")
        } else {
            format!("<HIERARCHY_DISPLAY_FOLDER>{folder}</HIERARCHY_DISPLAY_FOLDER>")
        };
        rows.push_str(&format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <DIMENSION_UNIQUE_NAME>[{tname}]</DIMENSION_UNIQUE_NAME>\
            <HIERARCHY_NAME>{cname}</HIERARCHY_NAME>\
            <HIERARCHY_UNIQUE_NAME>[{tname}].[{cname}]</HIERARCHY_UNIQUE_NAME>\
            <HIERARCHY_CAPTION>{cname}</HIERARCHY_CAPTION>\
            <DIMENSION_TYPE>3</DIMENSION_TYPE>\
            <HIERARCHY_CARDINALITY>0</HIERARCHY_CARDINALITY>\
            <DEFAULT_MEMBER>[{tname}].[{cname}].[All]</DEFAULT_MEMBER>\
            <ALL_MEMBER>[{tname}].[{cname}].[All]</ALL_MEMBER>\
            <DESCRIPTION/>\
            <STRUCTURE>0</STRUCTURE>\
            <IS_VIRTUAL>false</IS_VIRTUAL>\
            <IS_READWRITE>false</IS_READWRITE>\
            <DIMENSION_UNIQUE_SETTINGS>1</DIMENSION_UNIQUE_SETTINGS>\
            <DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>\
            <HIERARCHY_ORDINAL>{ordinal}</HIERARCHY_ORDINAL>\
            <DIMENSION_IS_SHARED>true</DIMENSION_IS_SHARED>\
            <HIERARCHY_IS_VISIBLE>true</HIERARCHY_IS_VISIBLE>\
            <HIERARCHY_ORIGIN>2</HIERARCHY_ORIGIN>\
            {folder_elem}\
            <INSTANCE_SELECTION>0</INSTANCE_SELECTION>\
            <GROUPING_BEHAVIOR>1</GROUPING_BEHAVIOR>\
            <STRUCTURE_TYPE>Natural</STRUCTURE_TYPE>\
            </row>"
        ));
    }
    (schema, rows)
}

pub fn discover_hierarchies(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
) -> (String, Response) {
    let (schema, rows) = hierarchies_content(catalog, tables);
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn dmv_hierarchies(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
) -> (String, Response) {
    let (schema, rows) = hierarchies_content(catalog, tables);
    execute_xml(session_id, rowset(&schema, &rows))
}

fn xsd_to_level_dbtype(xsd: &str) -> u32 {
    match xsd {
        "integer" | "unsignedLong" => 20,
        "double" => 5,
        "boolean" => 11,
        "dateTime" => 135,
        _ => 130,
    }
}

fn levels_content(catalog: &str, tables: &[TableMeta]) -> (String, String) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("DIMENSION_UNIQUE_NAME", "string"),
        ("HIERARCHY_UNIQUE_NAME", "string"),
        ("LEVEL_NAME", "string"),
        ("LEVEL_UNIQUE_NAME", "string"),
        ("LEVEL_GUID", "uuid"),
        ("LEVEL_CAPTION", "string"),
        ("LEVEL_NUMBER", "unsignedInt"),
        ("LEVEL_CARDINALITY", "unsignedInt"),
        ("LEVEL_TYPE", "int"),
        ("DESCRIPTION", "string"),
        ("CUSTOM_ROLLUP_SETTINGS", "int"),
        ("LEVEL_UNIQUE_SETTINGS", "int"),
        ("LEVEL_IS_VISIBLE", "boolean"),
        ("LEVEL_ORDERING_PROPERTY", "string"),
        ("LEVEL_DBTYPE", "int"),
        ("LEVEL_MASTER_UNIQUE_NAME", "string"),
        ("LEVEL_NAME_SQL_COLUMN_NAME", "string"),
        ("LEVEL_KEY_SQL_COLUMN_NAME", "string"),
        ("LEVEL_UNIQUE_NAME_SQL_COLUMN_NAME", "string"),
        ("LEVEL_ATTRIBUTE_HIERARCHY_NAME", "string"),
        ("LEVEL_KEY_CARDINALITY", "unsignedShort"),
        ("LEVEL_ORIGIN", "unsignedShort"),
    ]);
    let cat = xml_escape_value(catalog);
    let mut rows = String::new();

    // [Measures] level — no DESCRIPTION, no LEVEL_ORDERING_PROPERTY, no SQL column names
    rows.push_str(&format!(
        "<row>\
        <CATALOG_NAME>{cat}</CATALOG_NAME>\
        <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
        <DIMENSION_UNIQUE_NAME>[Measures]</DIMENSION_UNIQUE_NAME>\
        <HIERARCHY_UNIQUE_NAME>[Measures]</HIERARCHY_UNIQUE_NAME>\
        <LEVEL_NAME>MeasuresLevel</LEVEL_NAME>\
        <LEVEL_UNIQUE_NAME>[Measures].[MeasuresLevel]</LEVEL_UNIQUE_NAME>\
        <LEVEL_CAPTION>MeasuresLevel</LEVEL_CAPTION>\
        <LEVEL_NUMBER>0</LEVEL_NUMBER>\
        <LEVEL_CARDINALITY>0</LEVEL_CARDINALITY>\
        <LEVEL_TYPE>0</LEVEL_TYPE>\
        <CUSTOM_ROLLUP_SETTINGS>0</CUSTOM_ROLLUP_SETTINGS>\
        <LEVEL_UNIQUE_SETTINGS>0</LEVEL_UNIQUE_SETTINGS>\
        <LEVEL_IS_VISIBLE>true</LEVEL_IS_VISIBLE>\
        <LEVEL_DBTYPE>130</LEVEL_DBTYPE>\
        <LEVEL_ATTRIBUTE_HIERARCHY_NAME>Measures</LEVEL_ATTRIBUTE_HIERARCHY_NAME>\
        <LEVEL_KEY_CARDINALITY>1</LEVEL_KEY_CARDINALITY>\
        <LEVEL_ORIGIN>6</LEVEL_ORIGIN>\
        </row>"
    ));

    let mut entries: Vec<(&TableMeta, &ColumnMeta)> = tables
        .iter()
        .filter(|t| !t.is_hidden)
        .flat_map(|t| {
            t.columns
                .iter()
                .filter(|c| !c.is_hidden)
                .map(move |c| (t, c))
        })
        .collect();

    entries.sort_by(|a, b| a.0.name.cmp(&b.0.name).then(a.1.name.cmp(&b.1.name)));

    for (table, col) in entries {
        let tname = xml_escape_value(&table.name);
        let cname = xml_escape_value(&col.name);
        let dbtype = xsd_to_level_dbtype(&col.data_type);

        rows.push_str(&format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <DIMENSION_UNIQUE_NAME>[{tname}]</DIMENSION_UNIQUE_NAME>\
            <HIERARCHY_UNIQUE_NAME>[{tname}].[{cname}]</HIERARCHY_UNIQUE_NAME>\
            <LEVEL_NAME>(All)</LEVEL_NAME>\
            <LEVEL_UNIQUE_NAME>[{tname}].[{cname}].[(All)]</LEVEL_UNIQUE_NAME>\
            <LEVEL_CAPTION>(All)</LEVEL_CAPTION>\
            <LEVEL_NUMBER>0</LEVEL_NUMBER>\
            <LEVEL_CARDINALITY>0</LEVEL_CARDINALITY>\
            <LEVEL_TYPE>1</LEVEL_TYPE>\
            <CUSTOM_ROLLUP_SETTINGS>0</CUSTOM_ROLLUP_SETTINGS>\
            <LEVEL_UNIQUE_SETTINGS>0</LEVEL_UNIQUE_SETTINGS>\
            <LEVEL_IS_VISIBLE>true</LEVEL_IS_VISIBLE>\
            <LEVEL_ORDERING_PROPERTY>(All)</LEVEL_ORDERING_PROPERTY>\
            <LEVEL_DBTYPE>3</LEVEL_DBTYPE>\
            <LEVEL_KEY_CARDINALITY>1</LEVEL_KEY_CARDINALITY>\
            <LEVEL_ORIGIN>2</LEVEL_ORIGIN>\
            </row>"
        ));

        rows.push_str(&format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <DIMENSION_UNIQUE_NAME>[{tname}]</DIMENSION_UNIQUE_NAME>\
            <HIERARCHY_UNIQUE_NAME>[{tname}].[{cname}]</HIERARCHY_UNIQUE_NAME>\
            <LEVEL_NAME>{cname}</LEVEL_NAME>\
            <LEVEL_UNIQUE_NAME>[{tname}].[{cname}].[{cname}]</LEVEL_UNIQUE_NAME>\
            <LEVEL_CAPTION>{cname}</LEVEL_CAPTION>\
            <LEVEL_NUMBER>1</LEVEL_NUMBER>\
            <LEVEL_CARDINALITY>0</LEVEL_CARDINALITY>\
            <LEVEL_TYPE>0</LEVEL_TYPE>\
            <DESCRIPTION/>\
            <CUSTOM_ROLLUP_SETTINGS>0</CUSTOM_ROLLUP_SETTINGS>\
            <LEVEL_UNIQUE_SETTINGS>0</LEVEL_UNIQUE_SETTINGS>\
            <LEVEL_IS_VISIBLE>true</LEVEL_IS_VISIBLE>\
            <LEVEL_ORDERING_PROPERTY>{cname}</LEVEL_ORDERING_PROPERTY>\
            <LEVEL_DBTYPE>{dbtype}</LEVEL_DBTYPE>\
            <LEVEL_NAME_SQL_COLUMN_NAME>NAME( [${tname}].[{cname}] )</LEVEL_NAME_SQL_COLUMN_NAME>\
            <LEVEL_KEY_SQL_COLUMN_NAME>KEY( [${tname}].[{cname}] )</LEVEL_KEY_SQL_COLUMN_NAME>\
            <LEVEL_UNIQUE_NAME_SQL_COLUMN_NAME>UNIQUENAME( [${tname}].[{cname}] )</LEVEL_UNIQUE_NAME_SQL_COLUMN_NAME>\
            <LEVEL_ATTRIBUTE_HIERARCHY_NAME>{cname}</LEVEL_ATTRIBUTE_HIERARCHY_NAME>\
            <LEVEL_KEY_CARDINALITY>1</LEVEL_KEY_CARDINALITY>\
            <LEVEL_ORIGIN>2</LEVEL_ORIGIN>\
            </row>"
        ));
    }
    (schema, rows)
}

pub fn discover_levels(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
) -> (String, Response) {
    let (schema, rows) = levels_content(catalog, tables);
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn dmv_levels(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
) -> (String, Response) {
    let (schema, rows) = levels_content(catalog, tables);
    execute_xml(session_id, rowset(&schema, &rows))
}

fn parse_member_unique_name(uname: &str) -> Option<Vec<String>> {
    let stripped = uname.trim().strip_prefix('[')?.strip_suffix(']')?;
    let parts: Vec<String> = stripped.split("].[").map(|s| s.to_string()).collect();
    if parts.len() >= 2 {
        Some(parts)
    } else {
        None
    }
}

pub fn discover_members(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    member_uname: &str,
    tree_op: u32,
) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("DIMENSION_UNIQUE_NAME", "string"),
        ("HIERARCHY_UNIQUE_NAME", "string"),
        ("LEVEL_UNIQUE_NAME", "string"),
        ("LEVEL_NUMBER", "unsignedInt"),
        ("MEMBER_ORDINAL", "unsignedInt"),
        ("MEMBER_NAME", "string"),
        ("MEMBER_UNIQUE_NAME", "string"),
        ("MEMBER_TYPE", "int"),
        ("MEMBER_GUID", "uuid"),
        ("MEMBER_CAPTION", "string"),
        ("CHILDREN_CARDINALITY", "unsignedInt"),
        ("PARENT_LEVEL", "unsignedInt"),
        ("PARENT_UNIQUE_NAME", "string"),
        ("PARENT_COUNT", "unsignedInt"),
        ("DESCRIPTION", "string"),
        ("EXPRESSION", "string"),
        ("MEMBER_KEY", "string"),
        ("IS_PLACEHOLDERMEMBER", "boolean"),
        ("IS_DATAMEMBER", "boolean"),
        ("SCOPE", "int"),
    ]);

    // Only handle TREE_OP=8 (SELF) for now.
    if tree_op != 8 {
        return ok_xml(session_id, rowset(&schema, ""));
    }

    let parts = match parse_member_unique_name(member_uname) {
        Some(p) if p.len() >= 3 => p,
        _ => return ok_xml(session_id, rowset(&schema, "")),
    };

    let table_name = &parts[0];
    let col_name = &parts[1];
    let member_name = &parts[2];

    let table = match tables
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(table_name))
    {
        Some(t) => t,
        None => return ok_xml(session_id, rowset(&schema, "")),
    };
    if !table
        .columns
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(col_name))
    {
        return ok_xml(session_id, rowset(&schema, ""));
    }

    let cat = xml_escape_value(catalog);
    let tname = xml_escape_value(&table.name);
    let cname = xml_escape_value(col_name);

    let rows = if member_name.eq_ignore_ascii_case("All") {
        format!(
            "<row>\
            <CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <DIMENSION_UNIQUE_NAME>[{tname}]</DIMENSION_UNIQUE_NAME>\
            <HIERARCHY_UNIQUE_NAME>[{tname}].[{cname}]</HIERARCHY_UNIQUE_NAME>\
            <LEVEL_UNIQUE_NAME>[{tname}].[{cname}].[(All)]</LEVEL_UNIQUE_NAME>\
            <LEVEL_NUMBER>0</LEVEL_NUMBER>\
            <MEMBER_ORDINAL>0</MEMBER_ORDINAL>\
            <MEMBER_NAME>All</MEMBER_NAME>\
            <MEMBER_UNIQUE_NAME>[{tname}].[{cname}].[All]</MEMBER_UNIQUE_NAME>\
            <MEMBER_TYPE>2</MEMBER_TYPE>\
            <MEMBER_CAPTION>All</MEMBER_CAPTION>\
            <CHILDREN_CARDINALITY>1</CHILDREN_CARDINALITY>\
            <PARENT_COUNT>0</PARENT_COUNT>\
            <MEMBER_KEY>0</MEMBER_KEY>\
            <IS_PLACEHOLDERMEMBER>false</IS_PLACEHOLDERMEMBER>\
            <IS_DATAMEMBER>false</IS_DATAMEMBER>\
            </row>"
        )
    } else {
        String::new()
    };

    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn dmv_kpis(session_id: Option<&str>) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("KPI_NAME", "string"),
        ("KPI_CAPTION", "string"),
        ("MEASUREGROUP_NAME", "string"),
        ("KPI_DISPLAY_FOLDER", "string"),
        ("KPI_GOAL", "string"),
        ("KPI_STATUS", "string"),
        ("KPI_TREND", "string"),
        ("KPI_VALUE", "string"),
    ]);
    execute_xml(session_id, rowset(&schema, ""))
}

pub fn discover_mdschema_kpis(session_id: Option<&str>) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("MEASUREGROUP_NAME", "string"),
        ("KPI_NAME", "string"),
        ("KPI_CAPTION", "string"),
        ("KPI_DESCRIPTION", "string"),
        ("KPI_DISPLAY_FOLDER", "string"),
        ("KPI_VALUE", "string"),
        ("KPI_GOAL", "string"),
        ("KPI_STATUS", "string"),
        ("KPI_TREND", "string"),
        ("KPI_STATUS_GRAPHIC", "string"),
        ("KPI_TREND_GRAPHIC", "string"),
        ("KPI_WEIGHT", "string"),
        ("KPI_CURRENT_TIME_MEMBER", "string"),
        ("KPI_PARENT_KPI_NAME", "string"),
        ("ANNOTATIONS", "string"),
        ("SCOPE", "int"),
    ]);
    ok_xml(session_id, rowset(&schema, ""))
}

pub fn discover_mdschema_measuregroups(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
) -> (String, Response) {
    let schema = make_schema(&[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("MEASUREGROUP_NAME", "string"),
        ("DESCRIPTION", "string"),
        ("IS_WRITE_ENABLED", "boolean"),
        ("MEASUREGROUP_CAPTION", "string"),
    ]);
    let cat = xml_escape_value(catalog);
    let mut visible: Vec<&TableMeta> = tables.iter().filter(|t| !t.is_hidden).collect();
    visible.sort_by(|a, b| a.name.cmp(&b.name));
    let rows: String = visible
        .iter()
        .map(|t| {
            let tname = xml_escape_value(&t.name);
            format!(
                "<row><CATALOG_NAME>{cat}</CATALOG_NAME>\
                <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
                <MEASUREGROUP_NAME>{tname}</MEASUREGROUP_NAME>\
                <DESCRIPTION/>\
                <IS_WRITE_ENABLED>false</IS_WRITE_ENABLED>\
                <MEASUREGROUP_CAPTION>{tname}</MEASUREGROUP_CAPTION></row>"
            )
        })
        .collect();
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn discover_mdschema_measuregroup_dimensions(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    relationships: &[RelationshipMeta],
) -> (String, Response) {
    let mut cols = String::new();
    for (name, typ) in &[
        ("CATALOG_NAME", "string"),
        ("SCHEMA_NAME", "string"),
        ("CUBE_NAME", "string"),
        ("MEASUREGROUP_NAME", "string"),
        ("MEASUREGROUP_CARDINALITY", "string"),
        ("DIMENSION_UNIQUE_NAME", "string"),
        ("DIMENSION_CARDINALITY", "string"),
        ("DIMENSION_IS_VISIBLE", "boolean"),
        ("DIMENSION_IS_FACT_DIMENSION", "boolean"),
    ] {
        cols.push_str(&format!(
            r#"<xsd:element sql:field="{name}" name="{name}" type="xsd:{typ}" minOccurs="0"/>"#
        ));
    }
    cols.push_str(concat!(
        r#"<xsd:element sql:field="DIMENSION_PATH" name="DIMENSION_PATH" minOccurs="0" maxOccurs="unbounded">"#,
        r#"<xsd:complexType><xsd:sequence>"#,
        r#"<xsd:element sql:field="MeasureGroupDimension" name="MeasureGroupDimension" type="xsd:string" minOccurs="0"/>"#,
        r#"</xsd:sequence></xsd:complexType></xsd:element>"#,
        r#"<xsd:element sql:field="DIMENSION_GRANULARITY" name="DIMENSION_GRANULARITY" type="xsd:string" minOccurs="0"/>"#,
    ));
    let schema = format!(
        concat!(
            r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql""#,
            r#" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified">"#,
            r#"<xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded">"#,
            r#"<xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/>"#,
            r#"</xsd:sequence></xsd:complexType></xsd:element>"#,
            r#"<xsd:complexType name="row"><xsd:sequence>{cols}</xsd:sequence></xsd:complexType>"#,
            r#"</xsd:schema>"#,
        ),
        cols = cols,
    );

    let cat = xml_escape_value(catalog);

    struct MgDimRow {
        measuregroup: String,
        is_fact: bool,
        xml: String,
    }
    let mut all_rows: Vec<MgDimRow> = Vec::new();

    for table in tables {
        if table.is_hidden {
            continue;
        }
        let tname = xml_escape_value(&table.name);
        let granularity_col = table
            .columns
            .iter()
            .find(|c| c.is_key)
            .or_else(|| table.columns.first())
            .map(|c| xml_escape_value(&c.name))
            .unwrap_or_else(|| tname.clone());
        let xml = format!(
            "<row><CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <MEASUREGROUP_NAME>{tname}</MEASUREGROUP_NAME>\
            <MEASUREGROUP_CARDINALITY>ONE</MEASUREGROUP_CARDINALITY>\
            <DIMENSION_UNIQUE_NAME>[{tname}]</DIMENSION_UNIQUE_NAME>\
            <DIMENSION_CARDINALITY>ONE</DIMENSION_CARDINALITY>\
            <DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>\
            <DIMENSION_IS_FACT_DIMENSION>true</DIMENSION_IS_FACT_DIMENSION>\
            <DIMENSION_GRANULARITY>[{tname}].[{granularity_col}]</DIMENSION_GRANULARITY></row>"
        );
        all_rows.push(MgDimRow { measuregroup: table.name.clone(), is_fact: true, xml });
    }

    // Relationship rows.
    // Per the engine's own relationship convention (see ExecutionContext::
    // expanded_filter_context): fromTable is the "many"/fact side, toTable is
    // the "one"/dimension side.
    // MEASUREGROUP_NAME  = from_table (the fact/measure-group table)
    // DIMENSION_UNIQUE_NAME = [to_table] (the dimension table)
    // DIMENSION_PATH: dimension first, then fact
    // DIMENSION_GRANULARITY = [to_table].[to_column] (the join key on the dimension side)
    for rel in relationships {
        if !rel.is_active {
            continue;
        }
        let from_visible = tables
            .iter()
            .any(|t| t.name == rel.from_table && !t.is_hidden);
        let to_visible = tables
            .iter()
            .any(|t| t.name == rel.to_table && !t.is_hidden);
        if !from_visible || !to_visible {
            continue;
        }
        // Per the engine's own relationship convention (see
        // ExecutionContext::expanded_filter_context): fromTable is the
        // "many"/fact side, toTable is the "one"/dimension side.
        let fact_tname = xml_escape_value(&rel.from_table);
        let dim_tname = xml_escape_value(&rel.to_table);
        let dim_col = xml_escape_value(&rel.to_column);
        let xml = format!(
            "<row><CATALOG_NAME>{cat}</CATALOG_NAME>\
            <CUBE_NAME>{CUBE_NAME}</CUBE_NAME>\
            <MEASUREGROUP_NAME>{fact_tname}</MEASUREGROUP_NAME>\
            <MEASUREGROUP_CARDINALITY>MANY</MEASUREGROUP_CARDINALITY>\
            <DIMENSION_UNIQUE_NAME>[{dim_tname}]</DIMENSION_UNIQUE_NAME>\
            <DIMENSION_CARDINALITY>ONE</DIMENSION_CARDINALITY>\
            <DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>\
            <DIMENSION_IS_FACT_DIMENSION>false</DIMENSION_IS_FACT_DIMENSION>\
            <DIMENSION_PATH><MeasureGroupDimension>{dim_tname}</MeasureGroupDimension></DIMENSION_PATH>\
            <DIMENSION_PATH><MeasureGroupDimension>{fact_tname}</MeasureGroupDimension></DIMENSION_PATH>\
            <DIMENSION_GRANULARITY>[{dim_tname}].[{dim_col}]</DIMENSION_GRANULARITY></row>"
        );
        all_rows.push(MgDimRow { measuregroup: rel.from_table.clone(), is_fact: false, xml });
    }

    // Sort: alphabetical by measure group, then non-fact (relationship) before fact (self).
    all_rows.sort_by(|a, b| {
        a.measuregroup
            .cmp(&b.measuregroup)
            .then(a.is_fact.cmp(&b.is_fact))
    });

    let rows: String = all_rows.into_iter().map(|r| r.xml).collect();
    ok_xml(session_id, rowset(&schema, &rows))
}

pub struct DmvResult {
    pub schema: &'static [(&'static str, &'static str)],
    pub rows: Vec<Row>,
}

static CUBES_SCHEMA: &[(&str, &str)] = &[
    ("CUBE_NAME", "string"),
    ("BASE_CUBE_NAME", "string"),
    ("CUBE_CAPTION", "string"),
    ("LAST_SCHEMA_UPDATE", "dateTime"),
    ("LAST_DATA_UPDATE", "dateTime"),
    ("DESCRIPTION", "string"),
];

static CATALOGS_SCHEMA: &[(&str, &str)] = &[
    ("CATALOG_NAME", "string"),
    ("DESCRIPTION", "string"),
    ("ROLES", "string"),
    ("DATE_MODIFIED", "dateTime"),
    ("COMPATIBILITY_LEVEL", "int"),
    ("TYPE", "int"),
    ("VERSION", "long"),
    ("DATABASE_ID", "string"),
    ("DATABASE_GUID", "string"),
    ("DATE_QUERIED", "dateTime"),
    ("CURRENTLY_USED", "boolean"),
    ("POPULARITY", "float"),
    ("WEIGHTEDPOPULARITY", "double"),
    ("CLIENTCACHEREFRESHPOLICY", "unsignedInt"),
    ("ENCRYPTION_LEVEL", "string"),
    ("CRYPTOKEY_UPDATED", "dateTime"),
];

static MEASURES_FULL_SCHEMA: &[(&str, &str)] = &[
    ("CATALOG_NAME", "string"),
    ("SCHEMA_NAME", "string"),
    ("CUBE_NAME", "string"),
    ("MEASURE_NAME", "string"),
    ("MEASURE_UNIQUE_NAME", "string"),
    ("MEASURE_CAPTION", "string"),
    ("MEASURE_GUID", "string"),
    ("MEASURE_AGGREGATOR", "int"),
    ("DATA_TYPE", "unsignedShort"),
    ("NUMERIC_PRECISION", "unsignedShort"),
    ("NUMERIC_SCALE", "short"),
    ("MEASURE_UNITS", "string"),
    ("DESCRIPTION", "string"),
    ("EXPRESSION", "string"),
    ("MEASURE_IS_VISIBLE", "boolean"),
    ("LEVELS_LIST", "string"),
    ("MEASURE_NAME_SQL_COLUMN_NAME", "string"),
    ("MEASURE_UNQUALIFIED_CAPTION", "string"),
    ("MEASUREGROUP_NAME", "string"),
    ("MEASURE_DISPLAY_FOLDER", "string"),
    ("DEFAULT_FORMAT_STRING", "string"),
];

static MEASURES_SCHEMA: &[(&str, &str)] = MEASURES_FULL_SCHEMA;

static DIMENSIONS_SCHEMA: &[(&str, &str)] = &[
    ("CATALOG_NAME", "string"),
    ("SCHEMA_NAME", "string"),
    ("CUBE_NAME", "string"),
    ("DIMENSION_NAME", "string"),
    ("DIMENSION_UNIQUE_NAME", "string"),
    ("DIMENSION_GUID", "uuid"),
    ("DIMENSION_CAPTION", "string"),
    ("DIMENSION_ORDINAL", "unsignedInt"),
    ("DIMENSION_TYPE", "short"),
    ("DIMENSION_CARDINALITY", "unsignedInt"),
    ("DEFAULT_HIERARCHY", "string"),
    ("DESCRIPTION", "string"),
    ("IS_VIRTUAL", "boolean"),
    ("IS_READWRITE", "boolean"),
    ("DIMENSION_UNIQUE_SETTINGS", "int"),
    ("DIMENSION_MASTER_NAME", "string"),
    ("DIMENSION_IS_VISIBLE", "boolean"),
];

pub fn dmv_cubes_rows(databases: &[DatabaseMeta]) -> DmvResult {
    let rows = databases
        .iter()
        .map(|db| {
            vec![
                ("CUBE_NAME".into(), CUBE_NAME.to_string()),
                ("BASE_CUBE_NAME".into(), CUBE_NAME.to_string()),
                ("CUBE_CAPTION".into(), CUBE_NAME.to_string()),
                ("LAST_SCHEMA_UPDATE".into(), db.last_schema_update.clone()),
                ("LAST_DATA_UPDATE".into(), db.last_refreshed.clone()),
                ("DESCRIPTION".into(), String::new()),
            ]
        })
        .collect();
    DmvResult { schema: CUBES_SCHEMA, rows }
}

pub fn dmv_catalogs_rows(databases: &[DatabaseMeta]) -> DmvResult {
    let rows = databases
        .iter()
        .map(|db| {
            vec![
                ("CATALOG_NAME".into(), db.name.clone()),
                ("DESCRIPTION".into(), String::new()),
                (
                    "COMPATIBILITY_LEVEL".into(),
                    CATALOG_COMPAT_LEVEL.to_string(),
                ),
                ("DATABASE_ID".into(), db.id.clone()),
            ]
        })
        .collect();
    DmvResult { schema: CATALOGS_SCHEMA, rows }
}

pub fn dmv_measures_rows(catalog: &str, measures: &[MeasureMeta]) -> DmvResult {
    let rows = measures
        .iter()
        .map(|m| {
            vec![
                ("CATALOG_NAME".into(), catalog.to_string()),
                ("SCHEMA_NAME".into(), catalog.to_string()),
                ("CUBE_NAME".into(), CUBE_NAME.to_string()),
                ("MEASURE_NAME".into(), m.name.clone()),
                (
                    "MEASURE_UNIQUE_NAME".into(),
                    format!("[Measures].[{}]", m.name),
                ),
                ("MEASURE_CAPTION".into(), m.display_name.clone()),
                ("MEASURE_AGGREGATOR".into(), m.aggregator.to_string()),
                ("DATA_TYPE".into(), m.data_type.to_string()),
                ("NUMERIC_PRECISION".into(), "65535".into()),
                ("NUMERIC_SCALE".into(), "-1".into()),
                (
                    "DESCRIPTION".into(),
                    m.description.clone().unwrap_or_default(),
                ),
                ("EXPRESSION".into(), m.expression.clone()),
                ("MEASURE_IS_VISIBLE".into(), (!m.is_hidden).to_string()),
                ("MEASURE_NAME_SQL_COLUMN_NAME".into(), m.name.clone()),
                ("MEASURE_UNQUALIFIED_CAPTION".into(), m.display_name.clone()),
                ("MEASUREGROUP_NAME".into(), m.table_name.clone()),
                (
                    "MEASURE_DISPLAY_FOLDER".into(),
                    m.display_folder.clone().unwrap_or_default(),
                ),
                (
                    "DEFAULT_FORMAT_STRING".into(),
                    m.format_string.clone().unwrap_or_default(),
                ),
            ]
        })
        .collect();
    DmvResult { schema: MEASURES_SCHEMA, rows }
}

pub fn dmv_dimensions_rows(catalog: &str, tables: &[TableMeta]) -> DmvResult {
    let mut rows: Vec<Row> = vec![vec![
        ("CATALOG_NAME".into(), catalog.to_string()),
        ("SCHEMA_NAME".into(), String::new()),
        ("CUBE_NAME".into(), CUBE_NAME.to_string()),
        ("DIMENSION_NAME".into(), "Measures".to_string()),
        ("DIMENSION_UNIQUE_NAME".into(), "[Measures]".to_string()),
        ("DIMENSION_GUID".into(), String::new()),
        ("DIMENSION_CAPTION".into(), "Measures".to_string()),
        ("DIMENSION_ORDINAL".into(), "1".into()),
        ("DIMENSION_TYPE".into(), "2".into()),
        ("DIMENSION_CARDINALITY".into(), "0".into()),
        ("DEFAULT_HIERARCHY".into(), "[Measures]".to_string()),
        ("DESCRIPTION".into(), String::new()),
        ("IS_VIRTUAL".into(), "false".into()),
        ("IS_READWRITE".into(), "false".into()),
        ("DIMENSION_UNIQUE_SETTINGS".into(), "0".into()),
        ("DIMENSION_MASTER_NAME".into(), "Measures".to_string()),
        ("DIMENSION_IS_VISIBLE".into(), "true".into()),
    ]];
    for (i, t) in tables.iter().enumerate() {
        let default_hierarchy = t
            .columns
            .first()
            .map(|c| format!("[{}].[{}]", t.name, c.name))
            .unwrap_or_default();
        rows.push(vec![
            ("CATALOG_NAME".into(), catalog.to_string()),
            ("SCHEMA_NAME".into(), String::new()),
            ("CUBE_NAME".into(), CUBE_NAME.to_string()),
            ("DIMENSION_NAME".into(), t.name.clone()),
            ("DIMENSION_UNIQUE_NAME".into(), format!("[{}]", t.name)),
            ("DIMENSION_GUID".into(), String::new()),
            ("DIMENSION_CAPTION".into(), t.name.clone()),
            ("DIMENSION_ORDINAL".into(), (i + 2).to_string()),
            ("DIMENSION_TYPE".into(), "3".into()),
            ("DIMENSION_CARDINALITY".into(), "0".into()),
            ("DEFAULT_HIERARCHY".into(), default_hierarchy),
            (
                "DESCRIPTION".into(),
                t.description.clone().unwrap_or_default(),
            ),
            ("IS_VIRTUAL".into(), "false".into()),
            ("IS_READWRITE".into(), "false".into()),
            ("DIMENSION_UNIQUE_SETTINGS".into(), "1".into()),
            ("DIMENSION_MASTER_NAME".into(), t.name.clone()),
            ("DIMENSION_IS_VISIBLE".into(), (!t.is_hidden).to_string()),
        ]);
    }
    DmvResult { schema: DIMENSIONS_SCHEMA, rows }
}

pub fn render_dmv_result(session_id: Option<&str>, result: DmvResult) -> (String, Response) {
    let schema_xml = make_schema(result.schema);
    let mut xml_rows = String::new();
    for row in &result.rows {
        xml_rows.push_str("<row>");
        for (col, val) in row {
            if val.is_empty() {
                xml_rows.push_str(&format!("<{col}/>"));
            } else {
                xml_rows.push_str(&format!("<{col}>{v}</{col}>", v = xml_escape_value(val)));
            }
        }
        xml_rows.push_str("</row>");
    }
    execute_xml(session_id, rowset(&schema_xml, &xml_rows))
}

pub fn build_tom_xml(
    name: &str,
    tables: &[TableMeta],
    measures: &[MeasureMeta],
    relationships: &[RelationshipMeta],
    meta: &ModelMeta,
) -> String {
    let tables_xml = build_tom_tables_xml(tables);
    let measures_xml = build_tom_measures_xml(measures);
    let relationships_xml = build_tom_relationships_xml(relationships);
    format!(
        concat!(
            r#"<Database"#,
            r#" xmlns="http://schemas.microsoft.com/analysisservices/2003/engine""#,
            r#" xmlns:ddl2="http://schemas.microsoft.com/analysisservices/2003/engine/2""#,
            r#" xmlns:ddl2_2="http://schemas.microsoft.com/analysisservices/2003/engine/2/2""#,
            r#" xmlns:ddl100_100="http://schemas.microsoft.com/analysisservices/2008/engine/100/100""#,
            r#" xmlns:ddl400="http://schemas.microsoft.com/analysisservices/2012/engine/400""#,
            r#" xmlns:ddl401="http://schemas.microsoft.com/analysisservices/2012/engine/401""#,
            r#" xmlns:dwd="http://schemas.microsoft.com/DataWarehouse/Designer/1.0""#,
            r#">"#,
            r#"<ID>{name}</ID>"#,
            r#"<Name>{name}</Name>"#,
            r#"<CompatibilityLevel>{compat}</CompatibilityLevel>"#,
            r#"<StorageEngineUsed>{storage_engine}</StorageEngineUsed>"#,
            r#"<ddl400:Model>"#,
            r#"<ddl400:Name>{name}</ddl400:Name>"#,
            r#"<ddl400:DefaultMode>{default_mode}</ddl400:DefaultMode>"#,
            r#"<ddl400:Culture>{culture}</ddl400:Culture>"#,
            r#"<ddl400:Collation>{collation}</ddl400:Collation>"#,
            r#"<ddl400:Tables>{tables_xml}</ddl400:Tables>"#,
            r#"<ddl400:Relationships>{relationships_xml}</ddl400:Relationships>"#,
            r#"<ddl400:Roles/>"#,
            r#"<ddl400:Measures>{measures_xml}</ddl400:Measures>"#,
            r#"</ddl400:Model>"#,
            r#"<State>{state}</State>"#,
            r#"<ReadWriteMode>{read_write_mode}</ReadWriteMode>"#,
            r#"</Database>"#,
        ),
        name = xml_escape_value(name),
        compat = meta.compatibility_level,
        storage_engine = xml_escape_value(&meta.storage_engine_used),
        default_mode = xml_escape_value(&meta.default_mode),
        culture = xml_escape_value(&meta.culture),
        collation = xml_escape_value(&meta.collation),
        state = xml_escape_value(&meta.state),
        read_write_mode = xml_escape_value(&meta.read_write_mode),
        tables_xml = tables_xml,
        measures_xml = measures_xml,
        relationships_xml = relationships_xml,
    )
}

fn tom_opt_elem(elem: &str, text: Option<&str>) -> String {
    match text.filter(|s| !s.is_empty()) {
        Some(t) => format!("<ddl400:{elem}>{}</ddl400:{elem}>", xml_escape_value(t)),
        None => String::new(),
    }
}

fn build_tom_tables_xml(tables: &[TableMeta]) -> String {
    let mut xml = String::new();
    for table in tables {
        let mut cols_xml = String::new();
        for col in &table.columns {
            let format_string_xml = tom_opt_elem("FormatString", col.format_string.as_deref());
            let description_xml = tom_opt_elem("Description", col.description.as_deref());
            let display_folder_xml = tom_opt_elem("DisplayFolder", col.display_folder.as_deref());
            cols_xml.push_str(&format!(
                r#"<ddl400:Column><ddl400:Name>{col}</ddl400:Name><ddl400:DataType>{xsd}</ddl400:DataType>{format_string_xml}<ddl400:IsHidden>{hidden}</ddl400:IsHidden>{description_xml}{display_folder_xml}{cat_xml}</ddl400:Column>"#,
                col = xml_escape_value(&col.name),
                xsd = col.data_type,
                hidden = col.is_hidden,
                cat_xml = col.data_category.as_ref()
                    .map(|c| format!("<ddl400:DataCategory>{}</ddl400:DataCategory>", xml_escape_value(c.as_str())))
                    .unwrap_or_default(),
            ));
        }
        let cat_xml = table
            .data_category
            .as_ref()
            .map(|c| {
                format!(
                    "<ddl400:DataCategory>{}</ddl400:DataCategory>",
                    xml_escape_value(c.as_str())
                )
            })
            .unwrap_or_default();
        xml.push_str(&format!(
            r#"<ddl400:Table><ddl400:Name>{table}</ddl400:Name><ddl400:IsHidden>{hidden}</ddl400:IsHidden>{cat_xml}<ddl400:Columns>{cols_xml}</ddl400:Columns></ddl400:Table>"#,
            table = xml_escape_value(&table.name),
            hidden = table.is_hidden,
            cols_xml = cols_xml,
        ));
    }
    xml
}

fn build_tom_measures_xml(measures: &[MeasureMeta]) -> String {
    measures
        .iter()
        .map(|m| {
            let format_string_xml = tom_opt_elem("FormatString", m.format_string.as_deref());
            let description_xml = tom_opt_elem("Description", m.description.as_deref());
            let display_folder_xml = tom_opt_elem("DisplayFolder", m.display_folder.as_deref());
            format!(
                r#"<ddl400:Measure><ddl400:Name>{name}</ddl400:Name><ddl400:Expression>{expr}</ddl400:Expression>{format_string_xml}<ddl400:IsHidden>{hidden}</ddl400:IsHidden>{description_xml}{display_folder_xml}</ddl400:Measure>"#,
                name = xml_escape_value(&m.name),
                hidden = m.is_hidden,
                expr = xml_escape_value(&m.expression),
            )
        })
        .collect()
}

fn build_tom_relationships_xml(relationships: &[RelationshipMeta]) -> String {
    relationships
        .iter()
        .map(|r| {
            let cross_filter = if r.bidirectional { "BothDirections" } else { "OneDirection" };
            format!(
                concat!(
                    r#"<ddl400:Relationship>"#,
                    r#"<ddl400:Name>{name}</ddl400:Name>"#,
                    r#"<ddl400:FromTableID>{from_table}</ddl400:FromTableID>"#,
                    r#"<ddl400:FromColumnID>{from_col}</ddl400:FromColumnID>"#,
                    r#"<ddl400:ToTableID>{to_table}</ddl400:ToTableID>"#,
                    r#"<ddl400:ToColumnID>{to_col}</ddl400:ToColumnID>"#,
                    r#"<ddl400:IsActive>{active}</ddl400:IsActive>"#,
                    r#"<ddl400:CrossFilteringBehavior>{cross_filter}</ddl400:CrossFilteringBehavior>"#,
                    r#"</ddl400:Relationship>"#,
                ),
                name        = xml_escape_value(&r.name),
                from_table  = xml_escape_value(&r.from_table),
                from_col    = xml_escape_value(&r.from_column),
                to_table    = xml_escape_value(&r.to_table),
                to_col      = xml_escape_value(&r.to_column),
                active      = r.is_active,
                cross_filter = cross_filter,
            )
        })
        .collect()
}

pub fn discover_xml_metadata(
    session_id: Option<&str>,
    object_expansion: Option<&str>,
    databases: &[DatabaseMeta],
    database_tom_xml: Option<String>,
    meta: Option<&ModelMeta>,
    config: &ServerConfig,
) -> (String, Response) {
    if object_expansion == Some("ReferenceOnly") {
        let schema = make_xmldoc_schema("METADATA");

        let db_refs: String = databases
            .iter()
            .map(|db| {
                format!(
                    r#"<Database><ID>{id}</ID><Name>{name}</Name></Database>"#,
                    id = xml_escape_value(&db.id),
                    name = xml_escape_value(&db.name),
                )
            })
            .collect();

        let default_meta;
        let m = match meta {
            Some(m) => m,
            None => {
                default_meta = ModelMeta::default();
                &default_meta
            }
        };

        let server_xml = format!(
            concat!(
                r#"<Server xmlns="http://schemas.microsoft.com/analysisservices/2003/engine">"#,
                r#"<Name>{server_name}</Name>"#,
                r#"<ID>{server_name}</ID>"#,
                r#"<CreatedTimestamp>{created}</CreatedTimestamp>"#,
                r#"<LastSchemaUpdate>{last_update}</LastSchemaUpdate>"#,
                r#"<Version>{server_version}</Version>"#,
                r#"<Edition>Enterprise64</Edition>"#,
                r#"<EditionID>-2117995759</EditionID>"#,
                r#"<ServerMode>Tabular</ServerMode>"#,
                r#"<ServerLocation>Local</ServerLocation>"#,
                r#"<DefaultCompatibilityLevel>{compat}</DefaultCompatibilityLevel>"#,
                r#"<SupportedCompatibilityLevels>1200,1400,1500</SupportedCompatibilityLevels>"#,
                r#"<CompatibilityMode>PowerBI</CompatibilityMode>"#,
                r#"<SupportsNewMetadataVersioning>true</SupportsNewMetadataVersioning>"#,
                r#"<Databases>{db_refs}</Databases>"#,
                r#"</Server>"#,
            ),
            server_name = xml_escape_attr(&config.server_name),
            created = xml_escape_value(&m.created_timestamp),
            last_update = xml_escape_value(&m.last_schema_update),
            compat = m.compatibility_level,
            db_refs = db_refs,
            server_version = SERVER_VERSION,
        );

        let rows = format!("<row><METADATA>{server_xml}</METADATA></row>");
        return ok_xml(session_id, rowset(&schema, &rows));
    }

    let schema = make_xmldoc_schema("METADATA");
    let metadata_xml = database_tom_xml.unwrap_or_default();
    let rows = format!("<row><METADATA>{metadata_xml}</METADATA></row>");
    ok_xml(session_id, rowset(&schema, &rows))
}

fn xsd_to_tom_data_type(xsd: &str) -> i32 {
    match xsd {
        "string" => 2,
        "integer" | "unsignedLong" | "long" | "int" => 6,
        "double" | "float" => 8,
        "dateTime" => 9,
        "decimal" => 10,
        "boolean" => 11,
        "base64Binary" => 17,
        _ => 2,
    }
}

pub fn tmschema_model(
    session_id: Option<&str>,
    db_name: &str,
    meta: &ModelMeta,
) -> (String, Response) {
    let storage_mode = match meta.storage_engine_used.as_str() {
        "InMemory" => 1,
        "DirectQuery" => 2,
        _ => 1,
    };
    let default_mode = match meta.default_mode.as_str() {
        "Import" => 1,
        "DirectQuery" => 2,
        "Dual" => 3,
        "Push" => 4,
        _ => 1,
    };
    let schema = make_schema(&[
        ("ID", "int"),
        ("Name", "string"),
        ("Description", "string"),
        ("StorageMode", "int"),
        ("DefaultMode", "int"),
        ("Culture", "string"),
        ("CompatibilityLevel", "int"),
    ]);
    let rows = format!(
        r#"<row><ID>1</ID><Name>{name}</Name><Description/><StorageMode>{storage_mode}</StorageMode><DefaultMode>{default_mode}</DefaultMode><Culture>{culture}</Culture><CompatibilityLevel>{compat}</CompatibilityLevel></row>"#,
        name = xml_escape_value(db_name),
        culture = xml_escape_value(&meta.culture),
        compat = meta.compatibility_level,
    );
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn tmschema_tables(session_id: Option<&str>, tables: &[TableMeta]) -> (String, Response) {
    let schema = make_schema(&[
        ("ID", "int"),
        ("ModelID", "int"),
        ("Name", "string"),
        ("DataCategory", "string"),
        ("Description", "string"),
        ("IsHidden", "boolean"),
        ("IsPrivate", "boolean"),
        ("ShowAsVariationsOnly", "boolean"),
        ("StorageMode", "int"),
    ]);
    let mut rows = String::new();
    for (i, table) in tables.iter().enumerate() {
        let id = i + 1;
        rows.push_str(&format!(
            r#"<row><ID>{id}</ID><ModelID>1</ModelID><Name>{name}</Name><DataCategory>{cat}</DataCategory><Description>{desc}</Description><IsHidden>{hidden}</IsHidden><IsPrivate>false</IsPrivate><ShowAsVariationsOnly>false</ShowAsVariationsOnly><StorageMode>1</StorageMode></row>"#,
            name = xml_escape_value(&table.name),
            cat  = table.data_category.as_ref().map(|c| xml_escape_value(c.as_str())).unwrap_or_default(),
            desc = table.description.as_deref().map(xml_escape_value).unwrap_or_default(),
            hidden = table.is_hidden,
        ));
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn tmschema_columns(session_id: Option<&str>, tables: &[TableMeta]) -> (String, Response) {
    let schema = make_schema(&[
        ("ID", "int"),
        ("TableID", "int"),
        ("ExplicitName", "string"),
        ("InferredDataType", "int"),
        ("ExplicitDataType", "int"),
        ("DataCategory", "string"),
        ("Description", "string"),
        ("IsHidden", "boolean"),
        ("IsUnique", "boolean"),
        ("IsKey", "boolean"),
        ("IsNullable", "boolean"),
        ("Alignment", "int"),
        ("TableDetailPosition", "int"),
        ("IsDefaultLabel", "boolean"),
        ("IsDefaultImage", "boolean"),
        ("SummarizableBy", "int"),
        ("Type", "int"),
        ("IsAvailableInMDX", "boolean"),
        ("DisplayOrdinal", "int"),
        ("ErrorMessage", "string"),
        ("FormatString", "string"),
        ("DisplayFolder", "string"),
        ("SortByColumnID", "int"),
    ]);
    let mut col_id_map = std::collections::HashMap::new();
    for (t_idx, table) in tables.iter().enumerate() {
        let table_id = t_idx + 1;
        for (c_idx, col) in table.columns.iter().enumerate() {
            col_id_map.insert(
                (table.name.as_str(), col.name.as_str()),
                table_id * 1000 + c_idx + 1,
            );
        }
    }
    let mut rows = String::new();
    for (t_idx, table) in tables.iter().enumerate() {
        let table_id = t_idx + 1;
        for (c_idx, col) in table.columns.iter().enumerate() {
            let col_id = table_id * 1000 + c_idx + 1;
            let data_type = xsd_to_tom_data_type(&col.data_type);
            let sort_by_id = col
                .sort_by_column
                .as_deref()
                .and_then(|s| col_id_map.get(&(table.name.as_str(), s)).copied())
                .unwrap_or(0);
            rows.push_str(&format!(
                r#"<row><ID>{col_id}</ID><TableID>{table_id}</TableID><ExplicitName>{name}</ExplicitName><InferredDataType>{data_type}</InferredDataType><ExplicitDataType>{data_type}</ExplicitDataType><DataCategory>{cat}</DataCategory><Description>{desc}</Description><IsHidden>{hidden}</IsHidden><IsUnique>{is_unique}</IsUnique><IsKey>{is_key}</IsKey><IsNullable>{is_nullable}</IsNullable><Alignment>0</Alignment><TableDetailPosition>0</TableDetailPosition><IsDefaultLabel>false</IsDefaultLabel><IsDefaultImage>false</IsDefaultImage><SummarizableBy>0</SummarizableBy><Type>1</Type><IsAvailableInMDX>true</IsAvailableInMDX><DisplayOrdinal>{c_idx}</DisplayOrdinal><ErrorMessage/><FormatString>{fmt}</FormatString><DisplayFolder>{folder}</DisplayFolder><SortByColumnID>{sort_by_id}</SortByColumnID></row>"#,
                name = xml_escape_value(&col.name),
                cat  = col.data_category.as_ref().map(|c| xml_escape_value(c.as_str())).unwrap_or_default(),
                desc = col.description.as_deref().map(xml_escape_value).unwrap_or_default(),
                hidden = col.is_hidden,
                is_key = col.is_key,
                is_nullable = col.is_nullable,
                is_unique = col.is_unique,
                fmt = col.format_string.as_deref().map(xml_escape_value).unwrap_or_default(),
                folder = col.display_folder.as_deref().map(xml_escape_value).unwrap_or_default(),
            ));
        }
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn tmschema_measures(
    session_id: Option<&str>,
    measures: &[MeasureMeta],
    tables: &[TableMeta],
) -> (String, Response) {
    let schema = make_schema(&[
        ("ID", "int"),
        ("TableID", "int"),
        ("Name", "string"),
        ("Description", "string"),
        ("Expression", "string"),
        ("FormatString", "string"),
        ("DataType", "int"),
        ("IsHidden", "boolean"),
        ("DisplayFolder", "string"),
        ("ErrorMessage", "string"),
        ("IsSimpleMeasure", "boolean"),
        ("State", "int"),
    ]);
    let table_id_map: std::collections::HashMap<&str, usize> = tables
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.as_str(), i + 1))
        .collect();
    let mut rows = String::new();
    for (i, m) in measures.iter().enumerate() {
        let id = i + 1;
        let table_id = table_id_map
            .get(m.table_name.as_str())
            .copied()
            .unwrap_or(1);
        let desc = m
            .description
            .as_deref()
            .map(xml_escape_value)
            .unwrap_or_default();
        let fmt = m
            .format_string
            .as_deref()
            .map(xml_escape_value)
            .unwrap_or_default();
        let folder = m
            .display_folder
            .as_deref()
            .map(xml_escape_value)
            .unwrap_or_default();
        rows.push_str(&format!(
            r#"<row><ID>{id}</ID><TableID>{table_id}</TableID><Name>{name}</Name><Description>{desc}</Description><Expression>{expr}</Expression><FormatString>{fmt}</FormatString><DataType>8</DataType><IsHidden>{hidden}</IsHidden><DisplayFolder>{folder}</DisplayFolder><ErrorMessage/><IsSimpleMeasure>true</IsSimpleMeasure><State>1</State></row>"#,
            name = xml_escape_value(&m.name),
            expr = xml_escape_value(&m.expression),
            hidden = m.is_hidden,
        ));
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn tmschema_relationships(
    session_id: Option<&str>,
    relationships: &[RelationshipMeta],
    tables: &[TableMeta],
) -> (String, Response) {
    let schema = make_schema(&[
        ("ID", "int"),
        ("Name", "string"),
        ("FromTableID", "int"),
        ("FromColumnID", "int"),
        ("FromCardinality", "int"),
        ("ToTableID", "int"),
        ("ToColumnID", "int"),
        ("ToCardinality", "int"),
        ("CrossFilteringBehavior", "int"),
        ("IsActive", "boolean"),
        ("RelyOnReferentialIntegrity", "boolean"),
        ("SecurityFilteringBehavior", "int"),
        ("JoinOnDateBehavior", "int"),
        ("State", "int"),
    ]);
    let mut table_id_map = std::collections::HashMap::new();
    let mut col_id_map = std::collections::HashMap::new();
    for (t_idx, table) in tables.iter().enumerate() {
        let table_id = t_idx + 1;
        table_id_map.insert(table.name.as_str(), table_id);
        for (c_idx, col) in table.columns.iter().enumerate() {
            col_id_map.insert(
                (table.name.as_str(), col.name.as_str()),
                table_id * 1000 + c_idx + 1,
            );
        }
    }
    let mut rows = String::new();
    for (i, rel) in relationships.iter().enumerate() {
        let id = i + 1;
        let from_table_id = table_id_map
            .get(rel.from_table.as_str())
            .copied()
            .unwrap_or(0);
        let from_col_id = col_id_map
            .get(&(rel.from_table.as_str(), rel.from_column.as_str()))
            .copied()
            .unwrap_or(0);
        let to_table_id = table_id_map
            .get(rel.to_table.as_str())
            .copied()
            .unwrap_or(0);
        let to_col_id = col_id_map
            .get(&(rel.to_table.as_str(), rel.to_column.as_str()))
            .copied()
            .unwrap_or(0);
        let cross_filter = if rel.bidirectional { 2 } else { 1 };
        rows.push_str(&format!(
            r#"<row><ID>{id}</ID><Name>{name}</Name><FromTableID>{ftid}</FromTableID><FromColumnID>{fcid}</FromColumnID><FromCardinality>2</FromCardinality><ToTableID>{ttid}</ToTableID><ToColumnID>{tcid}</ToColumnID><ToCardinality>1</ToCardinality><CrossFilteringBehavior>{cf}</CrossFilteringBehavior><IsActive>{active}</IsActive><RelyOnReferentialIntegrity>false</RelyOnReferentialIntegrity><SecurityFilteringBehavior>1</SecurityFilteringBehavior><JoinOnDateBehavior>0</JoinOnDateBehavior><State>1</State></row>"#,
            name = xml_escape_value(&rel.name),
            ftid = from_table_id,
            fcid = from_col_id,
            ttid = to_table_id,
            tcid = to_col_id,
            cf = cross_filter,
            active = rel.is_active,
        ));
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn tmschema_partitions(session_id: Option<&str>, tables: &[TableMeta]) -> (String, Response) {
    let schema = make_schema(&[
        ("ID", "int"),
        ("TableID", "int"),
        ("Name", "string"),
        ("Mode", "int"),
        ("State", "int"),
        ("Type", "int"),
        ("Description", "string"),
        ("ErrorMessage", "string"),
    ]);
    let mut rows = String::new();
    for (t_idx, table) in tables.iter().enumerate() {
        let table_id = t_idx + 1;
        rows.push_str(&format!(
            r#"<row><ID>{table_id}</ID><TableID>{table_id}</TableID><Name>{name}</Name><Mode>1</Mode><State>4</State><Type>1</Type><Description/><ErrorMessage/></row>"#,
            name = xml_escape_value(&table.name),
        ));
    }
    ok_xml(session_id, rowset(&schema, &rows))
}

pub fn discover_csdl_metadata(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    measures: &[MeasureMeta],
    relationships: &[RelationshipMeta],
    version: &str,
) -> (String, Response) {
    let schema = make_xmldoc_schema("METADATA");
    let csdl = build_csdl(catalog, tables, measures, relationships, version);
    let rows = format!("<row><METADATA>{csdl}</METADATA></row>");
    ok_xml(session_id, rowset(&schema, &rows))
}

fn csdl_col_type(xsd: &str) -> (&'static str, bool) {
    match xsd {
        "string" => ("String", true),
        "integer" | "int" | "long" | "unsignedLong" => ("Int64", false),
        "double" | "float" => ("Double", false),
        "decimal" => ("Decimal", false),
        "dateTime" => ("DateTime", false),
        "boolean" => ("Boolean", false),
        _ => ("String", true),
    }
}

fn csdl_measure_type(oledb_type: u16) -> &'static str {
    match oledb_type {
        3 | 20 => "Int64",
        5 => "Double",
        6 => "Decimal",
        7 => "DateTime",
        11 => "Boolean",
        _ => "Double",
    }
}

fn lineage_tag(scope: &str, name: &str) -> String {
    let key = format!("{scope}\0{name}");
    Uuid::new_v5(&Uuid::NAMESPACE_OID, key.as_bytes()).to_string()
}

fn build_csdl(
    catalog: &str,
    tables: &[TableMeta],
    measures: &[MeasureMeta],
    relationships: &[RelationshipMeta],
    version: &str,
) -> String {
    let ns = xml_escape_value(catalog);
    let bi_version = if version == "2.0" { "2.0" } else { "2.5" };
    const RN: &str = "RowNumber_2662979B_1795_4F74_8F37_6A1BA8059B61";
    const RN_CAP: &str = "RowNumber-2662979B-1795-4F74-8F37-6A1BA8059B61";

    // EntitySets
    let mut entity_sets = String::new();
    for table in tables {
        let t = xml_escape_value(&table.name);
        let tag = lineage_tag("table", &table.name);
        let hidden_attr = if table.is_hidden {
            r#" Hidden="true""#
        } else {
            ""
        };
        entity_sets.push_str(&format!(
            r#"<ns5:EntitySet Name="{t}" EntityType="{ns}.{t}"><bi:EntitySet LineageTag="{tag}"{hidden_attr}/></ns5:EntitySet>"#
        ));
    }

    // AssociationSets — v2.0 omits Role on End elements, v2.5 includes them.
    let mut assoc_sets = String::new();
    for rel in relationships {
        let rname = xml_escape_value(&rel.name);
        let ft = xml_escape_value(&rel.from_table);
        let fc = xml_escape_value(&rel.from_column);
        let tt = xml_escape_value(&rel.to_table);
        let tc = xml_escape_value(&rel.to_column);
        let from_role = format!("{ft}_{fc}");
        let to_role = format!("{tt}_{tc}");
        let mut bi_attrs = String::new();
        if !rel.is_active {
            bi_attrs.push_str(r#" State="Inactive""#);
        }
        if rel.bidirectional {
            bi_attrs.push_str(r#" CrossFilterDirection="Both""#);
        }
        if bi_version == "2.0" {
            assoc_sets.push_str(&format!(
                r#"<ns5:AssociationSet Name="{rname}" Association="{ns}.{rname}"><ns5:End EntitySet="{ft}"/><ns5:End EntitySet="{tt}"/><bi:AssociationSet{bi_attrs}/></ns5:AssociationSet>"#
            ));
        } else {
            assoc_sets.push_str(&format!(
                r#"<ns5:AssociationSet Name="{rname}" Association="{ns}.{rname}"><ns5:End EntitySet="{ft}" Role="{from_role}"/><ns5:End EntitySet="{tt}" Role="{to_role}"/><bi:AssociationSet{bi_attrs}/></ns5:AssociationSet>"#
            ));
        }
    }

    let bi_container = format!(
        concat!(
            r#"<bi:EntityContainer Caption="{caption}" Culture="en-US" DirectQueryMode="Import">"#,
            r#"<bi:ModelCapabilities>"#,
            r#"<bi:DiscourageCompositeModels>0</bi:DiscourageCompositeModels>"#,
            r#"<bi:EncourageIsEmptyDAXFunctionUsage>true</bi:EncourageIsEmptyDAXFunctionUsage>"#,
            r#"<bi:QueryBatching>1</bi:QueryBatching>"#,
            r#"<bi:Variables>1</bi:Variables>"#,
            r#"<bi:InOperator>1</bi:InOperator>"#,
            r#"<bi:TableConstructor>1</bi:TableConstructor>"#,
            r#"<bi:ExecutionMetrics>1</bi:ExecutionMetrics>"#,
            r#"<bi:VirtualColumns>0</bi:VirtualColumns>"#,
            r#"<bi:VisualCalculations>0</bi:VisualCalculations>"#,
            r#"<bi:DAXFunctions>"#,
            r#"<bi:SummarizeColumns>1</bi:SummarizeColumns>"#,
            r#"<bi:SubstituteWithIndex>1</bi:SubstituteWithIndex>"#,
            r#"<bi:LeftOuterJoin>1</bi:LeftOuterJoin>"#,
            r#"<bi:StringMinMax>1</bi:StringMinMax>"#,
            r#"<bi:TreatAs>1</bi:TreatAs>"#,
            r#"<bi:Error>1</bi:Error>"#,
            r#"<bi:OptimizedNotInOperator>1</bi:OptimizedNotInOperator>"#,
            r#"<bi:NonVisual>0</bi:NonVisual>"#,
            r#"</bi:DAXFunctions>"#,
            r#"</bi:ModelCapabilities>"#,
            r#"</bi:EntityContainer>"#,
        ),
        caption = ns,
    );

    // EntityTypes
    let mut entity_types = String::new();
    for table in tables {
        let t = xml_escape_value(&table.name);
        let mut props = format!(
            r#"<ns5:Property Name="{RN}" Type="Int64" Nullable="false"><bi:Property Caption="{RN_CAP}" ReferenceName="{RN_CAP}" Hidden="true" Contents="RowNumber" Stability="RowNumber"/></ns5:Property>"#
        );

        for col in &table.columns {
            let cname = to_edm_name(&col.name);
            let (ctype, is_str) = csdl_col_type(&col.data_type);
            let tag = lineage_tag(&table.name, &col.name);
            let str_attrs = if is_str {
                r#" MaxLength="Max" Unicode="true" FixedLength="false""#
            } else {
                ""
            };
            let fmt_attr = match &col.format_string {
                Some(fs) => format!(r#" FormatString="{}""#, xml_escape_attr(fs)),
                None if is_str => String::new(),
                None => r#" FormatString="0""#.to_string(),
            };
            let agg_fn = col.summarize_by.as_csdl_str();
            let hidden_attr = if col.is_hidden {
                r#" Hidden="true""#
            } else {
                ""
            };
            let folder_attr = match col.display_folder.as_deref().filter(|s| !s.is_empty()) {
                Some(f) => format!(r#" DisplayFolder="{}""#, xml_escape_attr(f)),
                None => String::new(),
            };
            props.push_str(&format!(
                r#"<ns5:Property Name="{cname}" Type="{ctype}"{str_attrs}><bi:Property{fmt_attr} DefaultAggregateFunction="{agg_fn}" LineageTag="{tag}"{hidden_attr}{folder_attr}/></ns5:Property>"#
            ));
        }

        for m in measures.iter().filter(|m| m.table_name == table.name) {
            let mname = to_edm_name(&m.name);
            let caption = xml_escape_attr(&m.display_name);
            let mtype = csdl_measure_type(m.data_type);
            let tag = lineage_tag(&table.name, &m.name);
            let distributive_by = if m.aggregator == 1 {
                format!(
                    r#"<bi:DistributiveBy AggregationKind="Sum"><bi:EntityRef Name="{t}"/></bi:DistributiveBy>"#
                )
            } else {
                String::new()
            };
            let m_fmt = m.format_string.as_deref().unwrap_or("0");
            let hidden_attr = if m.is_hidden { r#" Hidden="true""# } else { "" };
            let folder_attr = match m.display_folder.as_deref().filter(|s| !s.is_empty()) {
                Some(f) => format!(r#" DisplayFolder="{}""#, xml_escape_attr(f)),
                None => String::new(),
            };
            props.push_str(&format!(
                r#"<ns5:Property Name="{mname}" Type="{mtype}"><bi:Measure Caption="{caption}" ReferenceName="{caption}" FormatString="{fmt}" LineageTag="{tag}"{hidden_attr}{folder_attr}><bi:ContainsDetailRows>false</bi:ContainsDetailRows>{distributive_by}</bi:Measure></ns5:Property>"#,
                fmt = xml_escape_attr(m_fmt),
            ));
        }

        for rel in relationships.iter().filter(|r| r.from_table == table.name) {
            let rname = xml_escape_attr(&rel.name);
            let ft = xml_escape_attr(&rel.from_table);
            let fc = xml_escape_attr(&rel.from_column);
            let tt = xml_escape_attr(&rel.to_table);
            let tc = xml_escape_attr(&rel.to_column);
            let from_role = format!("{ft}_{fc}");
            let to_role = format!("{tt}_{tc}");
            props.push_str(&format!(
                r#"<ns5:NavigationProperty Name="{ft}_{fc}" Relationship="{ns}.{rname}" FromRole="{from_role}" ToRole="{to_role}"><bi:NavigationProperty/></ns5:NavigationProperty>"#
            ));
        }

        entity_types.push_str(&format!(
            r#"<ns5:EntityType Name="{t}"><ns5:Key><ns5:PropertyRef Name="{RN}"/></ns5:Key>{props}<bi:EntityType/></ns5:EntityType>"#
        ));
    }

    let mut associations = String::new();
    for rel in relationships {
        let rname = xml_escape_attr(&rel.name);
        let ft = xml_escape_attr(&rel.from_table);
        let fc = xml_escape_attr(&rel.from_column);
        let tt = xml_escape_attr(&rel.to_table);
        let tc = xml_escape_attr(&rel.to_column);
        let from_role = format!("{ft}_{fc}");
        let to_role = format!("{tt}_{tc}");
        // from_table = many-side (FK/fact) = Dependent, Multiplicity "*"
        // to_table   = one-side (PK/dim) = Principal, Multiplicity "0..1"
        let referential_constraint = if bi_version == "2.5" {
            format!(
                r#"<ns5:ReferentialConstraint><ns5:Principal Role="{to_role}"><ns5:PropertyRef Name="{tc}"/></ns5:Principal><ns5:Dependent Role="{from_role}"><ns5:PropertyRef Name="{fc}"/></ns5:Dependent></ns5:ReferentialConstraint>"#
            )
        } else {
            String::new()
        };
        associations.push_str(&format!(
            r#"<ns5:Association Name="{rname}">{referential_constraint}<ns5:End Role="{from_role}" Type="{ns}.{ft}" Multiplicity="*"/><ns5:End Role="{to_role}" Type="{ns}.{tt}" Multiplicity="0..1"/></ns5:Association>"#
        ));
    }

    format!(
        r#"<ns5:Schema bi:Version="{bi_version}" Namespace="{ns}" xmlns:ns5="http://schemas.microsoft.com/ado/2008/09/edm" xmlns:bi="http://schemas.microsoft.com/sqlbi/2010/10/edm/extensions"><ns5:EntityContainer Name="{ns}">{entity_sets}{assoc_sets}{bi_container}</ns5:EntityContainer>{entity_types}{associations}</ns5:Schema>"#
    )
}

pub fn discover_functions(session_id: Option<&str>, origin: Option<u32>) -> (String, Response) {
    const SCHEMA: &str = concat!(
        r#"<xsd:schema xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:sql="urn:schemas-microsoft-com:xml-sql" targetNamespace="urn:schemas-microsoft-com:xml-analysis:rowset" elementFormDefault="qualified">"#,
        r#"<xsd:element name="root"><xsd:complexType><xsd:sequence minOccurs="0" maxOccurs="unbounded"><xsd:element name="row" type="row" minOccurs="0" maxOccurs="unbounded"/></xsd:sequence></xsd:complexType></xsd:element>"#,
        r#"<xsd:complexType name="row"><xsd:sequence>"#,
        r#"<xsd:element sql:field="FUNCTION_NAME" name="FUNCTION_NAME" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="DESCRIPTION" name="DESCRIPTION" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="PARAMETER_LIST" name="PARAMETER_LIST" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="RETURN_TYPE" name="RETURN_TYPE" type="xsd:int" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="ORIGIN" name="ORIGIN" type="xsd:int" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="INTERFACE_NAME" name="INTERFACE_NAME" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="LIBRARY_NAME" name="LIBRARY_NAME" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="DLL_NAME" name="DLL_NAME" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="HELP_FILE" name="HELP_FILE" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="HELP_CONTEXT" name="HELP_CONTEXT" type="xsd:int" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="OBJECT" name="OBJECT" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="CAPTION" name="CAPTION" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="PARAMETERINFO" name="PARAMETERINFO" minOccurs="0" maxOccurs="unbounded">"#,
        r#"<xsd:complexType><xsd:sequence>"#,
        r#"<xsd:element sql:field="NAME" name="NAME" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="DESCRIPTION" name="DESCRIPTION" type="xsd:string" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="OPTIONAL" name="OPTIONAL" type="xsd:boolean" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="REPEATABLE" name="REPEATABLE" type="xsd:boolean" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="REPEATGROUP" name="REPEATGROUP" type="xsd:int" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="SKIPPABLE" name="SKIPPABLE" type="xsd:boolean" minOccurs="0"/>"#,
        r#"</xsd:sequence></xsd:complexType></xsd:element>"#,
        r#"<xsd:element sql:field="DIRECTQUERY_PUSHABLE" name="DIRECTQUERY_PUSHABLE" type="xsd:int" minOccurs="0"/>"#,
        r#"<xsd:element sql:field="VISUAL_CALCULATIONS_INFO" name="VISUAL_CALCULATIONS_INFO" type="xsd:int" minOccurs="0"/>"#,
        r#"</xsd:sequence></xsd:complexType></xsd:schema>"#,
    );

    const ROWS_ORIGIN3: &str = include_str!("functions_origin3.xml");

    let dynamic_buf;
    let rows: &str = match origin {
        Some(3) => ROWS_ORIGIN3,
        Some(4) => {
            dynamic_buf = build_dynamic_functions_xml();
            &dynamic_buf
        }
        _ => "",
    };

    ok_xml(session_id, rowset(SCHEMA, rows))
}

pub fn discover_dbschema_tables(
    session_id: Option<&str>,
    catalog: &str,
    tables: &[TableMeta],
    created_at: &str,
    last_modified: &str,
) -> (String, Response) {
    let schema = make_schema(&[
        ("TABLE_CATALOG", "string"),
        ("TABLE_SCHEMA", "string"),
        ("TABLE_NAME", "string"),
        ("TABLE_TYPE", "string"),
        ("TABLE_GUID", "string"),
        ("DESCRIPTION", "string"),
        ("TABLE_PROPID", "unsignedInt"),
        ("DATE_CREATED", "dateTime"),
        ("DATE_MODIFIED", "dateTime"),
        ("TABLE_OLAP_TYPE", "string"),
    ]);

    let mut rows = String::new();
    let cat = xml_escape_value(catalog);
    let date_created = xml_escape_value(created_at);
    let date_modified = xml_escape_value(last_modified);
    for table in tables {
        if table.is_hidden {
            continue;
        }
        let name = xml_escape_value(&table.name);
        rows.push_str(&format!(
            "<row>\
            <TABLE_CATALOG>{cat}</TABLE_CATALOG>\
            <TABLE_SCHEMA>Model</TABLE_SCHEMA>\
            <TABLE_NAME>{name}</TABLE_NAME>\
            <TABLE_TYPE>SYSTEM TABLE</TABLE_TYPE>\
            <DESCRIPTION/>\
            <DATE_CREATED>{date_created}</DATE_CREATED>\
            <DATE_MODIFIED>{date_modified}</DATE_MODIFIED>\
            <TABLE_OLAP_TYPE>MEASURE_GROUP</TABLE_OLAP_TYPE>\
            </row>"
        ));
        rows.push_str(&format!(
            "<row>\
            <TABLE_CATALOG>{cat}</TABLE_CATALOG>\
            <TABLE_SCHEMA>Model</TABLE_SCHEMA>\
            <TABLE_NAME>${name}</TABLE_NAME>\
            <TABLE_TYPE>TABLE</TABLE_TYPE>\
            <DESCRIPTION/>\
            <DATE_CREATED>{date_created}</DATE_CREATED>\
            <DATE_MODIFIED>{date_modified}</DATE_MODIFIED>\
            <TABLE_OLAP_TYPE>CUBE_DIMENSION</TABLE_OLAP_TYPE>\
            </row>"
        ));
    }

    const SYSTEM_SCHEMAS: &[(&str, &str)] = &[
        ("DBSCHEMA_CATALOGS", "c8b52211-5cf3-11ce-ade5-00aa0044773d"),
        ("DBSCHEMA_TABLES", "c8b52229-5cf3-11ce-ade5-00aa0044773d"),
        ("DBSCHEMA_COLUMNS", "c8b52214-5cf3-11ce-ade5-00aa0044773d"),
        (
            "DBSCHEMA_PROVIDER_TYPES",
            "c8b5222c-5cf3-11ce-ade5-00aa0044773d",
        ),
        ("MDSCHEMA_CUBES", "c8b522d8-5cf3-11ce-ade5-00aa0044773d"),
        (
            "MDSCHEMA_DIMENSIONS",
            "c8b522d9-5cf3-11ce-ade5-00aa0044773d",
        ),
        (
            "MDSCHEMA_HIERARCHIES",
            "c8b522da-5cf3-11ce-ade5-00aa0044773d",
        ),
        ("MDSCHEMA_LEVELS", "c8b522db-5cf3-11ce-ade5-00aa0044773d"),
        ("MDSCHEMA_MEASURES", "c8b522dc-5cf3-11ce-ade5-00aa0044773d"),
        (
            "MDSCHEMA_PROPERTIES",
            "c8b522dd-5cf3-11ce-ade5-00aa0044773d",
        ),
        ("MDSCHEMA_MEMBERS", "c8b522de-5cf3-11ce-ade5-00aa0044773d"),
        ("MDSCHEMA_FUNCTIONS", "a07ccd07-8148-11d0-87bb-00c04fc33942"),
        ("MDSCHEMA_SETS", "a07ccd0b-8148-11d0-87bb-00c04fc33942"),
        ("DISCOVER_INSTANCES", "20518699-2474-4c15-9885-0e947ec7a7e3"),
        ("MDSCHEMA_KPIS", "2ae44109-ed3d-4842-b16f-b694d1cb0e3f"),
        (
            "MDSCHEMA_MEASUREGROUPS",
            "e1625ebf-fa96-42fd-bea6-db90adafd96b",
        ),
        (
            "MDSCHEMA_MEASUREGROUP_DIMENSIONS",
            "a07ccd33-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "MDSCHEMA_INPUT_DATASOURCES",
            "a07ccd32-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DMSCHEMA_MINING_SERVICES",
            "3add8a95-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_SERVICE_PARAMETERS",
            "3add8a75-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_FUNCTIONS",
            "3add8a79-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_MODEL_CONTENT",
            "3add8a76-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_MODEL_XML",
            "4290b2d5-0e9c-4aa7-9369-98c95cfd9d13",
        ),
        (
            "DMSCHEMA_MINING_MODEL_CONTENT_PMML",
            "4290b2d5-0e9c-4aa7-9369-98c95cfd9d13",
        ),
        (
            "DMSCHEMA_MINING_MODELS",
            "3add8a77-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_COLUMNS",
            "3add8a78-d8b9-11d2-8d2a-00e029154fde",
        ),
        (
            "DMSCHEMA_MINING_STRUCTURES",
            "883269f3-0cad-462f-b6f5-e88a72418c4b",
        ),
        (
            "DMSCHEMA_MINING_STRUCTURE_COLUMNS",
            "9952e836-bfbf-4d1f-8535-9b67dbd9ddfe",
        ),
        (
            "DISCOVER_PROPERTIES",
            "4b40adfb-8b09-4758-97bb-636e8ae97bcf",
        ),
        (
            "DISCOVER_SCHEMA_ROWSETS",
            "eea0302b-7922-4992-8991-0e605d0e5593",
        ),
        (
            "DISCOVER_ENUMERATORS",
            "55a9e78b-accb-45b4-95a6-94c5065617a7",
        ),
        ("DISCOVER_KEYWORDS", "1426c443-4cdd-4a40-8f45-572fab9bbaa1"),
        ("DISCOVER_LITERALS", "c3ef5ecb-0a07-4665-a140-b075722dbdc2"),
        ("DISCOVER_TRACES", "a07ccd1a-8148-11d0-87bb-00c04fc33942"),
        (
            "DISCOVER_TRACE_DEFINITION_PROVIDERINFO",
            "a07ccd1b-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_XEVENT_PACKAGES",
            "a07ccd1c-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_XEVENT_OBJECTS",
            "a07ccd1d-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_XEVENT_OBJECT_COLUMNS",
            "a07ccd1e-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_XEVENT_SESSION_TARGETS",
            "a07ccd1f-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_XEVENT_SESSIONS",
            "a07ccd20-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_TRACE_COLUMNS",
            "a07ccd18-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_TRACE_EVENT_CATEGORIES",
            "a07ccd19-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_MEMORYUSAGE",
            "a07ccd21-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_MEMORYGRANT",
            "a07ccd23-8148-11d0-87bb-00c04fc33942",
        ),
        ("DISCOVER_LOCKS", "a07ccd24-8148-11d0-87bb-00c04fc33942"),
        (
            "DISCOVER_CONNECTIONS",
            "a07ccd25-8148-11d0-87bb-00c04fc33942",
        ),
        ("DISCOVER_SESSIONS", "a07ccd26-8148-11d0-87bb-00c04fc33942"),
        ("DISCOVER_JOBS", "a07ccd27-8148-11d0-87bb-00c04fc33942"),
        (
            "DISCOVER_TRANSACTIONS",
            "a07ccd28-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_DB_CONNECTIONS",
            "a07ccd2a-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_MASTER_KEY",
            "a07ccd29-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_PERFORMANCE_COUNTERS",
            "a07ccd2e-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_POWERBI_ROLES",
            "a07ccd8b-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_POWERBI_DATASOURCES",
            "a07ccd8d-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_PARTITION_DIMENSION_STAT",
            "a07ccd8e-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_PARTITION_STAT",
            "a07ccd8f-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_DIMENSION_STAT",
            "a07ccd90-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_M_EXPRESSIONS",
            "a07ccd93-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_MODEL_SECURITY",
            "a07ccd88-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_OBJECT_COUNTERS",
            "a07ccd89-8148-11d0-87bb-00c04fc33942",
        ),
        ("DISCOVER_MEM_STATS", "a07ccd8a-8148-11d0-87bb-00c04fc33942"),
        (
            "DISCOVER_DB_MEM_STATS",
            "a07ccd8c-8148-11d0-87bb-00c04fc33942",
        ),
        ("DISCOVER_COMMANDS", "a07ccd34-8148-11d0-87bb-00c04fc33942"),
        (
            "DISCOVER_COMMAND_OBJECTS",
            "a07ccd35-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_OBJECT_ACTIVITY",
            "a07ccd36-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_OBJECT_MEMORY_USAGE",
            "a07ccd37-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_STORAGE_TABLES",
            "a07ccd43-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_STORAGE_TABLE_COLUMNS",
            "a07ccd44-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_STORAGE_TABLE_COLUMN_SEGMENTS",
            "a07ccd45-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_CALC_DEPENDENCY",
            "a07ccd46-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_CSDL_METADATA",
            "87b86062-21c3-460f-b4f8-5be98394f13b",
        ),
        (
            "DISCOVER_RESOURCE_POOLS",
            "a07ccd47-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "DISCOVER_RING_BUFFERS",
            "a07ccd48-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_MODEL", "a07ccd49-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_DATA_SOURCES",
            "a07ccd4a-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_TABLES", "a07ccd4b-8148-11d0-87bb-00c04fc33942"),
        ("TMSCHEMA_COLUMNS", "a07ccd4c-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_ATTRIBUTE_HIERARCHIES",
            "a07ccd4d-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PARTITIONS",
            "a07ccd4e-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_RELATIONSHIPS",
            "a07ccd4f-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_MEASURES", "a07ccd50-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_HIERARCHIES",
            "a07ccd51-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_LEVELS", "a07ccd52-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_ANNOTATIONS",
            "a07ccd53-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_KPIS", "a07ccd5f-8148-11d0-87bb-00c04fc33942"),
        ("TMSCHEMA_CULTURES", "a07ccd63-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_OBJECT_TRANSLATIONS",
            "a07ccd64-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_LINGUISTIC_METADATA",
            "a07ccd65-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_STORAGE_FOLDERS",
            "a07ccd60-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_STORAGE_FILES",
            "a07ccd61-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_TABLE_STORAGES",
            "a07ccd55-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_COLUMN_STORAGES",
            "a07ccd56-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PARTITION_STORAGES",
            "a07ccd57-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_SEGMENT_MAP_STORAGES",
            "a07ccd58-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_DICTIONARY_STORAGES",
            "a07ccd59-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_COLUMN_PARTITION_STORAGES",
            "a07ccd5a-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_SEGMENT_STORAGES",
            "a07ccd62-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_RELATIONSHIP_STORAGES",
            "a07ccd5b-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_RELATIONSHIP_INDEX_STORAGES",
            "a07ccd5c-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_ATTRIBUTE_HIERARCHY_STORAGES",
            "a07ccd5d-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_HIERARCHY_STORAGES",
            "a07ccd5e-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PERSPECTIVES",
            "a07ccd66-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PERSPECTIVE_TABLES",
            "a07ccd67-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PERSPECTIVE_COLUMNS",
            "a07ccd68-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PERSPECTIVE_HIERARCHIES",
            "a07ccd69-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PERSPECTIVE_MEASURES",
            "a07ccd6a-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_ROLES", "a07ccd6b-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_ROLE_MEMBERSHIPS",
            "a07ccd6c-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_TABLE_PERMISSIONS",
            "a07ccd6d-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_VARIATIONS",
            "a07ccd6e-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_SETS", "a07ccd6f-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_PERSPECTIVE_SETS",
            "a07ccd70-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_EXTENDED_PROPERTIES",
            "a07ccd71-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_EXPRESSIONS",
            "a07ccd72-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_COLUMN_PERMISSIONS",
            "a07ccd73-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_DETAIL_ROWS_DEFINITIONS",
            "a07ccd54-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_RELATED_COLUMN_DETAILS",
            "a07ccd74-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_GROUP_BY_COLUMNS",
            "a07ccd75-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_CALCULATION_GROUPS",
            "a07ccd76-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_CALCULATION_ITEMS",
            "a07ccd77-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_ALTERNATE_OF_DEFINITIONS",
            "a07ccd78-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_REFRESH_POLICIES",
            "a07ccd79-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_FORMAT_STRING_DEFINITIONS",
            "a07ccd7a-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_QUERY_GROUPS",
            "a07ccd7b-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_ANALYTICS_AIMETADATA",
            "a07ccd7c-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_CHANGED_PROPERTIES",
            "5b5f186b-e834-4e61-af92-eb1e6bf3d21e",
        ),
        (
            "TMSCHEMA_EXCLUDED_ARTIFACTS",
            "e0b79227-1f53-41b4-bac8-5315e58f12f2",
        ),
        (
            "TMSCHEMA_GENERAL_SEGMENT_MAP_SEGMENT_METADATA_STORAGES",
            "a07ccd7f-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_DELTA_TABLE_METADATA_STORAGES",
            "a07ccd80-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_PARQUET_FILE_STORAGES",
            "a07ccd81-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_DATA_COVERAGE_DEFINITIONS",
            "a07ccd82-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_CALCULATION_EXPRESSIONS",
            "a07ccd83-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_CALENDARS", "a07ccd84-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_CALENDAR_COLUMN_GROUPS",
            "a07ccd85-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_CALENDAR_COLUMN_REFERENCES",
            "a07ccd86-8148-11d0-87bb-00c04fc33942",
        ),
        ("TMSCHEMA_FUNCTIONS", "a07ccd7d-8148-11d0-87bb-00c04fc33942"),
        (
            "TMSCHEMA_BINDING_INFO_COLLECTION",
            "a07ccd87-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_DELTA_TABLE_COLUMN_STORAGES",
            "a07ccd97-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_STRING_INDEX_STORAGES",
            "a07ccd98-8148-11d0-87bb-00c04fc33942",
        ),
        (
            "TMSCHEMA_COLUMN_INDEX_STORAGES",
            "a07ccd99-8148-11d0-87bb-00c04fc33942",
        ),
    ];
    for (name, guid) in SYSTEM_SCHEMAS {
        rows.push_str(&format!(
            "<row>\
            <TABLE_SCHEMA>$SYSTEM</TABLE_SCHEMA>\
            <TABLE_NAME>{name}</TABLE_NAME>\
            <TABLE_TYPE>SCHEMA</TABLE_TYPE>\
            <TABLE_GUID>{guid}</TABLE_GUID>\
            <TABLE_OLAP_TYPE>SCHEMA</TABLE_OLAP_TYPE>\
            </row>"
        ));
    }

    ok_xml(session_id, rowset(&schema, &rows))
}

fn build_dynamic_functions_xml() -> String {
    use crate::engine::functions::REGISTRY;

    let mut rows = String::new();
    let mut names: Vec<&str> = REGISTRY.iter_meta().map(|(n, _)| n).collect();
    names.sort_unstable();

    for name in names {
        let Some(meta) = REGISTRY.get_meta(name) else {
            continue;
        };

        let params: String = meta.params.iter().map(|p| {
            format!(
                "<PARAMETERINFO><NAME>{}</NAME><DESCRIPTION>{}</DESCRIPTION><OPTIONAL>{}</OPTIONAL><REPEATABLE>{}</REPEATABLE></PARAMETERINFO>",
                xml_escape_value(p.name),
                xml_escape_value(p.description),
                p.optional,
                p.repeatable,
            )
        }).collect();

        rows.push_str(&format!(
            "<row><FUNCTION_NAME>{name}</FUNCTION_NAME><DESCRIPTION>{desc}</DESCRIPTION><ORIGIN>4</ORIGIN><INTERFACE_NAME>{iface}</INTERFACE_NAME><LIBRARY_NAME>SCALAR</LIBRARY_NAME>{params}</row>",
            name = xml_escape_value(name),
            desc = xml_escape_value(meta.description),
            iface = xml_escape_value(meta.interface_name),
        ));
    }
    rows
}

// ── MDX cellset response ──────────────────────────────────────────────────────

// MDX-engine rewrite (tracked separately) will restructure these render entry
// points; args here are all real cellset-construction inputs, not incidental.
#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset(
    session_id: Option<&str>,
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    measure_name: Option<&str>,
    total_value: Option<&str>,
    leaf_members: &[(String, String)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    show_measures_axis: bool,
) -> (String, Response) {
    let inner = build_cellset_root(
        cube_name,
        axis,
        measure_name,
        total_value,
        leaf_members,
        cell_props,
        last_data_update,
        last_schema_update,
        show_measures_axis,
    );
    let xml = cellset_envelope(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

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

// XSD schema embedded in every mddataset response — same for all query shapes.
// Uses r###"..."### so the "## in ##targetNamespace does not close the literal.
const MDDATASET_SCHEMA: &str = r###"<xs:schema targetNamespace="urn:schemas-microsoft-com:xml-analysis:mddataset" elementFormDefault="qualified"><xs:import namespace="http://schemas.microsoft.com/analysisservices/2003/xmla" /><xs:complexType name="MemberType"><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="Hierarchy" type="xs:string" /></xs:complexType><xs:complexType name="PropType"><xs:sequence><xs:element name="Default" minOccurs="0" /></xs:sequence><xs:attribute name="name" type="xs:string" use="required" /><xs:attribute name="type" type="xs:QName" /></xs:complexType><xs:complexType name="TupleType"><xs:sequence><xs:element name="Member" type="MemberType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence></xs:complexType><xs:complexType name="MembersType"><xs:sequence><xs:element name="Member" type="MemberType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence><xs:attribute name="Hierarchy" type="xs:string" use="required" /></xs:complexType><xs:complexType name="TuplesType"><xs:sequence><xs:element name="Tuple" type="TupleType" minOccurs="0" maxOccurs="unbounded" /></xs:sequence></xs:complexType><xs:group name="SetType"><xs:choice><xs:element name="Members" type="MembersType" /><xs:element name="Tuples" type="TuplesType" /><xs:element name="CrossProduct" type="SetListType" /><xs:element ref="msxmla:NormTupleSet" /><xs:element name="Union"><xs:complexType><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /></xs:complexType></xs:element></xs:choice></xs:group><xs:complexType name="SetListType"><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /><xs:attribute name="Size" type="xs:unsignedInt" /></xs:complexType><xs:complexType name="OlapInfo"><xs:sequence><xs:element name="CubeInfo"><xs:complexType><xs:sequence><xs:element name="Cube" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:element name="CubeName" type="xs:string" /><xs:element name="LastDataUpdate" minOccurs="0" type="xs:dateTime" /><xs:element name="LastSchemaUpdate" minOccurs="0" type="xs:dateTime" /></xs:sequence></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element><xs:element name="AxesInfo"><xs:complexType><xs:sequence><xs:element name="AxisInfo" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:element name="HierarchyInfo" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="name" type="xs:string" use="required" /></xs:complexType></xs:element></xs:sequence><xs:attribute name="name" type="xs:string" /></xs:complexType></xs:element></xs:sequence></xs:complexType></xs:element><xs:element name="CellInfo"><xs:complexType><xs:choice minOccurs="0" maxOccurs="unbounded"><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:choice></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:complexType name="Axes"><xs:sequence><xs:element name="Axis" maxOccurs="unbounded"><xs:complexType><xs:group ref="SetType" minOccurs="0" maxOccurs="unbounded" /><xs:attribute name="name" type="xs:string" /></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:complexType name="CellData"><xs:sequence><xs:element name="Cell" minOccurs="0" maxOccurs="unbounded"><xs:complexType><xs:sequence><xs:any namespace="##targetNamespace" minOccurs="0" maxOccurs="unbounded" processContents="skip" /></xs:sequence><xs:attribute name="CellOrdinal" type="xs:unsignedInt" use="required" /></xs:complexType></xs:element></xs:sequence></xs:complexType><xs:element name="root"><xs:complexType><xs:sequence><xs:any namespace="http://www.w3.org/2001/XMLSchema" processContents="strict" minOccurs="0" /><xs:element name="OlapInfo" type="OlapInfo" minOccurs="0" /><xs:element name="Axes" type="Axes" minOccurs="0" /><xs:element name="CellData" type="CellData" minOccurs="0" /></xs:sequence></xs:complexType></xs:element></xs:schema>"###;

#[allow(clippy::too_many_arguments)]
fn build_cellset_root(
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    measure_name: Option<&str>,
    total_value: Option<&str>,
    leaf_members: &[(String, String)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    show_measures_axis: bool,
) -> String {
    let hier_uname = format!("[{}].[{}]", axis.table, axis.hier);
    let all_uname = format!("[{}].[{}].[All]", axis.table, axis.hier);
    let all_lname = format!("[{}].[{}].[(All)]", axis.table, axis.hier);
    let leaf_lname = format!("[{}].[{}].[{}]", axis.table, axis.hier, axis.level);

    // AxesInfo
    let hier_info = build_hier_info(&hier_uname, &axis.dim_props);
    let cell_info = build_cell_info(cell_props);

    let axis1_info = if show_measures_axis {
        match measure_name {
            Some(_) => concat!(
                r#"<ns2:AxisInfo name="Axis1">"#,
                r#"<ns2:HierarchyInfo name="[Measures]">"#,
                r#"<ns2:UName name="[Measures].[MEMBER_UNIQUE_NAME]" type="xs:string" />"#,
                r#"<ns2:Caption name="[Measures].[MEMBER_CAPTION]" type="xs:string" />"#,
                r#"<ns2:LName name="[Measures].[LEVEL_UNIQUE_NAME]" type="xs:string" />"#,
                r#"<ns2:LNum name="[Measures].[LEVEL_NUMBER]" type="xs:int" />"#,
                r#"<ns2:DisplayInfo name="[Measures].[DISPLAY_INFO]" type="xs:unsignedInt" />"#,
                r#"</ns2:HierarchyInfo></ns2:AxisInfo>"#,
            )
            .to_string(),
            None => String::new(),
        }
    } else {
        String::new()
    };

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{hier_info}</ns2:AxisInfo>"#,
            "{axis1_info}",
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        hier_info = hier_info,
        axis1_info = axis1_info,
        cell_info = cell_info,
    );

    // Axis0 tuples
    let mut tuples = String::new();

    if axis.include_all {
        let display_info: u32 = if axis.all_only { 1000 } else { 66536 };
        tuples.push_str(&build_all_member_tuple(
            &all_uname,
            &all_lname,
            &hier_uname,
            display_info,
            &axis.dim_props,
        ));
    }

    if !axis.all_only {
        let last = leaf_members.len().saturating_sub(1);
        for (i, (caption, _)) in leaf_members.iter().enumerate() {
            let display_info: u32 = if i == last { 131072 } else { 0 };
            tuples.push_str(&build_leaf_member_tuple(
                caption,
                &leaf_lname,
                &all_uname,
                &hier_uname,
                display_info,
                &axis.dim_props,
            ));
        }
    }

    let axis1_axis = if show_measures_axis {
        match measure_name {
            Some(mn) => format!(
                concat!(
                    r#"<ns2:Axis name="Axis1"><ns2:Tuples><ns2:Tuple><ns2:Member>"#,
                    "<ns2:UName>[Measures].[{mn}]</ns2:UName>",
                    "<ns2:Caption>{mn}</ns2:Caption>",
                    "<ns2:LName>[Measures].[MeasuresLevel]</ns2:LName>",
                    "<ns2:LNum>0</ns2:LNum>",
                    "<ns2:DisplayInfo>0</ns2:DisplayInfo>",
                    "</ns2:Member></ns2:Tuple></ns2:Tuples></ns2:Axis>",
                ),
                mn = xml_escape_value(mn),
            ),
            None => String::new(),
        }
    } else {
        String::new()
    };

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{tuples}</ns2:Tuples></ns2:Axis>"#,
            "{axis1_axis}",
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        tuples = tuples,
        axis1_axis = axis1_axis,
    );

    // CellData
    let cell_data = if measure_name.is_none() {
        "<ns2:CellData />".to_string()
    } else {
        // When no CELL PROPERTIES clause is present (cell_props is empty), default
        // to rendering VALUE — clients that omit the clause still expect values back.
        let has_value =
            cell_props.is_empty() || cell_props.iter().any(|p| p.eq_ignore_ascii_case("VALUE"));
        let has_fmt = cell_props.is_empty()
            || cell_props
                .iter()
                .any(|p| p.eq_ignore_ascii_case("FORMATTED_VALUE"));
        let mut cells = String::new();
        let mut ord = 0u32;

        if axis.include_all {
            cells.push_str(&build_cell(
                ord,
                total_value.unwrap_or(""),
                has_value,
                has_fmt,
            ));
            ord += 1;
        }
        if !axis.all_only {
            for (_, value) in leaf_members {
                cells.push_str(&build_cell(ord, value, has_value, has_fmt));
                ord += 1;
            }
        }
        format!("<ns2:CellData>{cells}</ns2:CellData>")
    };

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

fn build_hier_info(hier_uname: &str, dim_props: &[String]) -> String {
    let prop = |elem: &str, name: &str, typ: &str| {
        format!(
            r#"<ns2:{elem} name="{hier}.[{name}]" type="{typ}" />"#,
            hier = hier_uname
        )
    };

    let mut out = format!(r#"<ns2:HierarchyInfo name="{hier}">"#, hier = hier_uname);
    out.push_str(&prop("UName", "MEMBER_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("Caption", "MEMBER_CAPTION", "xs:string"));
    out.push_str(&prop("LName", "LEVEL_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("LNum", "LEVEL_NUMBER", "xs:int"));
    out.push_str(&prop("DisplayInfo", "DISPLAY_INFO", "xs:unsignedInt"));

    for dp in dim_props {
        match dp.to_uppercase().as_str() {
            "PARENT_UNIQUE_NAME" => out.push_str(&prop(
                "PARENT_UNIQUE_NAME",
                "PARENT_UNIQUE_NAME",
                "xs:string",
            )),
            "HIERARCHY_UNIQUE_NAME" => out.push_str(&prop(
                "HIERARCHY_UNIQUE_NAME",
                "HIERARCHY_UNIQUE_NAME",
                "xs:string",
            )),
            "MEMBER_TYPE" => out.push_str(&prop("MEMBER_TYPE", "MEMBER_TYPE", "xs:int")),
            _ => {}
        }
    }

    out.push_str("</ns2:HierarchyInfo>");
    out
}

fn build_cell_info(cell_props: &[String]) -> String {
    if cell_props.is_empty() {
        return concat!(
            r#"<ns2:Value name="VALUE" />"#,
            r#"<ns2:FmtValue name="FORMATTED_VALUE" type="xs:string" />"#,
            r#"<ns2:CellOrdinal name="CELL_ORDINAL" type="xs:unsignedInt" />"#,
        )
        .to_string();
    }
    let mut out = String::new();
    for p in cell_props {
        match p.to_uppercase().as_str() {
            "VALUE" => out.push_str(r#"<ns2:Value name="VALUE" />"#),
            "FORMATTED_VALUE" => {
                out.push_str(r#"<ns2:FmtValue name="FORMATTED_VALUE" type="xs:string" />"#)
            }
            "FORMAT_STRING" => {
                out.push_str(r#"<ns2:FormatString name="FORMAT_STRING" type="xs:string" />"#)
            }
            "LANGUAGE" => out.push_str(r#"<ns2:Language name="LANGUAGE" type="xs:unsignedInt" />"#),
            "BACK_COLOR" => {
                out.push_str(r#"<ns2:BackColor name="BACK_COLOR" type="xs:unsignedInt" />"#)
            }
            "FORE_COLOR" => {
                out.push_str(r#"<ns2:ForeColor name="FORE_COLOR" type="xs:unsignedInt" />"#)
            }
            "FONT_FLAGS" => out.push_str(r#"<ns2:FontFlags name="FONT_FLAGS" type="xs:int" />"#),
            "CELL_ORDINAL" => {
                out.push_str(r#"<ns2:CellOrdinal name="CELL_ORDINAL" type="xs:unsignedInt" />"#)
            }
            _ => {}
        }
    }
    out
}

fn build_all_member_tuple(
    all_uname: &str,
    all_lname: &str,
    hier_uname: &str,
    display_info: u32,
    dim_props: &[String],
) -> String {
    let mut member = format!(
        concat!(
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>All</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>0</ns2:LNum>",
            "<ns2:DisplayInfo>{di}</ns2:DisplayInfo>",
        ),
        uname = xml_escape_value(all_uname),
        lname = xml_escape_value(all_lname),
        di = display_info,
    );

    for dp in dim_props {
        match dp.to_uppercase().as_str() {
            "MEMBER_TYPE" => member.push_str("<ns2:MEMBER_TYPE>2</ns2:MEMBER_TYPE>"),
            "HIERARCHY_UNIQUE_NAME" => member.push_str(&format!(
                "<ns2:HIERARCHY_UNIQUE_NAME>{}</ns2:HIERARCHY_UNIQUE_NAME>",
                xml_escape_value(hier_uname)
            )),
            _ => {} // PARENT_UNIQUE_NAME is not emitted for the All member (it has no parent)
        }
    }

    format!("<ns2:Tuple><ns2:Member>{member}</ns2:Member></ns2:Tuple>")
}

fn build_leaf_member_tuple(
    caption: &str,
    leaf_lname: &str,
    all_uname: &str,
    hier_uname: &str,
    display_info: u32,
    dim_props: &[String],
) -> String {
    let cap_esc = xml_escape_value(caption);
    // The unique name uses &[key] syntax; & is encoded as &amp; in XML text.
    let uname = format!("{}.&amp;[{}]", hier_uname, cap_esc);

    let mut member = format!(
        concat!(
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>{cap}</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>1</ns2:LNum>",
            "<ns2:DisplayInfo>{di}</ns2:DisplayInfo>",
        ),
        uname = uname,
        cap = cap_esc,
        lname = xml_escape_value(leaf_lname),
        di = display_info,
    );

    for dp in dim_props {
        match dp.to_uppercase().as_str() {
            "PARENT_UNIQUE_NAME" => member.push_str(&format!(
                "<ns2:PARENT_UNIQUE_NAME>{}</ns2:PARENT_UNIQUE_NAME>",
                xml_escape_value(all_uname)
            )),
            "MEMBER_TYPE" => member.push_str("<ns2:MEMBER_TYPE>1</ns2:MEMBER_TYPE>"),
            "HIERARCHY_UNIQUE_NAME" => member.push_str(&format!(
                "<ns2:HIERARCHY_UNIQUE_NAME>{}</ns2:HIERARCHY_UNIQUE_NAME>",
                xml_escape_value(hier_uname)
            )),
            _ => {}
        }
    }

    format!("<ns2:Tuple><ns2:Member>{member}</ns2:Member></ns2:Tuple>")
}

fn build_cell(ordinal: u32, value: &str, has_value: bool, has_fmt: bool) -> String {
    let mut inner = String::new();
    if has_value {
        inner.push_str(&format!(
            "<ns2:Value>{}</ns2:Value>",
            xml_escape_value(value)
        ));
    }
    if has_fmt {
        inner.push_str(&format!(
            "<ns2:FmtValue>{}</ns2:FmtValue>",
            xml_escape_value(value)
        ));
    }
    format!(r#"<ns2:Cell CellOrdinal="{ordinal}">{inner}</ns2:Cell>"#)
}

// ── Two-hierarchy NormTupleSet response ───────────────────────────────────────

pub fn execute_mdx_cellset_two_hier(
    session_id: Option<&str>,
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    h1h2_cells: &[(String, String, Option<String>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_two_hier_root(
        cube_name,
        axis,
        h1h2_cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn cellset_envelope_two_hier(session_id: Option<&str>, inner: &str) -> String {
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

fn build_two_hier_root(
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    h1h2_cells: &[(String, String, Option<String>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let sh = axis
        .second_hier
        .as_ref()
        .expect("build_two_hier_root requires second_hier");
    let hier1_uname = format!("[{}].[{}]", axis.table, axis.hier);
    let hier2_uname = format!("[{}].[{}]", sh.table, sh.hier);

    let hier1_info = build_hier_info(&hier1_uname, &axis.dim_props);
    let hier2_info = build_hier_info(&hier2_uname, &axis.dim_props);
    let cell_info_str = build_cell_info(cell_props);

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{h1i}{h2i}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        h1i = hier1_info,
        h2i = hier2_info,
        ci = cell_info_str,
    );

    // Collect ordered unique H1 / H2 values and H2-per-H1 associations.
    let mut h1_ordered: Vec<String> = Vec::new();
    let mut h1_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut h2_ordered: Vec<String> = Vec::new();
    let mut h2_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut h2_for_h1: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    // Leaf value lookup: (h1, h2) → cell value string.
    let mut leaf_values: std::collections::HashMap<(String, String), String> =
        std::collections::HashMap::new();

    for (h1, h2, val) in h1h2_cells {
        if h1_seen.insert(h1.clone()) {
            h1_ordered.push(h1.clone());
        }
        if h2_seen.insert(h2.clone()) {
            h2_ordered.push(h2.clone());
        }
        h2_for_h1.entry(h1.clone()).or_default().push(h2.clone());
        if let Some(v) = val {
            leaf_values.insert((h1.clone(), h2.clone()), v.clone());
        }
    }

    let has_values = !leaf_values.is_empty();

    // Compute h1 subtotals and grand total for cells that carry numeric values.
    let mut h1_subtotals: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    for (h1, h2, _) in h1h2_cells {
        if let Some(v) = leaf_values.get(&(h1.clone(), h2.clone())) {
            if let Ok(n) = v.parse::<f64>() {
                *h1_subtotals.entry(h1.clone()).or_insert(0.0) += n;
            }
        }
    }
    let grand_total: f64 = h1_subtotals.values().sum();

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    // Map H2 value → ordinal in the MembersLookup (All=0, empty=1, data values from 2).
    let h2_ordinal = |val: &str| -> u32 {
        h2_ordered
            .iter()
            .position(|v| v == val)
            .map(|i| (i + 2) as u32)
            .unwrap_or(0)
    };

    // Build NormTuples and cell data in parallel (same iteration order).
    let mut norm_tuples = String::new();
    let mut cell_data = String::from("<ns2:CellData>");

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            ordinal = ordinal,
            v = xml_escape_value(value)
        )
    };

    // Fixed header rows (always present).
    norm_tuples.push_str(&build_norm_tuple(0, 66536, 0, 1000)); // All×All root
    norm_tuples.push_str(&build_norm_tuple(1, 0, 0, 197608)); // empty×All
    norm_tuples.push_str(&build_norm_tuple(1, 131072, 1, 0)); // empty×empty

    let grand_total_str = if has_values {
        fmt_num(grand_total)
    } else {
        "1".to_string()
    };
    cell_data.push_str(&cell_xml(0, &grand_total_str));
    cell_data.push_str(&cell_xml(1, ""));
    cell_data.push_str(&cell_xml(2, ""));

    let mut ordinal: u32 = 3;

    for h1_val in h1_ordered.iter() {
        let h1_ord = {
            h1_ordered
                .iter()
                .position(|v| v == h1_val)
                .map(|i| (i + 2) as u32)
                .unwrap_or(0)
        };
        let empty_vec: Vec<String> = Vec::new();
        let h2s = h2_for_h1.get(h1_val).unwrap_or(&empty_vec);
        let n = h2s.len();

        // Summary row: this H1 leaf with H2.All.
        norm_tuples.push_str(&build_norm_tuple(h1_ord, 131072, 0, 66536));
        let h1_sub = h1_subtotals.get(h1_val.as_str()).copied().unwrap_or(0.0);
        let h1_sub_str = if has_values {
            fmt_num(h1_sub)
        } else {
            "1".to_string()
        };
        cell_data.push_str(&cell_xml(ordinal, &h1_sub_str));
        ordinal += 1;

        for (j, h2_val) in h2s.iter().enumerate() {
            let h2_ord = h2_ordinal(h2_val);
            let h2_di: u32 = if n > 1 && j == n - 1 { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(h1_ord, 131072, h2_ord, h2_di));
            let leaf = leaf_values.get(&(h1_val.clone(), h2_val.clone()));
            let leaf_str = leaf
                .map(|s| s.as_str())
                .unwrap_or(if has_values { "" } else { "1" });
            cell_data.push_str(&cell_xml(ordinal, leaf_str));
            ordinal += 1;
        }
    }

    cell_data.push_str("</ns2:CellData>");

    // Build MembersLookup blocks.
    let h1_all_uname = format!("[{}].[{}].[All]", axis.table, axis.hier);
    let h1_all_lname = format!("[{}].[{}].[(All)]", axis.table, axis.hier);
    let h1_leaf_lname = format!("[{}].[{}].[{}]", axis.table, axis.hier, axis.level);
    let h2_all_uname = format!("[{}].[{}].[All]", sh.table, sh.hier);
    let h2_all_lname = format!("[{}].[{}].[(All)]", sh.table, sh.hier);
    let h2_leaf_lname = format!("[{}].[{}].[{}]", sh.table, sh.hier, sh.level);

    let members_h1 = build_norm_members_block(
        &hier1_uname,
        &h1_all_uname,
        &h1_all_lname,
        &h1_leaf_lname,
        &h1_ordered,
    );
    let members_h2 = build_norm_members_block(
        &hier2_uname,
        &h2_all_uname,
        &h2_all_lname,
        &h2_leaf_lname,
        &h2_ordered,
    );

    let norm_tuple_set = format!(
        concat!(
            "<ns5:NormTupleSet>",
            "<ns5:NormTuples>{tuples}</ns5:NormTuples>",
            "<ns5:MembersLookup>{m1}{m2}</ns5:MembersLookup>",
            "</ns5:NormTupleSet>",
        ),
        tuples = norm_tuples,
        m1 = members_h1,
        m2 = members_h2,
    );

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0">{nts}</ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        nts = norm_tuple_set,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

fn build_norm_tuple(h1_ord: u32, h1_di: u32, h2_ord: u32, h2_di: u32) -> String {
    format!(
        concat!(
            "<ns5:NormTuple>",
            "<ns5:MemberRef>",
            "<ns5:MemberOrdinal>{ho}</ns5:MemberOrdinal>",
            "<ns5:MemberDispInfo>{hd}</ns5:MemberDispInfo>",
            "</ns5:MemberRef>",
            "<ns5:MemberRef>",
            "<ns5:MemberOrdinal>{to}</ns5:MemberOrdinal>",
            "<ns5:MemberDispInfo>{td}</ns5:MemberDispInfo>",
            "</ns5:MemberRef>",
            "</ns5:NormTuple>",
        ),
        ho = h1_ord,
        hd = h1_di,
        to = h2_ord,
        td = h2_di,
    )
}

fn build_norm_tuple_3(
    h1_ord: u32,
    h1_di: u32,
    h2_ord: u32,
    h2_di: u32,
    h3_ord: u32,
    h3_di: u32,
) -> String {
    format!(
        concat!(
            "<ns5:NormTuple>",
            "<ns5:MemberRef><ns5:MemberOrdinal>{a}</ns5:MemberOrdinal><ns5:MemberDispInfo>{b}</ns5:MemberDispInfo></ns5:MemberRef>",
            "<ns5:MemberRef><ns5:MemberOrdinal>{c}</ns5:MemberOrdinal><ns5:MemberDispInfo>{d}</ns5:MemberDispInfo></ns5:MemberRef>",
            "<ns5:MemberRef><ns5:MemberOrdinal>{e}</ns5:MemberOrdinal><ns5:MemberDispInfo>{f}</ns5:MemberDispInfo></ns5:MemberRef>",
            "</ns5:NormTuple>",
        ),
        a = h1_ord, b = h1_di, c = h2_ord, d = h2_di, e = h3_ord, f = h3_di,
    )
}

fn build_norm_members_block_compact(
    hier_uname: &str,
    all_uname: &str,
    all_lname: &str,
    leaf_lname: &str,
    leaf_values: &[String],
) -> String {
    let mut members = String::new();

    members.push_str(&format!(
        concat!(
            "<ns2:Member>",
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>All</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>0</ns2:LNum>",
            "<ns2:HIERARCHY_UNIQUE_NAME>{hier}</ns2:HIERARCHY_UNIQUE_NAME>",
            "</ns2:Member>",
        ),
        uname = xml_escape_value(all_uname),
        lname = xml_escape_value(all_lname),
        hier = hier_uname,
    ));

    for val in leaf_values {
        let cap = xml_escape_value(val);
        members.push_str(&format!(
            concat!(
                "<ns2:Member>",
                "<ns2:UName>{hier}.&amp;[{cap}]</ns2:UName>",
                "<ns2:Caption>{cap}</ns2:Caption>",
                "<ns2:LName>{lname}</ns2:LName>",
                "<ns2:LNum>1</ns2:LNum>",
                "<ns2:PARENT_UNIQUE_NAME>{all}</ns2:PARENT_UNIQUE_NAME>",
                "<ns2:HIERARCHY_UNIQUE_NAME>{hier}</ns2:HIERARCHY_UNIQUE_NAME>",
                "</ns2:Member>",
            ),
            hier = hier_uname,
            cap = cap,
            lname = xml_escape_value(leaf_lname),
            all = xml_escape_value(all_uname),
        ));
    }

    format!("<ns2:Members>{}</ns2:Members>", members)
}

fn build_norm_members_block(
    hier_uname: &str,
    all_uname: &str,
    all_lname: &str,
    leaf_lname: &str,
    leaf_values: &[String],
) -> String {
    let mut members = String::new();

    // All member (level 0, no parent).
    members.push_str(&format!(
        concat!(
            "<ns2:Member>",
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>All</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>0</ns2:LNum>",
            "<ns2:HIERARCHY_UNIQUE_NAME>{hier}</ns2:HIERARCHY_UNIQUE_NAME>",
            "</ns2:Member>",
        ),
        uname = xml_escape_value(all_uname),
        lname = xml_escape_value(all_lname),
        hier = hier_uname,
    ));

    // Empty placeholder at level 1 (UName ends with bare `&`).
    members.push_str(&format!(
        concat!(
            "<ns2:Member>",
            "<ns2:UName>{hier}.&amp;</ns2:UName>",
            "<ns2:Caption />",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>1</ns2:LNum>",
            "<ns2:PARENT_UNIQUE_NAME>{all}</ns2:PARENT_UNIQUE_NAME>",
            "<ns2:HIERARCHY_UNIQUE_NAME>{hier}</ns2:HIERARCHY_UNIQUE_NAME>",
            "</ns2:Member>",
        ),
        hier = hier_uname,
        lname = xml_escape_value(leaf_lname),
        all = xml_escape_value(all_uname),
    ));

    // Actual leaf members.
    for val in leaf_values {
        let cap = xml_escape_value(val);
        members.push_str(&format!(
            concat!(
                "<ns2:Member>",
                "<ns2:UName>{hier}.&amp;[{cap}]</ns2:UName>",
                "<ns2:Caption>{cap}</ns2:Caption>",
                "<ns2:LName>{lname}</ns2:LName>",
                "<ns2:LNum>1</ns2:LNum>",
                "<ns2:PARENT_UNIQUE_NAME>{all}</ns2:PARENT_UNIQUE_NAME>",
                "<ns2:HIERARCHY_UNIQUE_NAME>{hier}</ns2:HIERARCHY_UNIQUE_NAME>",
                "</ns2:Member>",
            ),
            hier = hier_uname,
            cap = cap,
            lname = xml_escape_value(leaf_lname),
            all = xml_escape_value(all_uname),
        ));
    }

    format!("<ns2:Members>{}</ns2:Members>", members)
}

// ── Two-independent-dim-axis response ─────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_two_dim_axis(
    session_id: Option<&str>,
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    pairs: &[(String, String)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_two_dim_axis_root(
        cube_name,
        col_axis,
        row_axis,
        pairs,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_empty_placeholder_tuple(
    hier_uname: &str,
    leaf_lname: &str,
    all_uname: &str,
    dim_props: &[String],
) -> String {
    let mut member = format!(
        concat!(
            "<ns2:UName>{hier}.&amp;</ns2:UName>",
            "<ns2:Caption />",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>1</ns2:LNum>",
            "<ns2:DisplayInfo>0</ns2:DisplayInfo>",
        ),
        hier = hier_uname,
        lname = xml_escape_value(leaf_lname),
    );
    for dp in dim_props {
        match dp.to_uppercase().as_str() {
            "PARENT_UNIQUE_NAME" => member.push_str(&format!(
                "<ns2:PARENT_UNIQUE_NAME>{}</ns2:PARENT_UNIQUE_NAME>",
                xml_escape_value(all_uname)
            )),
            "MEMBER_TYPE" => member.push_str("<ns2:MEMBER_TYPE>1</ns2:MEMBER_TYPE>"),
            "HIERARCHY_UNIQUE_NAME" => member.push_str(&format!(
                "<ns2:HIERARCHY_UNIQUE_NAME>{}</ns2:HIERARCHY_UNIQUE_NAME>",
                xml_escape_value(hier_uname)
            )),
            _ => {}
        }
    }
    format!("<ns2:Tuple><ns2:Member>{member}</ns2:Member></ns2:Tuple>")
}

fn build_two_dim_axis_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    pairs: &[(String, String)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let col_hier_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);
    let row_hier_uname = format!("[{}].[{}]", row_axis.table, row_axis.hier);

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{c}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{r}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        c = build_hier_info(&col_hier_uname, &col_axis.dim_props),
        r = build_hier_info(&row_hier_uname, &row_axis.dim_props),
        ci = build_cell_info(cell_props),
    );

    // Collect ordered distinct leaf values and the cross-product combo set.
    let mut col_leaves: Vec<String> = Vec::new();
    let mut col_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut row_leaves: Vec<String> = Vec::new();
    let mut row_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut combo_set: std::collections::HashSet<(String, String)> =
        std::collections::HashSet::new();

    for (c, r) in pairs {
        if col_seen.insert(c.clone()) {
            col_leaves.push(c.clone());
        }
        if row_seen.insert(r.clone()) {
            row_leaves.push(r.clone());
        }
        combo_set.insert((c.clone(), r.clone()));
    }

    let n_axis0 = (2 + col_leaves.len()) as u32; // All + empty + leaves
    let has_data = !pairs.is_empty();

    // Build Axis0 tuples: All, empty placeholder, then all leaves (each DI=131072).
    let col_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let col_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let col_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let mut axis0_tuples = String::new();
    axis0_tuples.push_str(&build_all_member_tuple(
        &col_all_uname,
        &col_all_lname,
        &col_hier_uname,
        66536,
        &col_axis.dim_props,
    ));
    axis0_tuples.push_str(&build_empty_placeholder_tuple(
        &col_hier_uname,
        &col_leaf_lname,
        &col_all_uname,
        &col_axis.dim_props,
    ));
    for cap in &col_leaves {
        axis0_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &col_leaf_lname,
            &col_all_uname,
            &col_hier_uname,
            131072,
            &col_axis.dim_props,
        ));
    }

    // Build Axis1 tuples: same structure.
    let row_all_uname = format!("[{}].[{}].[All]", row_axis.table, row_axis.hier);
    let row_all_lname = format!("[{}].[{}].[(All)]", row_axis.table, row_axis.hier);
    let row_leaf_lname = format!(
        "[{}].[{}].[{}]",
        row_axis.table, row_axis.hier, row_axis.level
    );
    let mut axis1_tuples = String::new();
    axis1_tuples.push_str(&build_all_member_tuple(
        &row_all_uname,
        &row_all_lname,
        &row_hier_uname,
        66536,
        &row_axis.dim_props,
    ));
    axis1_tuples.push_str(&build_empty_placeholder_tuple(
        &row_hier_uname,
        &row_leaf_lname,
        &row_all_uname,
        &row_axis.dim_props,
    ));
    for cap in &row_leaves {
        axis1_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &row_leaf_lname,
            &row_all_uname,
            &row_hier_uname,
            131072,
            &row_axis.dim_props,
        ));
    }

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="Axis1"><ns2:Tuples>{a1}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        a0 = axis0_tuples,
        a1 = axis1_tuples,
    );

    // CellData: ordinal = col_pos + n_axis0 * row_pos
    // Rules:
    //   row=All (0): emit all col positions (All, empty, every leaf)
    //   row=empty (1): emit only col=All(0) and col=empty(1); skip col=leaf
    //   row=leaf_j: emit col=All(0); skip col=empty(1); emit col=leaf_i iff combo exists
    let mut cells = String::new();
    let cell_v1 = |ord: u32| -> String {
        format!(r#"<ns2:Cell CellOrdinal="{ord}"><ns2:Value>1</ns2:Value></ns2:Cell>"#)
    };

    if has_data {
        // row = All (row_pos = 0): all col positions get value=1
        for col_pos in 0..n_axis0 {
            cells.push_str(&cell_v1(col_pos));
        }
        // row = empty (row_pos = 1): only All and empty columns
        cells.push_str(&cell_v1(n_axis0)); // col=All × row=empty
        cells.push_str(&cell_v1(1 + n_axis0)); // col=empty × row=empty

        // row = each leaf j (row_pos = 2+j):
        for (j, row_val) in row_leaves.iter().enumerate() {
            let row_pos = (2 + j) as u32;
            let base = row_pos * n_axis0;
            cells.push_str(&cell_v1(base)); // col=All × row=leaf_j
                                            // col=empty: skip
            for (i, col_val) in col_leaves.iter().enumerate() {
                if combo_set.contains(&(col_val.clone(), row_val.clone())) {
                    cells.push_str(&cell_v1(base + (2 + i as u32)));
                }
            }
        }
    }

    let cell_data = format!("<ns2:CellData>{cells}</ns2:CellData>");

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Two-dim-axis with single measure ─────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_two_dim_axis_measure(
    session_id: Option<&str>,
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    cells: &[(String, String, Option<String>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_two_dim_axis_measure_root(
        cube_name,
        col_axis,
        row_axis,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_two_dim_axis_measure_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    cells: &[(String, String, Option<String>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let col_hier_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);
    let row_hier_uname = format!("[{}].[{}]", row_axis.table, row_axis.hier);

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{c}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{r}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        c = build_hier_info(&col_hier_uname, &col_axis.dim_props),
        r = build_hier_info(&row_hier_uname, &row_axis.dim_props),
        ci = build_cell_info(cell_props),
    );

    let mut col_leaves: Vec<String> = Vec::new();
    let mut col_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut row_leaves: Vec<String> = Vec::new();
    let mut row_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut cell_map: std::collections::HashMap<(String, String), f64> =
        std::collections::HashMap::new();

    for (c, r, v) in cells {
        if col_seen.insert(c.clone()) {
            col_leaves.push(c.clone());
        }
        if row_seen.insert(r.clone()) {
            row_leaves.push(r.clone());
        }
        if let Some(s) = v.as_deref() {
            if let Ok(n) = s.parse::<f64>() {
                cell_map.insert((c.clone(), r.clone()), n);
            }
        }
    }

    let n_col_leaves = col_leaves.len();
    let n_row_leaves = row_leaves.len();

    let mut col_totals: Vec<f64> = vec![0.0; n_col_leaves];
    let mut row_totals: Vec<f64> = vec![0.0; n_row_leaves];
    for ((c, r), val) in &cell_map {
        if let Some(ci) = col_leaves.iter().position(|x| x == c) {
            col_totals[ci] += val;
        }
        if let Some(ri) = row_leaves.iter().position(|x| x == r) {
            row_totals[ri] += val;
        }
    }
    let grand_total: f64 = col_totals.iter().sum();

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    let n_axis0 = (1 + n_col_leaves) as u32;

    let col_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let col_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let col_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let mut axis0_tuples = String::new();
    axis0_tuples.push_str(&build_all_member_tuple(
        &col_all_uname,
        &col_all_lname,
        &col_hier_uname,
        66536,
        &col_axis.dim_props,
    ));
    for (i, cap) in col_leaves.iter().enumerate() {
        let di = if i == n_col_leaves - 1 { 131072 } else { 0 };
        axis0_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &col_leaf_lname,
            &col_all_uname,
            &col_hier_uname,
            di,
            &col_axis.dim_props,
        ));
    }

    let row_all_uname = format!("[{}].[{}].[All]", row_axis.table, row_axis.hier);
    let row_all_lname = format!("[{}].[{}].[(All)]", row_axis.table, row_axis.hier);
    let row_leaf_lname = format!(
        "[{}].[{}].[{}]",
        row_axis.table, row_axis.hier, row_axis.level
    );
    let mut axis1_tuples = String::new();
    axis1_tuples.push_str(&build_all_member_tuple(
        &row_all_uname,
        &row_all_lname,
        &row_hier_uname,
        66536,
        &row_axis.dim_props,
    ));
    for (j, cap) in row_leaves.iter().enumerate() {
        let di = if j == n_row_leaves - 1 { 131072 } else { 0 };
        axis1_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &row_leaf_lname,
            &row_all_uname,
            &row_hier_uname,
            di,
            &row_axis.dim_props,
        ));
    }

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="Axis1"><ns2:Tuples>{a1}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        a0 = axis0_tuples,
        a1 = axis1_tuples,
    );

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    let mut cell_data = String::from("<ns2:CellData>");

    // row = All (row_pos=0): grand total then col totals.
    cell_data.push_str(&cell_xml(0, &fmt_num(grand_total)));
    for (i, ct) in col_totals.iter().enumerate() {
        cell_data.push_str(&cell_xml(1 + i as u32, &fmt_num(*ct)));
    }

    // row = leaf_j (row_pos = 1+j).
    for (j, row_val) in row_leaves.iter().enumerate() {
        let base = (1 + j) as u32 * n_axis0;
        cell_data.push_str(&cell_xml(base, &fmt_num(row_totals[j])));
        for (i, col_val) in col_leaves.iter().enumerate() {
            if let Some(val) = cell_map.get(&(col_val.clone(), row_val.clone())) {
                cell_data.push_str(&cell_xml(base + 1 + i as u32, &fmt_num(*val)));
            }
        }
    }
    cell_data.push_str("</ns2:CellData>");

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── CrossJoin(dim, measures) ON COLUMNS + dim ON ROWS col-matrix ──────────────

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_col_matrix(
    session_id: Option<&str>,
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    measures_first: bool,
    matrix_on_rows: bool,
) -> (String, Response) {
    let inner = build_col_matrix_root(
        cube_name,
        col_axis,
        row_axis,
        matrix_measures,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
        measures_first,
        matrix_on_rows,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_measures_norm_members(matrix_measures: &[(String, String)]) -> String {
    let mut members = String::new();
    for (name, _) in matrix_measures {
        let cap = xml_escape_value(name);
        members.push_str(&format!(
            concat!(
                "<ns2:Member>",
                "<ns2:UName>[Measures].[{uname}]</ns2:UName>",
                "<ns2:Caption>{cap}</ns2:Caption>",
                "<ns2:LName>[Measures].[MeasuresLevel]</ns2:LName>",
                "<ns2:LNum>0</ns2:LNum>",
                "<ns2:HIERARCHY_UNIQUE_NAME>[Measures]</ns2:HIERARCHY_UNIQUE_NAME>",
                "</ns2:Member>",
            ),
            uname = cap,
            cap = cap,
        ));
    }
    format!("<ns2:Members>{}</ns2:Members>", members)
}

#[allow(clippy::too_many_arguments)]
fn build_col_matrix_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    row_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    measures_first: bool,
    matrix_on_rows: bool,
) -> String {
    let n_measures = matrix_measures.len();
    let col_hier_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);
    let row_hier_uname = format!("[{}].[{}]", row_axis.table, row_axis.hier);

    let col_hi = build_hier_info(&col_hier_uname, &col_axis.dim_props);
    let meas_hi = build_measures_hier_info();
    let crossjoin_hier_infos = if measures_first {
        format!("{meas_hi}{col_hi}")
    } else {
        format!("{col_hi}{meas_hi}")
    };
    let simple_hier_info = build_hier_info(&row_hier_uname, &row_axis.dim_props);

    let (a0_hier, a1_hier) = if matrix_on_rows {
        (simple_hier_info, crossjoin_hier_infos)
    } else {
        (crossjoin_hier_infos, simple_hier_info)
    };

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{a1}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = a0_hier,
        a1 = a1_hier,
        cell_info = build_cell_info(cell_props),
    );

    let mut col_leaves: Vec<String> = Vec::new();
    let mut col_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut row_leaves: Vec<String> = Vec::new();
    let mut row_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut leaf_vals: std::collections::HashMap<(String, String), Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (c, r, vals) in cells {
        if col_seen.insert(c.clone()) {
            col_leaves.push(c.clone());
        }
        if row_seen.insert(r.clone()) {
            row_leaves.push(r.clone());
        }
        leaf_vals.insert((c.clone(), r.clone()), vals.clone());
    }

    let n_col_leaves = col_leaves.len();
    let n_col_members = 1 + n_col_leaves;

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    // Per-measure column subtotals and row subtotals.
    let mut col_subtotals: Vec<Vec<f64>> = vec![vec![0.0; n_measures]; n_col_leaves];
    let mut row_subtotals: Vec<Vec<f64>> = vec![vec![0.0; n_measures]; row_leaves.len()];
    for (c, r, vals) in cells {
        let ci = col_leaves.iter().position(|x| x == c);
        let ri = row_leaves.iter().position(|x| x == r);
        for (m, v) in vals.iter().enumerate().take(n_measures) {
            if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                if let Some(ci) = ci {
                    col_subtotals[ci][m] += n;
                }
                if let Some(ri) = ri {
                    row_subtotals[ri][m] += n;
                }
            }
        }
    }
    let grand_totals: Vec<f64> = (0..n_measures)
        .map(|m| {
            col_subtotals
                .iter()
                .map(|s| s.get(m).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();
    let has_values = !cells.is_empty();

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    // NormTuples: H1/H2 identity and iteration order depend on measures_first.
    //
    // measures_first=true  → H1=measures (outer), H2=col_dim (inner)
    //   H1 DI: 0 for (not-last-m, All col); 131072 otherwise
    //   H2 DI: 66536 for All; 0 for non-last leaf; 131072 for last leaf
    //   axis0_pos = m * n_col_members + col_idx
    //
    // measures_first=false → H1=col_dim (outer), H2=measures (inner)
    //   H1 DI: 1000/(All,not-last-m) 197608/(All,last-m) 0/(leaf,neither-last) 131072/(leaf,either-last)
    //   H2 DI: 0/(not-last-m) 131072/(last-m or leaf)
    //   axis0_pos = col_idx * n_measures + m
    let mut norm_tuples = String::new();

    if measures_first {
        for m in 0..n_measures {
            let is_last_m = m == n_measures - 1;
            let h1_di_all: u32 = if is_last_m { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(m as u32, h1_di_all, 0, 66536));
            for (ci, _) in col_leaves.iter().enumerate() {
                let is_last_col = ci == n_col_leaves - 1;
                let h2_di: u32 = if is_last_col { 131072 } else { 0 };
                norm_tuples.push_str(&build_norm_tuple(m as u32, 131072, (ci + 1) as u32, h2_di));
            }
        }
    } else {
        for m in 0..n_measures {
            let is_last_m = m == n_measures - 1;
            let h1_di: u32 = if is_last_m { 197608 } else { 1000 };
            let h2_di: u32 = if is_last_m { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(0, h1_di, m as u32, h2_di));
        }
        for (ci, _) in col_leaves.iter().enumerate() {
            let c_ord = (ci + 1) as u32;
            let is_last_col = ci == n_col_leaves - 1;
            for m in 0..n_measures {
                let is_last_m = m == n_measures - 1;
                let h1_di: u32 = if is_last_col || is_last_m { 131072 } else { 0 };
                norm_tuples.push_str(&build_norm_tuple(c_ord, h1_di, m as u32, 131072));
            }
        }
    }

    // MembersLookup: H1/H2 order matches the HierarchyInfo order.
    let col_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let col_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let col_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let members_col = build_norm_members_block_compact(
        &col_hier_uname,
        &col_all_uname,
        &col_all_lname,
        &col_leaf_lname,
        &col_leaves,
    );
    let members_measures = build_measures_norm_members(matrix_measures);
    let members_lookup = if measures_first {
        format!("<ns5:MembersLookup>{members_measures}{members_col}</ns5:MembersLookup>")
    } else {
        format!("<ns5:MembersLookup>{members_col}{members_measures}</ns5:MembersLookup>")
    };

    let norm_tuple_set = format!(
        concat!(
            "<ns5:NormTupleSet>",
            "<ns5:NormTuples>{tuples}</ns5:NormTuples>",
            "{ml}",
            "</ns5:NormTupleSet>",
        ),
        tuples = norm_tuples,
        ml = members_lookup,
    );

    // Simple Tuples for row_axis (used as Axis1 when matrix_on_rows=false, Axis0 when true).
    let n_row_leaves = row_leaves.len();
    let row_all_uname = format!("[{}].[{}].[All]", row_axis.table, row_axis.hier);
    let row_all_lname = format!("[{}].[{}].[(All)]", row_axis.table, row_axis.hier);
    let row_leaf_lname = format!(
        "[{}].[{}].[{}]",
        row_axis.table, row_axis.hier, row_axis.level
    );
    let mut simple_tuples = String::new();
    simple_tuples.push_str(&build_all_member_tuple(
        &row_all_uname,
        &row_all_lname,
        &row_hier_uname,
        66536,
        &row_axis.dim_props,
    ));
    for (j, cap) in row_leaves.iter().enumerate() {
        let di = if j == n_row_leaves - 1 { 131072 } else { 0 };
        simple_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &row_leaf_lname,
            &row_all_uname,
            &row_hier_uname,
            di,
            &row_axis.dim_props,
        ));
    }

    let cell_data;
    let axes;

    if matrix_on_rows {
        // CrossJoin on ROWS: Axis0 = simple dim (row_axis), Axis1 = NormTupleSet.
        // cell ordinal = simple_idx + n_axis0_members * axis1_pos(col_idx, m)
        let n_axis0_members = (1 + n_row_leaves) as u32;
        let axis1_pos = |col_idx: usize, m: usize| -> u32 {
            if measures_first {
                (m * n_col_members + col_idx) as u32
            } else {
                (col_idx * n_measures + m) as u32
            }
        };

        let mut cd = String::from("<ns2:CellData>");

        // simple_idx = 0 (All): grand totals and col subtotals.
        for (m, gt) in grand_totals.iter().enumerate() {
            let val = if has_values {
                fmt_num(*gt)
            } else {
                "1".to_string()
            };
            cd.push_str(&cell_xml(n_axis0_members * axis1_pos(0, m), &val));
        }
        for (ci, _) in col_leaves.iter().enumerate() {
            for (m, ct) in col_subtotals[ci].iter().enumerate() {
                let val = if has_values {
                    fmt_num(*ct)
                } else {
                    "1".to_string()
                };
                cd.push_str(&cell_xml(n_axis0_members * axis1_pos(ci + 1, m), &val));
            }
        }

        // simple_idx = ri+1 (simple leaf): row subtotals then leaf cells.
        for (ri, row_val) in row_leaves.iter().enumerate() {
            let si = (ri + 1) as u32;
            for (m, rt) in row_subtotals[ri].iter().enumerate() {
                let val = if has_values {
                    fmt_num(*rt)
                } else {
                    "1".to_string()
                };
                cd.push_str(&cell_xml(si + n_axis0_members * axis1_pos(0, m), &val));
            }
            for (ci, col_val) in col_leaves.iter().enumerate() {
                if let Some(vals) = leaf_vals.get(&(col_val.clone(), row_val.clone())) {
                    for m in 0..n_measures {
                        let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                        if !v.is_empty() {
                            cd.push_str(&cell_xml(si + n_axis0_members * axis1_pos(ci + 1, m), v));
                        }
                    }
                }
            }
        }
        cd.push_str("</ns2:CellData>");
        cell_data = cd;

        axes = format!(
            concat!(
                "<ns2:Axes>",
                r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
                r#"<ns2:Axis name="Axis1">{nts}</ns2:Axis>"#,
                r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
                "</ns2:Axes>",
            ),
            a0 = simple_tuples,
            nts = norm_tuple_set,
        );
    } else {
        // CrossJoin on COLUMNS: Axis0 = NormTupleSet, Axis1 = simple dim (row_axis).
        // cell ordinal = axis0_pos(col_idx, m) + n_axis0_tuples * row_idx
        let n_axis0_tuples = (n_col_members * n_measures) as u32;
        let axis0_pos = |col_idx: usize, m: usize| -> u32 {
            if measures_first {
                (m * n_col_members + col_idx) as u32
            } else {
                (col_idx * n_measures + m) as u32
            }
        };

        let mut cd = String::from("<ns2:CellData>");

        // axis1_pos = 0 (All row): grand totals then col subtotals.
        for (m, gt) in grand_totals.iter().enumerate() {
            let val = if has_values {
                fmt_num(*gt)
            } else {
                "1".to_string()
            };
            cd.push_str(&cell_xml(axis0_pos(0, m), &val));
        }
        for (ci, _) in col_leaves.iter().enumerate() {
            for (m, ct) in col_subtotals[ci].iter().enumerate() {
                let val = if has_values {
                    fmt_num(*ct)
                } else {
                    "1".to_string()
                };
                cd.push_str(&cell_xml(axis0_pos(ci + 1, m), &val));
            }
        }

        // axis1_pos = ri+1 for each row leaf.
        for (ri, row_val) in row_leaves.iter().enumerate() {
            let base = (ri + 1) as u32 * n_axis0_tuples;
            for (m, rt) in row_subtotals[ri].iter().enumerate() {
                let val = if has_values {
                    fmt_num(*rt)
                } else {
                    "1".to_string()
                };
                cd.push_str(&cell_xml(base + axis0_pos(0, m), &val));
            }
            for (ci, col_val) in col_leaves.iter().enumerate() {
                if let Some(vals) = leaf_vals.get(&(col_val.clone(), row_val.clone())) {
                    for m in 0..n_measures {
                        let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                        if !v.is_empty() {
                            cd.push_str(&cell_xml(base + axis0_pos(ci + 1, m), v));
                        }
                    }
                }
            }
        }
        cd.push_str("</ns2:CellData>");
        cell_data = cd;

        axes = format!(
            concat!(
                "<ns2:Axes>",
                r#"<ns2:Axis name="Axis0">{nts}</ns2:Axis>"#,
                r#"<ns2:Axis name="Axis1"><ns2:Tuples>{a1}</ns2:Tuples></ns2:Axis>"#,
                r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
                "</ns2:Axes>",
            ),
            nts = norm_tuple_set,
            a1 = simple_tuples,
        );
    }

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Scalar response ───────────────────────────────────────────────────────────
//
// Pattern: SELECT FROM [Model] WHERE ([Measures].[M])
// No axes at all. SlicerAxis only. Single cell at ordinal 0.

pub fn execute_mdx_scalar(
    session_id: Option<&str>,
    cube_name: &str,
    value: Option<&str>,
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_scalar_root(
        cube_name,
        value,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_scalar_root(
    cube_name: &str,
    value: Option<&str>,
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        cell_info = build_cell_info(cell_props),
    );

    let slicer = r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#;

    let val_str = value.unwrap_or("0");
    let cell_data = format!(
        r#"<ns2:CellData><ns2:Cell CellOrdinal="0"><ns2:Value>{v}</ns2:Value></ns2:Cell></ns2:CellData>"#,
        v = xml_escape_value(val_str),
    );

    let axes = format!("<ns2:Axes>{slicer}</ns2:Axes>", slicer = slicer,);

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Measures-only-COLUMNS response ────────────────────────────────────────────
//
// Pattern: SELECT {measures} ON COLUMNS FROM [Model]
// Axis0 = one Tuple per measure. No Axis1. SlicerAxis.
// cell ordinal = measure index (0-based).

pub fn execute_mdx_meas_only_cols(
    session_id: Option<&str>,
    cube_name: &str,
    matrix_measures: &[(String, String)],
    values: &[Option<String>],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_meas_only_cols_root(
        cube_name,
        matrix_measures,
        values,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_meas_only_cols_root(
    cube_name: &str,
    matrix_measures: &[(String, String)],
    values: &[Option<String>],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let n_measures = matrix_measures.len();
    let meas_hi = build_measures_hier_info();

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = meas_hi,
        cell_info = build_cell_info(cell_props),
    );

    // Axis0: one Tuple per measure.
    let mut axis0_tuples = String::new();
    for (i, (name, _)) in matrix_measures.iter().enumerate() {
        let di = if i == n_measures - 1 { 131072 } else { 0 };
        axis0_tuples.push_str(&build_measure_tuple(name, di));
    }

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        a0 = axis0_tuples,
    );

    // cell ordinal = measure index
    let mut cell_data = String::from("<ns2:CellData>");
    for (i, (_, _)) in matrix_measures.iter().enumerate() {
        let v = values.get(i).and_then(|x| x.as_deref()).unwrap_or("0");
        if !v.is_empty() {
            cell_data.push_str(&format!(
                r#"<ns2:Cell CellOrdinal="{i}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
                v = xml_escape_value(v),
            ));
        }
    }
    cell_data.push_str("</ns2:CellData>");

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Single-axis CrossJoin response ────────────────────────────────────────────
//
// Pattern: CrossJoin(dim, measures) on a single axis (ON COLUMNS or ON ROWS),
// with no second dim axis.
// Axis0 = NormTupleSet (dim + measures CrossJoin). No Axis1. SlicerAxis.
// cell ordinal (measures_first=false): dim_idx * n_measures + meas_idx
// cell ordinal (measures_first=true):  meas_idx * n_dim_members + dim_idx

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn execute_mdx_cellset_single_axis_crossjoin(
    session_id: Option<&str>,
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Option<String>, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    measures_first: bool,
) -> (String, Response) {
    let inner = build_single_axis_crossjoin_root(
        cube_name,
        col_axis,
        matrix_measures,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
        measures_first,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn build_single_axis_crossjoin_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Option<String>, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    measures_first: bool,
) -> String {
    if col_axis.second_hier.is_some() {
        return build_single_axis_two_hier_crossjoin_root(
            cube_name,
            col_axis,
            matrix_measures,
            cells,
            cell_props,
            last_data_update,
            last_schema_update,
            measures_first,
        );
    }
    let n_measures = matrix_measures.len();
    let col_hier_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);

    let col_hi = build_hier_info(&col_hier_uname, &col_axis.dim_props);
    let meas_hi = build_measures_hier_info();
    let axis0_hier = if measures_first {
        format!("{meas_hi}{col_hi}")
    } else {
        format!("{col_hi}{meas_hi}")
    };

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = axis0_hier,
        cell_info = build_cell_info(cell_props),
    );

    // Collect distinct dim leaves in encounter order.
    let mut dim_leaves: Vec<String> = Vec::new();
    let mut dim_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut leaf_vals: std::collections::HashMap<String, Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (leaf, _h2, vals) in cells {
        if dim_seen.insert(leaf.clone()) {
            dim_leaves.push(leaf.clone());
        }
        leaf_vals.insert(leaf.clone(), vals.clone());
    }

    let n_dim_leaves = dim_leaves.len();
    let n_dim_members = 1 + n_dim_leaves; // All + leaves

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    // Per-measure grand totals (sum of leaf values).
    let mut grand_totals: Vec<f64> = vec![0.0; n_measures];
    for vals in leaf_vals.values() {
        for (m, v) in vals.iter().enumerate().take(n_measures) {
            if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                grand_totals[m] += n;
            }
        }
    }
    let has_values = !cells.is_empty();

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    // NormTuples — same display-info logic as build_col_matrix_root Axis0.
    let mut norm_tuples = String::new();

    if measures_first {
        for m in 0..n_measures {
            let is_last_m = m == n_measures - 1;
            let h1_di: u32 = if is_last_m { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(m as u32, h1_di, 0, 66536));
            for (di, _) in dim_leaves.iter().enumerate() {
                let is_last_dim = di == n_dim_leaves - 1;
                let h2_di: u32 = if is_last_dim { 131072 } else { 0 };
                norm_tuples.push_str(&build_norm_tuple(m as u32, 131072, (di + 1) as u32, h2_di));
            }
        }
    } else {
        for m in 0..n_measures {
            let is_last_m = m == n_measures - 1;
            let h1_di: u32 = if is_last_m { 197608 } else { 1000 };
            let h2_di: u32 = if is_last_m { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(0, h1_di, m as u32, h2_di));
        }
        for (di, _) in dim_leaves.iter().enumerate() {
            let d_ord = (di + 1) as u32;
            let is_last_dim = di == n_dim_leaves - 1;
            for m in 0..n_measures {
                let is_last_m = m == n_measures - 1;
                let h1_di: u32 = if is_last_dim || is_last_m { 131072 } else { 0 };
                norm_tuples.push_str(&build_norm_tuple(d_ord, h1_di, m as u32, 131072));
            }
        }
    }

    // MembersLookup.
    let col_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let col_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let col_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let members_col = build_norm_members_block_compact(
        &col_hier_uname,
        &col_all_uname,
        &col_all_lname,
        &col_leaf_lname,
        &dim_leaves,
    );
    let members_measures = build_measures_norm_members(matrix_measures);
    let members_lookup = if measures_first {
        format!("<ns5:MembersLookup>{members_measures}{members_col}</ns5:MembersLookup>")
    } else {
        format!("<ns5:MembersLookup>{members_col}{members_measures}</ns5:MembersLookup>")
    };

    let norm_tuple_set = format!(
        concat!(
            "<ns5:NormTupleSet>",
            "<ns5:NormTuples>{tuples}</ns5:NormTuples>",
            "{ml}",
            "</ns5:NormTupleSet>",
        ),
        tuples = norm_tuples,
        ml = members_lookup,
    );

    // Cell ordinal functions.
    let axis0_pos = |dim_idx: usize, m: usize| -> u32 {
        if measures_first {
            (m * n_dim_members + dim_idx) as u32
        } else {
            (dim_idx * n_measures + m) as u32
        }
    };

    // CellData: All-row grand totals first, then per-leaf values.
    let mut cd = String::from("<ns2:CellData>");

    for (m, gt) in grand_totals.iter().enumerate() {
        let val = if has_values {
            fmt_num(*gt)
        } else {
            "1".to_string()
        };
        cd.push_str(&cell_xml(axis0_pos(0, m), &val));
    }
    for (di, leaf) in dim_leaves.iter().enumerate() {
        if let Some(vals) = leaf_vals.get(leaf) {
            for m in 0..n_measures {
                let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                if !v.is_empty() {
                    cd.push_str(&cell_xml(axis0_pos(di + 1, m), v));
                }
            }
        }
    }
    cd.push_str("</ns2:CellData>");

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0">{nts}</ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        nts = norm_tuple_set,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cd,
    )
}

// ── Single-axis CrossJoin with two-hierarchy dim ──────────────────────────────
//
// CrossJoin(Hierarchize(DrilldownMember(CrossJoin(H1, H2), ...)), {measures}) ON COLUMNS.
// All three hierarchies appear on Axis0. CellData rows:
//   grand-total (All×All) × each measure
//   for each H1 leaf: (H1×All) subtotal × each measure, then (H1×H2) per leaf × each measure.

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn build_single_axis_two_hier_crossjoin_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Option<String>, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
    measures_first: bool,
) -> String {
    let sh = match col_axis.second_hier.as_ref() {
        Some(s) => s,
        None => return String::new(),
    };
    let n_measures = matrix_measures.len();
    let hier1_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);
    let hier2_uname = format!("[{}].[{}]", sh.table, sh.hier);
    let h1i = build_hier_info(&hier1_uname, &col_axis.dim_props);
    let h2i = build_hier_info(&hier2_uname, &col_axis.dim_props);
    let mi = build_measures_hier_info();
    let axis0_hier = if measures_first {
        format!("{mi}{h1i}{h2i}")
    } else {
        format!("{h1i}{h2i}{mi}")
    };

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = axis0_hier,
        ci = build_cell_info(cell_props),
    );

    // Collect data from DAX rows.
    let mut h1_ordered: Vec<String> = Vec::new();
    let mut h1_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut h2_per_h1: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut h2_all: Vec<String> = Vec::new();
    let mut h2_all_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut leaf_vals: std::collections::HashMap<(String, String), Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (h1, opt_h2, vals) in cells {
        if h1_seen.insert(h1.clone()) {
            h1_ordered.push(h1.clone());
        }
        if let Some(h2) = opt_h2 {
            h2_per_h1.entry(h1.clone()).or_default().push(h2.clone());
            if h2_all_seen.insert(h2.clone()) {
                h2_all.push(h2.clone());
            }
            leaf_vals.insert((h1.clone(), h2.clone()), vals.clone());
        }
    }

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{v}")
        }
    };

    // Grand totals and per-H1 subtotals.
    let mut grand_totals = vec![0.0f64; n_measures];
    let mut h1_subtotals: std::collections::HashMap<String, Vec<f64>> =
        std::collections::HashMap::new();
    for (h1, _, vals) in cells {
        let sub = h1_subtotals
            .entry(h1.clone())
            .or_insert_with(|| vec![0.0; n_measures]);
        for (m, v) in vals.iter().enumerate().take(n_measures) {
            if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                grand_totals[m] += n;
                sub[m] += n;
            }
        }
    }
    let has_values = !cells.is_empty();

    // Ordered dim positions (grand-total, then per-H1 subtotal + children).
    // dim_pos[0] = (None, None) = grand total
    // dim_pos[1] = (Some(h1), None) = h1 subtotal
    // dim_pos[2..] = (Some(h1), Some(h2)) = leaves
    let mut dim_positions: Vec<(Option<&str>, Option<&str>)> = vec![(None, None)];
    for h1 in &h1_ordered {
        dim_positions.push((Some(h1.as_str()), None));
        if let Some(h2s) = h2_per_h1.get(h1) {
            for h2 in h2s {
                dim_positions.push((Some(h1.as_str()), Some(h2.as_str())));
            }
        }
    }
    let n_positions = dim_positions.len();

    let pos_of = |h1: Option<&str>, h2: Option<&str>| -> usize {
        dim_positions
            .iter()
            .position(|(a, b)| *a == h1 && *b == h2)
            .unwrap_or(0)
    };
    let axis0_pos = |h1: Option<&str>, h2: Option<&str>, m: usize| -> u32 {
        let p = pos_of(h1, h2);
        if measures_first {
            (m * n_positions + p) as u32
        } else {
            (p * n_measures + m) as u32
        }
    };

    // Member ordinals (0 = All, 1..N = leaves in encounter order).
    let h1_ord = |v: &str| -> u32 {
        h1_ordered
            .iter()
            .position(|s| s == v)
            .map(|i| i + 1)
            .unwrap_or(0) as u32
    };
    let h2_ord = |v: &str| -> u32 {
        h2_all
            .iter()
            .position(|s| s == v)
            .map(|i| i + 1)
            .unwrap_or(0) as u32
    };

    // Build NormTuples.
    // MemberRef order (dim-first): [H1, H2, Measure]; (measures-first): [Measure, H1, H2].
    let mut norm_tuples = String::new();
    if !measures_first {
        // Grand total: All × All × each measure.
        for m in 0..n_measures {
            let lm = m == n_measures - 1;
            norm_tuples.push_str(&build_norm_tuple_3(
                0,
                if lm { 197608 } else { 1000 },
                0,
                if lm { 132072 } else { 1000 },
                m as u32,
                if lm { 131072 } else { 0 },
            ));
        }
        for h1 in &h1_ordered {
            let h2s = h2_per_h1.get(h1).map(|v| v.as_slice()).unwrap_or(&[]);
            // H1 subtotal: H1 × All × each measure.
            for m in 0..n_measures {
                let lm = m == n_measures - 1;
                norm_tuples.push_str(&build_norm_tuple_3(
                    h1_ord(h1),
                    131072,
                    0,
                    if lm { 197608 } else { 132072 },
                    m as u32,
                    131072,
                ));
            }
            // H1 × H2 leaves × each measure.
            for (h2i_idx, h2) in h2s.iter().enumerate() {
                let lh2 = h2i_idx == h2s.len() - 1;
                for m in 0..n_measures {
                    let lm = m == n_measures - 1;
                    norm_tuples.push_str(&build_norm_tuple_3(
                        h1_ord(h1),
                        131072,
                        h2_ord(h2),
                        if (lh2 && lm) || (!lh2 && lm) {
                            131072
                        } else {
                            0
                        },
                        m as u32,
                        131072,
                    ));
                }
            }
        }
    } else {
        // Grand total: each measure × All × All.
        for m in 0..n_measures {
            let lm = m == n_measures - 1;
            norm_tuples.push_str(&build_norm_tuple_3(
                m as u32,
                if lm { 131072 } else { 0 },
                0,
                66536,
                0,
                66536,
            ));
        }
        for m in 0..n_measures {
            for h1 in &h1_ordered {
                let h2s = h2_per_h1.get(h1).map(|v| v.as_slice()).unwrap_or(&[]);
                // H1 subtotal.
                norm_tuples.push_str(&build_norm_tuple_3(
                    m as u32,
                    131072,
                    h1_ord(h1),
                    132072,
                    0,
                    66536,
                ));
                // H1 × H2 leaves.
                for h2 in h2s {
                    norm_tuples.push_str(&build_norm_tuple_3(
                        m as u32,
                        131072,
                        h1_ord(h1),
                        131072,
                        h2_ord(h2),
                        131072,
                    ));
                }
            }
        }
    }

    // MembersLookup.
    let h1_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let h1_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let h1_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let h2_all_uname = format!("[{}].[{}].[All]", sh.table, sh.hier);
    let h2_all_lname = format!("[{}].[{}].[(All)]", sh.table, sh.hier);
    let h2_leaf_lname = format!("[{}].[{}].[{}]", sh.table, sh.hier, sh.level);

    let members_h1 = build_norm_members_block_compact(
        &hier1_uname,
        &h1_all_uname,
        &h1_all_lname,
        &h1_leaf_lname,
        &h1_ordered,
    );
    let members_h2 = build_norm_members_block_compact(
        &hier2_uname,
        &h2_all_uname,
        &h2_all_lname,
        &h2_leaf_lname,
        &h2_all,
    );
    let members_meas = build_measures_norm_members(matrix_measures);
    let members_lookup = if measures_first {
        format!("<ns5:MembersLookup>{members_meas}{members_h1}{members_h2}</ns5:MembersLookup>")
    } else {
        format!("<ns5:MembersLookup>{members_h1}{members_h2}{members_meas}</ns5:MembersLookup>")
    };

    let norm_tuple_set = format!(
        "<ns5:NormTupleSet><ns5:NormTuples>{t}</ns5:NormTuples>{ml}</ns5:NormTupleSet>",
        t = norm_tuples,
        ml = members_lookup,
    );

    // CellData.
    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };
    let mut cd = String::from("<ns2:CellData>");
    // Grand total.
    for (m, gt) in grand_totals.iter().enumerate() {
        let val = if has_values {
            fmt_num(*gt)
        } else {
            "1".to_string()
        };
        cd.push_str(&cell_xml(axis0_pos(None, None, m), &val));
    }
    // Per-H1 subtotals and leaves.
    for h1 in &h1_ordered {
        let sub = h1_subtotals.get(h1);
        for m in 0..n_measures {
            if has_values {
                let val = sub.map(|s| fmt_num(s[m])).unwrap_or_default();
                if !val.is_empty() {
                    cd.push_str(&cell_xml(axis0_pos(Some(h1), None, m), &val));
                }
            }
        }
        if let Some(h2s) = h2_per_h1.get(h1) {
            for h2 in h2s {
                if let Some(vals) = leaf_vals.get(&(h1.clone(), h2.clone())) {
                    for m in 0..n_measures {
                        let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                        if !v.is_empty() {
                            cd.push_str(&cell_xml(axis0_pos(Some(h1), Some(h2), m), v));
                        }
                    }
                }
            }
        }
    }
    cd.push_str("</ns2:CellData>");

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0">{nts}</ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        nts = norm_tuple_set,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cd,
    )
}

// ── Single-axis CrossJoin with N≥2 independently-drilled dims ────────────────
//
// CrossJoin(...CrossJoin(dim1, measures)..., dimN) ON COLUMNS.
// Produces a NormTupleSet with N+1 hierarchies.  Measures appear at `measures_position`
// in the left-to-right tuple order.  Cell ordinals use row-major ordering based on
// the expanded-position counts for each slot.

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_single_axis_multi_dim_crossjoin(
    session_id: Option<&str>,
    cube_name: &str,
    dims: &[crate::mdx::AxisPlan],
    measures: &[(String, String)],
    measures_position: usize,
    cells: &[Vec<Option<String>>],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_single_axis_multi_dim_crossjoin_root(
        cube_name,
        dims,
        measures,
        measures_position,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_norm_tuple_n(refs: &[(u32, u32)]) -> String {
    let mut s = String::from("<ns5:NormTuple>");
    for (ord, di) in refs {
        s.push_str(&format!(
            "<ns5:MemberRef><ns5:MemberOrdinal>{ord}</ns5:MemberOrdinal><ns5:MemberDispInfo>{di}</ns5:MemberDispInfo></ns5:MemberRef>",
        ));
    }
    s.push_str("</ns5:NormTuple>");
    s
}

#[allow(clippy::too_many_arguments)]
fn build_single_axis_multi_dim_crossjoin_root(
    cube_name: &str,
    dims: &[crate::mdx::AxisPlan],
    measures: &[(String, String)],
    measures_position: usize,
    cells: &[Vec<Option<String>>],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let n_dims = dims.len();
    let n_meas = measures.len();

    // Collect unique leaf values per dim and leaf→measure values.
    let mut dim_leaves: Vec<Vec<String>> = vec![Vec::new(); n_dims];
    let mut dim_seen: Vec<std::collections::HashSet<String>> = vec![Default::default(); n_dims];
    let mut leaf_vals: std::collections::HashMap<Vec<String>, Vec<Option<String>>> =
        Default::default();

    for row in cells {
        let keys: Vec<String> = (0..n_dims)
            .map(|i| row.get(i).and_then(|v| v.clone()).unwrap_or_default())
            .collect();
        let vals: Vec<Option<String>> = (0..n_meas)
            .map(|i| row.get(n_dims + i).and_then(|v| v.clone()))
            .collect();
        for (di, key) in keys.iter().enumerate() {
            if !key.is_empty() && dim_seen[di].insert(key.clone()) {
                dim_leaves[di].push(key.clone());
            }
        }
        leaf_vals.insert(keys, vals);
    }

    // Number of "expanded" positions per dim: All + each leaf.
    let dim_n_pos: Vec<usize> = dim_leaves.iter().map(|l| 1 + l.len()).collect();

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{v}")
        }
    };

    // Compute a cell value: filter leaf_vals by per-dim positions (None = wildcard).
    let compute_val = |dim_pos: &[Option<String>], m_idx: usize| -> Option<f64> {
        let mut total = 0.0_f64;
        let mut any = false;
        for (keys, vals) in &leaf_vals {
            let matches = keys
                .iter()
                .zip(dim_pos.iter())
                .all(|(key, pos)| pos.as_ref().is_none_or(|p| key == p));
            if matches {
                if let Some(s) = vals.get(m_idx).and_then(|v| v.as_deref()) {
                    if let Ok(n) = s.parse::<f64>() {
                        total += n;
                        any = true;
                    }
                }
            }
        }
        if any {
            Some(total)
        } else {
            None
        }
    };

    // Slot sizes in tuple order (left-to-right):
    // [dim[0], ..., dim[meas_pos-1], Measures, dim[meas_pos], ..., dim[N-1]]
    // We build strides for row-major ordinal computation.
    let mut slot_sizes: Vec<usize> = Vec::with_capacity(n_dims + 1);
    for &n in &dim_n_pos[..measures_position] {
        slot_sizes.push(n);
    }
    slot_sizes.push(n_meas);
    for &n in &dim_n_pos[measures_position..n_dims] {
        slot_sizes.push(n);
    }

    let strides: Vec<usize> = (0..slot_sizes.len())
        .map(|j| slot_sizes[j + 1..].iter().product())
        .collect();

    // Ordinal from per-dim position indices (in dim-array order) + measure index.
    // Returns the cell ordinal for a given combination.
    let ordinal = |dim_pos_indices: &[usize], meas_idx: usize| -> u32 {
        let mut ord = 0usize;
        let mut slot = 0usize;
        for (i, &pi) in dim_pos_indices.iter().enumerate() {
            if i == measures_position {
                ord += meas_idx * strides[slot];
                slot += 1;
            }
            ord += pi * strides[slot];
            slot += 1;
        }
        if measures_position == n_dims {
            ord += meas_idx * strides[slot];
        }
        ord as u32
    };

    // Build HierarchyInfo blocks in tuple order.
    let mut hier_infos = String::new();
    for d in &dims[..measures_position] {
        hier_infos.push_str(&build_hier_info(
            &format!("[{}].[{}]", d.table, d.hier),
            &d.dim_props,
        ));
    }
    hier_infos.push_str(&build_measures_hier_info());
    for d in &dims[measures_position..n_dims] {
        hier_infos.push_str(&build_hier_info(
            &format!("[{}].[{}]", d.table, d.hier),
            &d.dim_props,
        ));
    }

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{ld}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{ls}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{hi}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        ld = xml_escape_value(last_data_update),
        ls = xml_escape_value(last_schema_update),
        hi = hier_infos,
        ci = build_cell_info(cell_props),
    );

    // Build NormTuples and CellData by enumerating all valid positions.
    // Enumerate in tuple-order (leftmost = outermost = slowest varying).
    // Each dim has positions: [0=All, 1=leaf[0], 2=leaf[1], ...]
    let has_values = !cells.is_empty();
    let mut norm_tuples = String::new();
    let mut cd = String::from("<ns2:CellData>");
    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    // Enumerate positions via counter array (one entry per dim, 0..dim_n_pos[i]).
    // We iterate in tuple-order using a recursive-style counter.
    // For simplicity, use a flat iteration approach for the common N=2 case
    // but written generically using index vectors.
    let total_dim_combos: usize = dim_n_pos.iter().product();
    let dim_strides_local: Vec<usize> = (0..n_dims)
        .map(|i| dim_n_pos[i + 1..].iter().product())
        .collect();

    for flat_dim in 0..total_dim_combos {
        // Decode dim position indices.
        let dim_pi: Vec<usize> = (0..n_dims)
            .map(|i| (flat_dim / dim_strides_local[i]) % dim_n_pos[i])
            .collect();

        // Dim positions as Option<String>: 0 → None (All), k → Some(leaf[k-1]).
        let dim_pos: Vec<Option<String>> = dim_pi
            .iter()
            .enumerate()
            .map(|(i, &pi)| {
                if pi == 0 {
                    None
                } else {
                    Some(dim_leaves[i][pi - 1].clone())
                }
            })
            .collect();

        // Skip leaf combos not present in the data.
        let all_specific = dim_pos.iter().all(|p| p.is_some());
        if all_specific {
            let key: Vec<String> = dim_pos
                .iter()
                .map(|p| p.clone().unwrap_or_default())
                .collect();
            if !leaf_vals.contains_key(&key) {
                continue;
            }
        }

        for meas_idx in 0..n_meas {
            // Build NormTuple MemberRefs in tuple order.
            let mut refs: Vec<(u32, u32)> = Vec::with_capacity(n_dims + 1);
            let is_last_meas = meas_idx == n_meas - 1;
            let is_last_dim_combo = dim_pi
                .iter()
                .enumerate()
                .all(|(i, &pi)| pi == dim_n_pos[i] - 1);
            let is_last_overall = is_last_meas && is_last_dim_combo;

            for slot_j in 0..=(n_dims) {
                if slot_j == measures_position {
                    let di: u32 = if slot_j < n_dims {
                        let is_last_slot = slot_j == n_dims && is_last_meas;
                        let _ = is_last_slot;
                        0u32
                    } else {
                        0u32
                    };
                    let _ = di;
                    // Measures slot.
                    let m_di: u32 = if is_last_overall { 131072 } else { 0 };
                    refs.push((meas_idx as u32, m_di));
                } else {
                    let dim_i = if slot_j < measures_position {
                        slot_j
                    } else {
                        slot_j - 1
                    };
                    let pi = dim_pi[dim_i];
                    let d_di: u32 = if pi == 0 {
                        if is_last_overall {
                            66536
                        } else {
                            1000
                        }
                    } else {
                        131072
                    };
                    refs.push((pi as u32, d_di));
                }
            }
            norm_tuples.push_str(&build_norm_tuple_n(&refs));

            // CellData.
            let ord = ordinal(&dim_pi, meas_idx);
            if has_values {
                if let Some(v) = compute_val(&dim_pos, meas_idx) {
                    cd.push_str(&cell_xml(ord, &fmt_num(v)));
                }
            } else {
                cd.push_str(&cell_xml(ord, "1"));
            }
        }
    }
    cd.push_str("</ns2:CellData>");

    // MembersLookup blocks in tuple order.
    let mut members_lookup_inner = String::new();
    for i in 0..measures_position {
        let d = &dims[i];
        let hier_uname = format!("[{}].[{}]", d.table, d.hier);
        let all_uname = format!("[{}].[{}].[All]", d.table, d.hier);
        let all_lname = format!("[{}].[{}].[(All)]", d.table, d.hier);
        let leaf_lname = format!("[{}].[{}].[{}]", d.table, d.hier, d.level);
        members_lookup_inner.push_str(&build_norm_members_block_compact(
            &hier_uname,
            &all_uname,
            &all_lname,
            &leaf_lname,
            &dim_leaves[i],
        ));
    }
    members_lookup_inner.push_str(&build_measures_norm_members(measures));
    for i in measures_position..n_dims {
        let d = &dims[i];
        let hier_uname = format!("[{}].[{}]", d.table, d.hier);
        let all_uname = format!("[{}].[{}].[All]", d.table, d.hier);
        let all_lname = format!("[{}].[{}].[(All)]", d.table, d.hier);
        let leaf_lname = format!("[{}].[{}].[{}]", d.table, d.hier, d.level);
        members_lookup_inner.push_str(&build_norm_members_block_compact(
            &hier_uname,
            &all_uname,
            &all_lname,
            &leaf_lname,
            &dim_leaves[i],
        ));
    }

    let norm_tuple_set = format!(
        "<ns5:NormTupleSet><ns5:NormTuples>{t}</ns5:NormTuples><ns5:MembersLookup>{ml}</ns5:MembersLookup></ns5:NormTupleSet>",
        t = norm_tuples, ml = members_lookup_inner,
    );

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0">{nts}</ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        nts = norm_tuple_set,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cd,
    )
}

// ── Measures-on-rows response ─────────────────────────────────────────────────
//
// Pattern: simple dim ON COLUMNS, pure measures set ON ROWS.
// Axis0 = simple Tuples (col dim All + leaves).
// Axis1 = simple Tuples (one per measure — no NormTupleSet, no CrossJoin).
// cell ordinal = col_idx + n_col_members * meas_idx

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_meas_on_rows(
    session_id: Option<&str>,
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_meas_on_rows_root(
        cube_name,
        col_axis,
        matrix_measures,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_meas_on_rows_root(
    cube_name: &str,
    col_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let n_measures = matrix_measures.len();
    let col_hier_uname = format!("[{}].[{}]", col_axis.table, col_axis.hier);

    let col_hi = build_hier_info(&col_hier_uname, &col_axis.dim_props);
    let meas_hi = build_measures_hier_info();

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{a1}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = col_hi,
        a1 = meas_hi,
        cell_info = build_cell_info(cell_props),
    );

    // Collect distinct col leaves in encounter order and measure values per leaf.
    let mut col_leaves: Vec<String> = Vec::new();
    let mut col_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut leaf_vals: std::collections::HashMap<String, Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (c, vals) in cells {
        if col_seen.insert(c.clone()) {
            col_leaves.push(c.clone());
        }
        leaf_vals.insert(c.clone(), vals.clone());
    }

    let n_col_leaves = col_leaves.len();
    let n_col_members = 1 + n_col_leaves;

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    // Per-measure column subtotals and grand totals.
    let mut col_subtotals: Vec<Vec<f64>> = vec![vec![0.0; n_measures]; n_col_leaves];
    for (c, vals) in cells {
        let ci = col_leaves.iter().position(|x| x == c);
        for (m, v) in vals.iter().enumerate().take(n_measures) {
            if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                if let Some(ci) = ci {
                    col_subtotals[ci][m] += n;
                }
            }
        }
    }
    let grand_totals: Vec<f64> = (0..n_measures)
        .map(|m| {
            col_subtotals
                .iter()
                .map(|s| s.get(m).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();
    let has_values = !cells.is_empty();

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    // cell ordinal = col_idx + n_col_members * meas_idx
    let mut cell_data = String::from("<ns2:CellData>");

    for (m, gt) in grand_totals.iter().enumerate() {
        let val = if has_values {
            fmt_num(*gt)
        } else {
            "1".to_string()
        };
        cell_data.push_str(&cell_xml(n_col_members as u32 * m as u32, &val));
        for (ci, col_val) in col_leaves.iter().enumerate() {
            if let Some(vals) = leaf_vals.get(col_val) {
                let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                if !v.is_empty() {
                    cell_data.push_str(&cell_xml(
                        (ci + 1) as u32 + n_col_members as u32 * m as u32,
                        v,
                    ));
                }
            } else if !has_values {
                cell_data.push_str(&cell_xml(
                    (ci + 1) as u32 + n_col_members as u32 * m as u32,
                    "1",
                ));
            }
        }
    }
    cell_data.push_str("</ns2:CellData>");

    // Axis0: simple Tuples for col dim (All + leaves).
    let col_all_uname = format!("[{}].[{}].[All]", col_axis.table, col_axis.hier);
    let col_all_lname = format!("[{}].[{}].[(All)]", col_axis.table, col_axis.hier);
    let col_leaf_lname = format!(
        "[{}].[{}].[{}]",
        col_axis.table, col_axis.hier, col_axis.level
    );
    let mut axis0_tuples = String::new();
    axis0_tuples.push_str(&build_all_member_tuple(
        &col_all_uname,
        &col_all_lname,
        &col_hier_uname,
        66536,
        &col_axis.dim_props,
    ));
    for (j, cap) in col_leaves.iter().enumerate() {
        let di = if j == n_col_leaves - 1 { 131072 } else { 0 };
        axis0_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &col_leaf_lname,
            &col_all_uname,
            &col_hier_uname,
            di,
            &col_axis.dim_props,
        ));
    }

    // Axis1: one Tuple per measure.
    let mut axis1_tuples = String::new();
    for (name, _) in matrix_measures {
        let cap = xml_escape_value(name);
        axis1_tuples.push_str(&format!(
            concat!(
                "<ns2:Tuple>",
                "<ns2:Member>",
                "<ns2:UName>[Measures].[{uname}]</ns2:UName>",
                "<ns2:Caption>{cap}</ns2:Caption>",
                "<ns2:LName>[Measures].[MeasuresLevel]</ns2:LName>",
                "<ns2:LNum>0</ns2:LNum>",
                "<ns2:HIERARCHY_UNIQUE_NAME>[Measures]</ns2:HIERARCHY_UNIQUE_NAME>",
                "</ns2:Member>",
                "</ns2:Tuple>",
            ),
            uname = cap,
            cap = cap,
        ));
    }

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="Axis1"><ns2:Tuples>{a1}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        a0 = axis0_tuples,
        a1 = axis1_tuples,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Measures-on-cols response ─────────────────────────────────────────────────
//
// Pattern: pure measures set ON COLUMNS, simple single-hier dim ON ROWS.
// Axis0 = one Tuple per measure (no All for measures).
// Axis1 = simple Tuples for dim (All + leaves).
// cell ordinal = meas_idx + n_measures * row_idx

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_meas_on_cols(
    session_id: Option<&str>,
    cube_name: &str,
    dim_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_meas_on_cols_root(
        cube_name,
        dim_axis,
        matrix_measures,
        cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_meas_on_cols_root(
    cube_name: &str,
    dim_axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    cells: &[(String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let n_measures = matrix_measures.len();
    let dim_hier_uname = format!("[{}].[{}]", dim_axis.table, dim_axis.hier);

    let dim_hi = build_hier_info(&dim_hier_uname, &dim_axis.dim_props);
    let meas_hi = build_measures_hier_info();

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{a0}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{a1}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        a0 = meas_hi,
        a1 = dim_hi,
        cell_info = build_cell_info(cell_props),
    );

    // Collect distinct row leaves in encounter order and measure values per leaf.
    let mut row_leaves: Vec<String> = Vec::new();
    let mut row_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut leaf_vals: std::collections::HashMap<String, Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (r, vals) in cells {
        if row_seen.insert(r.clone()) {
            row_leaves.push(r.clone());
        }
        leaf_vals.insert(r.clone(), vals.clone());
    }

    let n_row_leaves = row_leaves.len();

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    // Per-measure row subtotals and grand totals.
    let mut row_subtotals: Vec<Vec<f64>> = vec![vec![0.0; n_measures]; n_row_leaves];
    for (r, vals) in cells {
        let ri = row_leaves.iter().position(|x| x == r);
        for (m, v) in vals.iter().enumerate().take(n_measures) {
            if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                if let Some(ri) = ri {
                    row_subtotals[ri][m] += n;
                }
            }
        }
    }
    let grand_totals: Vec<f64> = (0..n_measures)
        .map(|m| {
            row_subtotals
                .iter()
                .map(|s| s.get(m).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();
    let has_values = !cells.is_empty();

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            v = xml_escape_value(value)
        )
    };

    // cell ordinal = meas_idx + n_measures * row_idx
    let mut cell_data = String::from("<ns2:CellData>");

    // row_idx = 0 (All): grand totals for each measure.
    for (m, gt) in grand_totals.iter().enumerate() {
        let val = if has_values {
            fmt_num(*gt)
        } else {
            "1".to_string()
        };
        cell_data.push_str(&cell_xml(m as u32, &val));
    }
    // row_idx = ri+1 (each leaf): per-row measure values.
    for (ri, row_val) in row_leaves.iter().enumerate() {
        let base = (ri + 1) as u32 * n_measures as u32;
        if let Some(vals) = leaf_vals.get(row_val) {
            for m in 0..n_measures {
                let v = vals.get(m).and_then(|x| x.as_deref()).unwrap_or("");
                if !v.is_empty() {
                    cell_data.push_str(&cell_xml(base + m as u32, v));
                }
            }
        } else if !has_values {
            for m in 0..n_measures {
                cell_data.push_str(&cell_xml(base + m as u32, "1"));
            }
        }
    }
    cell_data.push_str("</ns2:CellData>");

    // Axis0: one Tuple per measure (no All member for measures).
    let mut axis0_tuples = String::new();
    for (i, (name, _)) in matrix_measures.iter().enumerate() {
        let di = if i == n_measures - 1 { 131072 } else { 0 };
        axis0_tuples.push_str(&build_measure_tuple(name, di));
    }

    // Axis1: simple Tuples for dim (All + leaves).
    let dim_all_uname = format!("[{}].[{}].[All]", dim_axis.table, dim_axis.hier);
    let dim_all_lname = format!("[{}].[{}].[(All)]", dim_axis.table, dim_axis.hier);
    let dim_leaf_lname = format!(
        "[{}].[{}].[{}]",
        dim_axis.table, dim_axis.hier, dim_axis.level
    );
    let mut axis1_tuples = String::new();
    axis1_tuples.push_str(&build_all_member_tuple(
        &dim_all_uname,
        &dim_all_lname,
        &dim_hier_uname,
        66536,
        &dim_axis.dim_props,
    ));
    for (j, cap) in row_leaves.iter().enumerate() {
        let di = if j == n_row_leaves - 1 { 131072 } else { 0 };
        axis1_tuples.push_str(&build_leaf_member_tuple(
            cap,
            &dim_leaf_lname,
            &dim_all_uname,
            &dim_hier_uname,
            di,
            &dim_axis.dim_props,
        ));
    }

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{a0}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="Axis1"><ns2:Tuples>{a1}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        a0 = axis0_tuples,
        a1 = axis1_tuples,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

// ── Multi-measure matrix response ─────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
pub fn execute_mdx_cellset_matrix(
    session_id: Option<&str>,
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    matrix_cells: &[(String, String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> (String, Response) {
    let inner = build_matrix_root(
        cube_name,
        axis,
        matrix_measures,
        matrix_cells,
        cell_props,
        last_data_update,
        last_schema_update,
    );
    let xml = cellset_envelope_two_hier(session_id, &inner);
    let response = (
        StatusCode::OK,
        [("Content-Type", "text/xml; charset=utf-8")],
        xml.clone(),
    )
        .into_response();
    (xml, response)
}

fn build_measures_hier_info() -> String {
    let prop = |elem: &str, name: &str, typ: &str| -> String {
        format!(r#"<ns2:{elem} name="[Measures].[{name}]" type="{typ}" />"#)
    };
    let mut out = String::from(r#"<ns2:HierarchyInfo name="[Measures]">"#);
    out.push_str(&prop("UName", "MEMBER_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("Caption", "MEMBER_CAPTION", "xs:string"));
    out.push_str(&prop("LName", "LEVEL_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("LNum", "LEVEL_NUMBER", "xs:int"));
    out.push_str(&prop("DisplayInfo", "DISPLAY_INFO", "xs:unsignedInt"));
    out.push_str(&prop(
        "PARENT_UNIQUE_NAME",
        "PARENT_UNIQUE_NAME",
        "xs:string",
    ));
    out.push_str(&prop(
        "HIERARCHY_UNIQUE_NAME",
        "HIERARCHY_UNIQUE_NAME",
        "xs:string",
    ));
    out.push_str("</ns2:HierarchyInfo>");
    out
}

fn build_measure_tuple(name: &str, display_info: u32) -> String {
    let cap = xml_escape_value(name);
    format!(
        concat!(
            "<ns2:Tuple><ns2:Member>",
            "<ns2:UName>[Measures].[{uname}]</ns2:UName>",
            "<ns2:Caption>{cap}</ns2:Caption>",
            "<ns2:LName>[Measures].[MeasuresLevel]</ns2:LName>",
            "<ns2:LNum>0</ns2:LNum>",
            "<ns2:DisplayInfo>{di}</ns2:DisplayInfo>",
            "<ns2:HIERARCHY_UNIQUE_NAME>[Measures]</ns2:HIERARCHY_UNIQUE_NAME>",
            "</ns2:Member></ns2:Tuple>",
        ),
        uname = cap,
        cap = cap,
        di = display_info,
    )
}

fn build_matrix_root(
    cube_name: &str,
    axis: &crate::mdx::AxisPlan,
    matrix_measures: &[(String, String)],
    matrix_cells: &[(String, String, Vec<Option<String>>)],
    cell_props: &[String],
    last_data_update: &str,
    last_schema_update: &str,
) -> String {
    let sh = match axis.second_hier.as_ref() {
        Some(s) => s,
        None => return String::from("<ns2:root />"),
    };

    let n_measures = matrix_measures.len();
    let hier1_uname = format!("[{}].[{}]", axis.table, axis.hier);
    let hier2_uname = format!("[{}].[{}]", sh.table, sh.hier);

    let olap_info = format!(
        concat!(
            "<ns2:OlapInfo>",
            "<ns2:CubeInfo><ns2:Cube>",
            "<ns2:CubeName>{cube}</ns2:CubeName>",
            "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
            "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
            "</ns2:Cube></ns2:CubeInfo>",
            "<ns2:AxesInfo>",
            r#"<ns2:AxisInfo name="Axis0">{mi}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="Axis1">{h1i}{h2i}</ns2:AxisInfo>"#,
            r#"<ns2:AxisInfo name="SlicerAxis" />"#,
            "</ns2:AxesInfo>",
            "<ns2:CellInfo>{ci}</ns2:CellInfo>",
            "</ns2:OlapInfo>",
        ),
        cube = xml_escape_value(cube_name),
        last_data = xml_escape_value(last_data_update),
        last_schema = xml_escape_value(last_schema_update),
        mi = build_measures_hier_info(),
        h1i = build_hier_info(&hier1_uname, &axis.dim_props),
        h2i = build_hier_info(&hier2_uname, &axis.dim_props),
        ci = build_cell_info(cell_props),
    );

    // Axis0: one Tuple per measure.
    let last_m = n_measures.saturating_sub(1);
    let measure_tuples: String = matrix_measures
        .iter()
        .enumerate()
        .map(|(i, (name, _))| build_measure_tuple(name, if i == last_m { 131072 } else { 0 }))
        .collect();

    // Collect ordered distinct H1 / H2 values and per-H1 leaf lists.
    let mut h1_ordered: Vec<String> = Vec::new();
    let mut h1_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut h2_ordered: Vec<String> = Vec::new();
    let mut h2_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut h2_for_h1: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    let mut leaf_vals: std::collections::HashMap<(String, String), Vec<Option<String>>> =
        std::collections::HashMap::new();

    for (h1, h2, vals) in matrix_cells {
        if h1_seen.insert(h1.clone()) {
            h1_ordered.push(h1.clone());
        }
        if h2_seen.insert(h2.clone()) {
            h2_ordered.push(h2.clone());
        }
        h2_for_h1.entry(h1.clone()).or_default().push(h2.clone());
        leaf_vals.insert((h1.clone(), h2.clone()), vals.clone());
    }

    let has_values = !matrix_cells.is_empty();

    // Compact ordinals: All = 0, first leaf = 1, second = 2, …
    let h1_ord = |val: &str| -> u32 {
        h1_ordered
            .iter()
            .position(|v| v == val)
            .map(|i| (i + 1) as u32)
            .unwrap_or(0)
    };
    let h2_ord = |val: &str| -> u32 {
        h2_ordered
            .iter()
            .position(|v| v == val)
            .map(|i| (i + 1) as u32)
            .unwrap_or(0)
    };

    // Compute per-measure subtotals and grand totals.
    let mut h1_subtotals: std::collections::HashMap<String, Vec<f64>> =
        std::collections::HashMap::new();
    for (h1, h2, _) in matrix_cells {
        if let Some(vals) = leaf_vals.get(&(h1.clone(), h2.clone())) {
            let subs = h1_subtotals
                .entry(h1.clone())
                .or_insert_with(|| vec![0.0; n_measures]);
            for (i, v) in vals.iter().enumerate() {
                if let Some(n) = v.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                    if i < subs.len() {
                        subs[i] += n;
                    }
                }
            }
        }
    }
    let grand_totals: Vec<f64> = (0..n_measures)
        .map(|i| {
            h1_subtotals
                .values()
                .map(|s| s.get(i).copied().unwrap_or(0.0))
                .sum()
        })
        .collect();

    let fmt_num = |v: f64| -> String {
        if v.fract() == 0.0 && v.abs() < 1.0e15 {
            format!("{}", v as i64)
        } else {
            format!("{}", v)
        }
    };

    let cell_xml = |ordinal: u32, value: &str| -> String {
        format!(
            r#"<ns2:Cell CellOrdinal="{ordinal}"><ns2:Value>{v}</ns2:Value></ns2:Cell>"#,
            ordinal = ordinal,
            v = xml_escape_value(value)
        )
    };

    let mut norm_tuples = String::new();
    let mut cell_data = String::from("<ns2:CellData>");
    let mut row_index: u32 = 0;

    // Grand total row.
    norm_tuples.push_str(&build_norm_tuple(0, 66536, 0, 1000));
    for (m, gt) in grand_totals.iter().enumerate() {
        let val = if has_values {
            fmt_num(*gt)
        } else {
            "1".to_string()
        };
        cell_data.push_str(&cell_xml(row_index * n_measures as u32 + m as u32, &val));
    }
    row_index += 1;

    let n_h1 = h1_ordered.len();
    for (h1_idx, h1_val) in h1_ordered.iter().enumerate() {
        let is_last_h1 = h1_idx == n_h1 - 1;
        let h1o = h1_ord(h1_val);
        let empty_vec = Vec::new();
        let h2s = h2_for_h1.get(h1_val).unwrap_or(&empty_vec);
        let n = h2s.len();

        // H1 × All subtotal row.
        let h1_sub_di: u32 = if is_last_h1 { 131072 } else { 0 };
        let h2_sub_di: u32 = if n == 1 { 197608 } else { 66536 };
        norm_tuples.push_str(&build_norm_tuple(h1o, h1_sub_di, 0, h2_sub_di));
        let subs = h1_subtotals
            .get(h1_val.as_str())
            .cloned()
            .unwrap_or_else(|| vec![0.0; n_measures]);
        for (m, sv) in subs.iter().enumerate() {
            let val = if has_values {
                fmt_num(*sv)
            } else {
                "1".to_string()
            };
            cell_data.push_str(&cell_xml(row_index * n_measures as u32 + m as u32, &val));
        }
        row_index += 1;

        // H1 × H2 leaf rows.
        for (j, h2_val) in h2s.iter().enumerate() {
            let h2o = h2_ord(h2_val);
            let h2_di: u32 = if n > 1 && j == n - 1 { 131072 } else { 0 };
            norm_tuples.push_str(&build_norm_tuple(h1o, 131072, h2o, h2_di));
            let vals = leaf_vals.get(&(h1_val.clone(), h2_val.clone()));
            for m in 0..n_measures {
                let val = vals
                    .and_then(|v| v.get(m))
                    .and_then(|x| x.as_deref())
                    .unwrap_or(if has_values { "" } else { "1" });
                cell_data.push_str(&cell_xml(row_index * n_measures as u32 + m as u32, val));
            }
            row_index += 1;
        }
    }

    cell_data.push_str("</ns2:CellData>");

    // MembersLookup (compact — no empty placeholder).
    let h1_all_uname = format!("[{}].[{}].[All]", axis.table, axis.hier);
    let h1_all_lname = format!("[{}].[{}].[(All)]", axis.table, axis.hier);
    let h1_leaf_lname = format!("[{}].[{}].[{}]", axis.table, axis.hier, axis.level);
    let h2_all_uname = format!("[{}].[{}].[All]", sh.table, sh.hier);
    let h2_all_lname = format!("[{}].[{}].[(All)]", sh.table, sh.hier);
    let h2_leaf_lname = format!("[{}].[{}].[{}]", sh.table, sh.hier, sh.level);

    let members_h1 = build_norm_members_block_compact(
        &hier1_uname,
        &h1_all_uname,
        &h1_all_lname,
        &h1_leaf_lname,
        &h1_ordered,
    );
    let members_h2 = build_norm_members_block_compact(
        &hier2_uname,
        &h2_all_uname,
        &h2_all_lname,
        &h2_leaf_lname,
        &h2_ordered,
    );

    let norm_tuple_set = format!(
        concat!(
            "<ns5:NormTupleSet>",
            "<ns5:NormTuples>{tuples}</ns5:NormTuples>",
            "<ns5:MembersLookup>{m1}{m2}</ns5:MembersLookup>",
            "</ns5:NormTupleSet>",
        ),
        tuples = norm_tuples,
        m1 = members_h1,
        m2 = members_h2,
    );

    let axes = format!(
        concat!(
            "<ns2:Axes>",
            r#"<ns2:Axis name="Axis0"><ns2:Tuples>{mt}</ns2:Tuples></ns2:Axis>"#,
            r#"<ns2:Axis name="Axis1">{nts}</ns2:Axis>"#,
            r#"<ns2:Axis name="SlicerAxis"><ns2:Tuples /></ns2:Axis>"#,
            "</ns2:Axes>",
        ),
        mt = measure_tuples,
        nts = norm_tuple_set,
    );

    format!(
        "<ns2:root>{schema}{olap}{axes}{cell}</ns2:root>",
        schema = MDDATASET_SCHEMA,
        olap = olap_info,
        axes = axes,
        cell = cell_data,
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::catalog::SummarizeBy;

    const FAKE_TIMESTAMP: &str = "2025-01-01T00:00:00";

    fn make_table_meta(name: &str, columns: Vec<(&str, Option<&str>)>) -> TableMeta {
        TableMeta {
            name: name.to_string(),
            columns: columns
                .into_iter()
                .map(|(col_name, sort_by)| ColumnMeta {
                    name: col_name.to_string(),
                    data_type: "string".to_string(),
                    summarize_by: SummarizeBy::None,
                    is_hidden: false,
                    format_string: None,
                    display_folder: None,
                    data_category: None,
                    description: None,
                    sort_by_column: sort_by.map(|s| s.to_string()),
                    is_key: false,
                    is_nullable: true,
                    is_unique: false,
                })
                .collect(),
            is_hidden: false,
            data_category: None,
            description: None,
        }
    }

    #[test]
    fn tmschema_columns_emits_sort_by_column_id() {
        // Table has two columns: "MonthName" (sorted by "MonthNumber") and "MonthNumber".
        // Columns are sorted alphabetically by list_tables, so MonthName=col 1, MonthNumber=col 2.
        // col_ids: table_id=1, MonthName→1001, MonthNumber→1002.
        let tables = vec![make_table_meta(
            "Calendar",
            vec![("MonthName", Some("MonthNumber")), ("MonthNumber", None)],
        )];
        let (_, response) = tmschema_columns(None, &tables);
        let body = response.into_body();
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(http_body_util::BodyExt::collect(body))
            .unwrap()
            .to_bytes();
        let xml = std::str::from_utf8(&bytes).unwrap();

        // MonthName row: SortByColumnID should be 1002 (MonthNumber's ID).
        assert!(
            xml.contains("<SortByColumnID>1002</SortByColumnID>"),
            "expected SortByColumnID=1002: {xml}"
        );
        // MonthNumber row: SortByColumnID should be 0 (no sort-by).
        assert!(
            xml.contains("<SortByColumnID>0</SortByColumnID>"),
            "expected SortByColumnID=0: {xml}"
        );
    }

    #[test]
    fn measuregroup_dimensions_reports_fact_table_as_measuregroup() {
        // FactSales (many/fromTable) -> DimProduct (one/toTable), matching this
        // codebase's relationship convention (see ExecutionContext::
        // expanded_filter_context). The measure group must be the fact table,
        // not the dimension — regression test for a from/to inversion bug.
        let tables = vec![
            make_table_meta("FactSales", vec![("ProductKey", None)]),
            make_table_meta("DimProduct", vec![("ProductKey", None)]),
        ];
        let relationships = vec![crate::server::provider::RelationshipMeta {
            name: "FactSales_DimProduct".into(),
            from_table: "FactSales".into(),
            from_column: "ProductKey".into(),
            to_table: "DimProduct".into(),
            to_column: "ProductKey".into(),
            is_active: true,
            bidirectional: false,
        }];
        let (_, response) =
            discover_mdschema_measuregroup_dimensions(None, "TestCatalog", &tables, &relationships);
        let body = response.into_body();
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(http_body_util::BodyExt::collect(body))
            .unwrap()
            .to_bytes();
        let xml = std::str::from_utf8(&bytes).unwrap();

        assert!(
            xml.contains("<MEASUREGROUP_NAME>FactSales</MEASUREGROUP_NAME><MEASUREGROUP_CARDINALITY>MANY</MEASUREGROUP_CARDINALITY><DIMENSION_UNIQUE_NAME>[DimProduct]</DIMENSION_UNIQUE_NAME>"),
            "expected FactSales as measure group with DimProduct as its dimension: {xml}"
        );
        assert!(
            xml.contains(
                "<DIMENSION_GRANULARITY>[DimProduct].[ProductKey]</DIMENSION_GRANULARITY>"
            ),
            "expected granularity to reference the dimension's own key column: {xml}"
        );
    }

    #[test]
    fn tmschema_columns_sort_by_missing_target_emits_zero() {
        // If sort_by_column names a column that doesn't exist, emit 0 (don't panic).
        let tables = vec![make_table_meta("T", vec![("Col", Some("Ghost"))])];
        let (_, response) = tmschema_columns(None, &tables);
        let body = response.into_body();
        let bytes = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(http_body_util::BodyExt::collect(body))
            .unwrap()
            .to_bytes();
        let xml = std::str::from_utf8(&bytes).unwrap();
        assert!(
            xml.contains("<SortByColumnID>0</SortByColumnID>"),
            "expected fallback 0: {xml}"
        );
    }

    // ── $system query tests ───────────────────────────────────────────────────
    //
    // Two layers tested separately:
    //   1. Data layer  — DmvResult rows (no XML)
    //   2. XML layer   — render_dmv_result output matches original handlers

    fn system_databases() -> Vec<DatabaseMeta> {
        vec![DatabaseMeta {
            id: "DemoModel".into(),
            name: "DemoModel".into(),
            last_schema_update: FAKE_TIMESTAMP.into(),
            last_refreshed: FAKE_TIMESTAMP.into(),
        }]
    }

    fn system_measures() -> Vec<MeasureMeta> {
        vec![
            MeasureMeta {
                name: "TotalAmount".into(),
                table_name: "Sales".into(),
                display_name: "Total Amount".into(),
                expression: "SUM(Sales[Amount])".into(),
                aggregator: 1,
                data_type: 5,
                is_hidden: false,
                format_string: None,
                display_folder: None,
                description: None,
            },
            MeasureMeta {
                name: "HiddenMeasure".into(),
                table_name: "Sales".into(),
                display_name: "Hidden".into(),
                expression: "1".into(),
                aggregator: 1,
                data_type: 5,
                is_hidden: true,
                format_string: None,
                display_folder: None,
                description: None,
            },
        ]
    }

    fn system_tables() -> Vec<TableMeta> {
        vec![
            TableMeta {
                name: "Sales".into(),
                columns: vec![],
                is_hidden: false,
                data_category: None,
                description: None,
            },
            TableMeta {
                name: "Product".into(),
                columns: vec![],
                is_hidden: false,
                data_category: None,
                description: None,
            },
        ]
    }

    fn col<'a>(row: &'a Row, name: &str) -> Option<&'a str> {
        row.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn discover_hierarchies_returns_column_rows() {
        let tables = vec![
            make_table_meta("Sales", vec![("Amount", None), ("Date", None)]),
            make_table_meta("Product", vec![("Color", None)]),
        ];
        let (xml, _) = discover_hierarchies(None, "DemoModel", &tables);
        // Correct envelope
        assert!(xml.contains("DiscoverResponse"), "envelope: {xml}");
        // Schema declares all key columns
        assert!(
            xml.contains(r#"name="HIERARCHY_UNIQUE_NAME" type="xsd:string""#),
            "schema HIERARCHY_UNIQUE_NAME: {xml}"
        );
        assert!(
            xml.contains(r#"name="HIERARCHY_ORIGIN" type="xsd:unsignedShort""#),
            "schema HIERARCHY_ORIGIN: {xml}"
        );
        assert!(
            xml.contains(r#"name="GROUPING_BEHAVIOR" type="xsd:unsignedShort""#),
            "schema GROUPING_BEHAVIOR: {xml}"
        );
        assert!(
            xml.contains(r#"name="STRUCTURE_TYPE" type="xsd:string""#),
            "schema STRUCTURE_TYPE: {xml}"
        );
        // [Measures] hierarchy row
        assert!(
            xml.contains("<DIMENSION_UNIQUE_NAME>[Measures]</DIMENSION_UNIQUE_NAME>"),
            "Measures dim: {xml}"
        );
        assert!(
            xml.contains("<HIERARCHY_UNIQUE_NAME>[Measures]</HIERARCHY_UNIQUE_NAME>"),
            "Measures hier: {xml}"
        );
        assert!(
            xml.contains("<DIMENSION_TYPE>2</DIMENSION_TYPE>"),
            "Measures dim type: {xml}"
        );
        assert!(
            xml.contains("<HIERARCHY_ORIGIN>6</HIERARCHY_ORIGIN>"),
            "Measures origin: {xml}"
        );
        assert!(
            xml.contains("<GROUPING_BEHAVIOR>2</GROUPING_BEHAVIOR>"),
            "Measures grouping: {xml}"
        );
        // Column hierarchy rows
        assert!(
            xml.contains("<HIERARCHY_UNIQUE_NAME>[Sales].[Amount]</HIERARCHY_UNIQUE_NAME>"),
            "Amount: {xml}"
        );
        assert!(
            xml.contains("<HIERARCHY_UNIQUE_NAME>[Sales].[Date]</HIERARCHY_UNIQUE_NAME>"),
            "Date: {xml}"
        );
        assert!(
            xml.contains("<HIERARCHY_UNIQUE_NAME>[Product].[Color]</HIERARCHY_UNIQUE_NAME>"),
            "Color: {xml}"
        );
        assert!(
            xml.contains("<DEFAULT_MEMBER>[Sales].[Amount].[All]</DEFAULT_MEMBER>"),
            "Amount default member: {xml}"
        );
        assert!(
            xml.contains("<HIERARCHY_ORIGIN>2</HIERARCHY_ORIGIN>"),
            "attr origin: {xml}"
        );
        assert!(
            xml.contains("<GROUPING_BEHAVIOR>1</GROUPING_BEHAVIOR>"),
            "attr grouping: {xml}"
        );
        // Catalog name populated from argument
        assert!(
            xml.contains("<CATALOG_NAME>DemoModel</CATALOG_NAME>"),
            "catalog: {xml}"
        );
    }

    #[test]
    fn discover_levels_returns_all_and_member_levels() {
        let tables = vec![make_table_meta(
            "Sales",
            vec![("Amount", None), ("Date", None)],
        )];
        let (xml, _) = discover_levels(None, "DemoModel", &tables);
        assert!(xml.contains("DiscoverResponse"), "envelope: {xml}");
        assert!(
            xml.contains(r#"name="LEVEL_ORIGIN" type="xsd:unsignedShort""#),
            "schema LEVEL_ORIGIN: {xml}"
        );
        assert!(
            xml.contains(r#"name="LEVEL_TYPE" type="xsd:int""#),
            "schema LEVEL_TYPE: {xml}"
        );
        // [Measures] level
        assert!(
            xml.contains("<LEVEL_NAME>MeasuresLevel</LEVEL_NAME>"),
            "Measures level: {xml}"
        );
        assert!(
            xml.contains("<LEVEL_UNIQUE_NAME>[Measures].[MeasuresLevel]</LEVEL_UNIQUE_NAME>"),
            "Measures unique: {xml}"
        );
        assert!(
            xml.contains("<LEVEL_ORIGIN>6</LEVEL_ORIGIN>"),
            "Measures origin: {xml}"
        );
        // (All) level for Amount
        assert!(
            xml.contains("<LEVEL_UNIQUE_NAME>[Sales].[Amount].[(All)]</LEVEL_UNIQUE_NAME>"),
            "All level: {xml}"
        );
        assert!(
            xml.contains("<LEVEL_TYPE>1</LEVEL_TYPE>"),
            "All type: {xml}"
        );
        // Member level for Amount
        assert!(
            xml.contains("<LEVEL_UNIQUE_NAME>[Sales].[Amount].[Amount]</LEVEL_UNIQUE_NAME>"),
            "member level: {xml}"
        );
        assert!(
            xml.contains("<LEVEL_NUMBER>1</LEVEL_NUMBER>"),
            "member number: {xml}"
        );
        // SQL column names on member level
        assert!(xml.contains("NAME( [$Sales].[Amount] )"), "name sql: {xml}");
        assert!(xml.contains("KEY( [$Sales].[Amount] )"), "key sql: {xml}");
        // Both columns present
        assert!(xml.contains("[Sales].[Date]"), "Date hier: {xml}");
        assert!(
            xml.contains("<CATALOG_NAME>DemoModel</CATALOG_NAME>"),
            "catalog: {xml}"
        );
    }

    #[test]
    fn discover_measures_expanded_schema_and_rows() {
        let (xml, _) = discover_measures(None, "DemoModel", &system_measures());
        assert!(xml.contains("DiscoverResponse"), "envelope: {xml}");
        assert!(
            xml.contains(r#"name="NUMERIC_PRECISION" type="xsd:unsignedShort""#),
            "NUMERIC_PRECISION schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="EXPRESSION" type="xsd:string""#),
            "EXPRESSION schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="MEASURE_UNQUALIFIED_CAPTION" type="xsd:string""#),
            "UNQUALIFIED_CAPTION schema: {xml}"
        );
        assert!(
            xml.contains("<NUMERIC_PRECISION>65535</NUMERIC_PRECISION>"),
            "precision value: {xml}"
        );
        assert!(
            xml.contains("<NUMERIC_SCALE>-1</NUMERIC_SCALE>"),
            "scale value: {xml}"
        );
        assert!(
            xml.contains("<EXPRESSION>SUM(Sales[Amount])</EXPRESSION>"),
            "expression: {xml}"
        );
        assert!(
            xml.contains("<MEASURE_UNQUALIFIED_CAPTION>Total Amount</MEASURE_UNQUALIFIED_CAPTION>"),
            "unqualified caption: {xml}"
        );
        assert!(
            xml.contains("<MEASUREGROUP_NAME>Sales</MEASUREGROUP_NAME>"),
            "measuregroup is table name: {xml}"
        );
    }

    #[test]
    fn mdschema_properties_cell_type_returns_12_rows() {
        let (xml, _) = discover_mdschema_properties(None, "", &[], Some(2));
        assert!(xml.contains("DiscoverResponse"), "envelope: {xml}");
        assert!(
            xml.contains(r#"name="PROPERTY_TYPE" type="xsd:short""#),
            "schema PROPERTY_TYPE: {xml}"
        );
        assert!(
            xml.contains(r#"name="DATA_TYPE" type="xsd:unsignedShort""#),
            "schema DATA_TYPE: {xml}"
        );
        assert!(
            xml.contains("<PROPERTY_TYPE>2</PROPERTY_TYPE>"),
            "type value: {xml}"
        );
        assert!(
            xml.contains("<PROPERTY_NAME>VALUE</PROPERTY_NAME>"),
            "VALUE: {xml}"
        );
        assert!(
            xml.contains("<PROPERTY_NAME>FORMAT_STRING</PROPERTY_NAME>"),
            "FORMAT_STRING: {xml}"
        );
        assert!(
            xml.contains("<PROPERTY_NAME>FORMATTED_VALUE</PROPERTY_NAME>"),
            "FORMATTED_VALUE: {xml}"
        );
        assert!(
            xml.contains("<PROPERTY_NAME>CELL_ORDINAL</PROPERTY_NAME>"),
            "CELL_ORDINAL: {xml}"
        );
        assert!(
            xml.contains("<DATA_TYPE>12</DATA_TYPE>"),
            "VALUE data type: {xml}"
        );
        assert_eq!(
            xml.matches("<PROPERTY_NAME>").count(),
            12,
            "exactly 12 rows"
        );
    }

    #[test]
    fn mdschema_properties_other_type_returns_empty_rows() {
        let (xml, _) = discover_mdschema_properties(None, "", &[], Some(1));
        assert!(xml.contains("DiscoverResponse"), "envelope: {xml}");
        assert!(
            !xml.contains("<PROPERTY_NAME>"),
            "no rows for type 1: {xml}"
        );
        let (xml_none, _) = discover_mdschema_properties(None, "", &[], None);
        assert!(
            !xml_none.contains("<PROPERTY_NAME>"),
            "no rows when unrestricted: {xml_none}"
        );
    }

    // ── data layer tests ──────────────────────────────────────────────────────

    #[test]
    fn data_cubes_rows_columns_and_values() {
        let result = dmv_cubes_rows(&system_databases());
        assert_eq!(result.rows.len(), 1);
        let row = &result.rows[0];
        assert_eq!(col(row, "CUBE_NAME"), Some("Model"));
        assert_eq!(col(row, "BASE_CUBE_NAME"), Some("Model"));
        assert_eq!(col(row, "CUBE_CAPTION"), Some("Model"));
        assert_eq!(col(row, "DESCRIPTION"), Some(""));
        assert_eq!(col(row, "LAST_SCHEMA_UPDATE"), Some(FAKE_TIMESTAMP));
        assert_eq!(col(row, "LAST_DATA_UPDATE"), Some(FAKE_TIMESTAMP));
    }

    #[test]
    fn data_catalogs_rows_columns_and_values() {
        let result = dmv_catalogs_rows(&system_databases());
        assert_eq!(result.rows.len(), 1);
        let row = &result.rows[0];
        assert_eq!(col(row, "CATALOG_NAME"), Some("DemoModel"));
        assert_eq!(col(row, "COMPATIBILITY_LEVEL"), Some("1604"));
        assert!(col(row, "TYPE").is_none(), "TYPE must not be in rows");
        assert_eq!(col(row, "DATABASE_ID"), Some("DemoModel"));
        assert!(
            col(row, "DATE_MODIFIED").is_none(),
            "DATE_MODIFIED must not be in rows"
        );
    }

    #[test]
    fn data_measures_rows_both_measures_returned() {
        let result = dmv_measures_rows("DemoModel", &system_measures());
        assert_eq!(result.rows.len(), 2);
        assert_eq!(
            col(&result.rows[0], "MEASURE_CAPTION"),
            Some("Total Amount")
        );
        assert_eq!(col(&result.rows[0], "MEASURE_IS_VISIBLE"), Some("true"));
        assert_eq!(col(&result.rows[1], "MEASURE_CAPTION"), Some("Hidden"));
        assert_eq!(col(&result.rows[1], "MEASURE_IS_VISIBLE"), Some("false"));
    }

    #[test]
    fn data_dimensions_rows_all_tables_returned() {
        let result = dmv_dimensions_rows("DemoModel", &system_tables());
        assert_eq!(result.rows.len(), 3);
        assert_eq!(col(&result.rows[0], "DIMENSION_NAME"), Some("Measures"));
        assert_eq!(col(&result.rows[0], "DIMENSION_TYPE"), Some("2"));
        assert_eq!(
            col(&result.rows[0], "DEFAULT_HIERARCHY"),
            Some("[Measures]")
        );
        assert_eq!(col(&result.rows[1], "DIMENSION_NAME"), Some("Sales"));
        assert_eq!(
            col(&result.rows[1], "DIMENSION_UNIQUE_NAME"),
            Some("[Sales]")
        );
        assert_eq!(col(&result.rows[1], "DIMENSION_IS_VISIBLE"), Some("true"));
        assert_eq!(col(&result.rows[1], "DIMENSION_TYPE"), Some("3"));
        assert_eq!(col(&result.rows[2], "DIMENSION_NAME"), Some("Product"));
    }

    // ── XML layer tests ───────────────────────────────────────────────────────
    // render_dmv_result must produce the same structure as the original handlers.

    #[test]
    fn xml_cubes_schema_and_rows() {
        let xml = render_dmv_result(None, dmv_cubes_rows(&system_databases())).0;
        assert!(xml.contains("ExecuteResponse"), "wrong envelope: {xml}");
        assert!(
            xml.contains(r#"name="CUBE_NAME" type="xsd:string""#),
            "CUBE_NAME schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="LAST_DATA_UPDATE" type="xsd:dateTime""#),
            "LAST_DATA_UPDATE schema: {xml}"
        );
        assert!(
            xml.contains("<CUBE_NAME>Model</CUBE_NAME>"),
            "CUBE_NAME value: {xml}"
        );
        assert!(
            xml.contains("<BASE_CUBE_NAME>Model</BASE_CUBE_NAME>"),
            "BASE_CUBE_NAME value: {xml}"
        );
        assert!(
            xml.contains("<LAST_SCHEMA_UPDATE>2025-01-01T00:00:00</LAST_SCHEMA_UPDATE>"),
            "LAST_SCHEMA_UPDATE value: {xml}"
        );
        assert!(
            xml.contains("<LAST_DATA_UPDATE>2025-01-01T00:00:00</LAST_DATA_UPDATE>"),
            "LAST_DATA_UPDATE value: {xml}"
        );
    }

    #[test]
    fn xml_catalogs_schema_and_rows() {
        let xml = render_dmv_result(None, dmv_catalogs_rows(&system_databases())).0;
        assert!(xml.contains("ExecuteResponse"), "wrong envelope: {xml}");
        assert!(
            xml.contains(r#"name="DATE_MODIFIED" type="xsd:dateTime""#),
            "DATE_MODIFIED schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="COMPATIBILITY_LEVEL" type="xsd:int""#),
            "COMPATIBILITY_LEVEL schema: {xml}"
        );
        assert!(
            xml.contains("<CATALOG_NAME>DemoModel</CATALOG_NAME>"),
            "CATALOG_NAME value: {xml}"
        );
        assert!(
            xml.contains("<COMPATIBILITY_LEVEL>1604</COMPATIBILITY_LEVEL>"),
            "compat value: {xml}"
        );
        assert!(
            !xml.contains("<TYPE>"),
            "TYPE must not appear in rows: {xml}"
        );
        assert!(
            !xml.contains("<DATE_MODIFIED"),
            "DATE_MODIFIED must not appear in rows: {xml}"
        );
    }

    #[test]
    fn xml_measures_schema_and_rows() {
        let xml = render_dmv_result(None, dmv_measures_rows("DemoModel", &system_measures())).0;
        assert!(xml.contains("ExecuteResponse"), "wrong envelope: {xml}");
        assert!(
            xml.contains(r#"name="MEASURE_AGGREGATOR" type="xsd:int""#),
            "MEASURE_AGGREGATOR schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="MEASURE_IS_VISIBLE" type="xsd:boolean""#),
            "MEASURE_IS_VISIBLE schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="DATA_TYPE" type="xsd:unsignedShort""#),
            "DATA_TYPE schema: {xml}"
        );
        assert!(
            xml.contains("<MEASURE_CAPTION>Total Amount</MEASURE_CAPTION>"),
            "TotalAmount: {xml}"
        );
        assert!(
            xml.contains("<MEASURE_CAPTION>Hidden</MEASURE_CAPTION>"),
            "HiddenMeasure: {xml}"
        );
        assert!(
            xml.contains("<MEASURE_IS_VISIBLE>true</MEASURE_IS_VISIBLE>"),
            "visible flag: {xml}"
        );
        assert!(
            xml.contains("<MEASURE_IS_VISIBLE>false</MEASURE_IS_VISIBLE>"),
            "hidden flag: {xml}"
        );
    }

    #[test]
    fn xml_dimensions_schema_and_rows() {
        let xml = render_dmv_result(None, dmv_dimensions_rows("DemoModel", &system_tables())).0;
        assert!(xml.contains("ExecuteResponse"), "wrong envelope: {xml}");
        assert!(
            xml.contains(r#"name="DIMENSION_TYPE" type="xsd:short""#),
            "DIMENSION_TYPE schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="DIMENSION_IS_VISIBLE" type="xsd:boolean""#),
            "DIMENSION_IS_VISIBLE schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="DEFAULT_HIERARCHY" type="xsd:string""#),
            "DEFAULT_HIERARCHY schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="DIMENSION_ORDINAL" type="xsd:unsignedInt""#),
            "DIMENSION_ORDINAL schema: {xml}"
        );
        assert!(
            xml.contains("<DIMENSION_CAPTION>Measures</DIMENSION_CAPTION>"),
            "measures dim: {xml}"
        );
        assert!(
            xml.contains("<DIMENSION_CAPTION>Sales</DIMENSION_CAPTION>"),
            "sales dim: {xml}"
        );
        assert!(
            xml.contains("<DIMENSION_CAPTION>Product</DIMENSION_CAPTION>"),
            "product dim: {xml}"
        );
        assert!(
            xml.contains("<DEFAULT_HIERARCHY>[Measures]</DEFAULT_HIERARCHY>"),
            "measures default hierarchy: {xml}"
        );
        assert!(
            xml.contains("<DIMENSION_IS_VISIBLE>true</DIMENSION_IS_VISIBLE>"),
            "visible: {xml}"
        );
    }

    #[test]
    fn discover_dbschema_tables_returns_visible_tables_as_dimensions() {
        let mut tables = system_tables();
        tables.push(TableMeta {
            name: "HiddenTable".into(),
            columns: vec![],
            is_hidden: true,
            data_category: None,
            description: None,
        });
        let (xml, _) =
            discover_dbschema_tables(None, "DemoModel", &tables, FAKE_TIMESTAMP, FAKE_TIMESTAMP);
        assert!(
            xml.contains(r#"name="TABLE_OLAP_TYPE" type="xsd:string""#),
            "schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="TABLE_GUID" type="xsd:string""#),
            "TABLE_GUID schema: {xml}"
        );
        // Each visible table appears as MEASURE_GROUP (plain) and CUBE_DIMENSION ($-prefixed)
        assert!(
            xml.contains("<TABLE_NAME>Sales</TABLE_NAME>"),
            "plain sales: {xml}"
        );
        assert!(
            xml.contains("<TABLE_NAME>$Sales</TABLE_NAME>"),
            "dollar sales: {xml}"
        );
        assert!(
            xml.contains("<TABLE_OLAP_TYPE>MEASURE_GROUP</TABLE_OLAP_TYPE>"),
            "measure group: {xml}"
        );
        assert!(
            xml.contains("<TABLE_OLAP_TYPE>CUBE_DIMENSION</TABLE_OLAP_TYPE>"),
            "cube dimension: {xml}"
        );
        assert!(
            xml.contains("<TABLE_TYPE>SYSTEM TABLE</TABLE_TYPE>"),
            "system table type: {xml}"
        );
        assert!(
            xml.contains("<TABLE_CATALOG>DemoModel</TABLE_CATALOG>"),
            "catalog: {xml}"
        );
        assert!(
            xml.contains("<TABLE_SCHEMA>Model</TABLE_SCHEMA>"),
            "schema=Model: {xml}"
        );
        assert!(xml.contains("<DESCRIPTION/>"), "empty description: {xml}");
        assert!(
            xml.contains(&format!("<DATE_CREATED>{FAKE_TIMESTAMP}</DATE_CREATED>")),
            "date created: {xml}"
        );
        assert!(
            xml.contains(&format!("<DATE_MODIFIED>{FAKE_TIMESTAMP}</DATE_MODIFIED>")),
            "date modified: {xml}"
        );
        // $SYSTEM rows
        assert!(
            xml.contains("<TABLE_SCHEMA>$SYSTEM</TABLE_SCHEMA>"),
            "$SYSTEM rows: {xml}"
        );
        assert!(
            xml.contains("<TABLE_NAME>MDSCHEMA_CUBES</TABLE_NAME>"),
            "MDSCHEMA_CUBES system row: {xml}"
        );
        assert!(
            xml.contains("<TABLE_GUID>c8b522d8-5cf3-11ce-ade5-00aa0044773d</TABLE_GUID>"),
            "MDSCHEMA_CUBES guid: {xml}"
        );
        assert!(
            !xml.contains("HiddenTable"),
            "hidden table must be excluded: {xml}"
        );
    }

    #[test]
    fn discover_cubes_full_column_set() {
        let (xml, _) = discover_cubes(None, None, &system_databases());
        assert!(
            xml.contains(r#"name="PREFERRED_QUERY_PATTERNS" type="xsd:unsignedShort""#),
            "PREFERRED_QUERY_PATTERNS schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="BASE_CUBE_NAME" type="xsd:string""#),
            "BASE_CUBE_NAME in schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="CUBE_GUID" type="xsd:string""#),
            "CUBE_GUID in schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="SCHEMA_NAME" type="xsd:string""#),
            "SCHEMA_NAME in schema: {xml}"
        );
        assert!(
            xml.contains(r#"name="LAST_SCHEMA_UPDATE" type="xsd:dateTime""#),
            "LAST_SCHEMA_UPDATE in schema: {xml}"
        );
        assert!(
            xml.contains("<CATALOG_NAME>DemoModel</CATALOG_NAME>"),
            "CATALOG_NAME value: {xml}"
        );
        assert!(
            xml.contains("<CUBE_NAME>Model</CUBE_NAME>"),
            "CUBE_NAME value: {xml}"
        );
        assert!(
            !xml.contains("<BASE_CUBE_NAME>"),
            "BASE_CUBE_NAME must be absent from rows: {xml}"
        );
        assert!(
            xml.contains("<LAST_SCHEMA_UPDATE>2025-01-01T00:00:00</LAST_SCHEMA_UPDATE>"),
            "LAST_SCHEMA_UPDATE value: {xml}"
        );
        assert!(
            xml.contains("<LAST_DATA_UPDATE>2025-01-01T00:00:00</LAST_DATA_UPDATE>"),
            "LAST_DATA_UPDATE value: {xml}"
        );
        assert!(
            xml.contains("<CUBE_SOURCE>1</CUBE_SOURCE>"),
            "CUBE_SOURCE value: {xml}"
        );
        assert!(
            xml.contains("<PREFERRED_QUERY_PATTERNS>7</PREFERRED_QUERY_PATTERNS>"),
            "PREFERRED_QUERY_PATTERNS value: {xml}"
        );
    }

    #[test]
    fn discover_cubes_filtered_by_cube_source() {
        let (xml_match, _) = discover_cubes(None, Some(1), &system_databases());
        let (xml_no_match, _) = discover_cubes(None, Some(2), &system_databases());
        assert!(
            xml_match.contains("<CUBE_NAME>Model</CUBE_NAME>"),
            "source=1 should match: {xml_match}"
        );
        assert!(
            !xml_no_match.contains("<CUBE_NAME>"),
            "source=2 should not match: {xml_no_match}"
        );
    }

    // ── MDX cellset tests ─────────────────────────────────────────────────────

    fn color_axis() -> crate::mdx::AxisPlan {
        crate::mdx::AxisPlan {
            axis_id: 0,
            table: "vtest_product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "vtest_product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        }
    }

    fn q6_cell_props() -> Vec<String> {
        vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ]
    }

    #[test]
    fn cellset_q6_envelope_and_structure() {
        let axis = color_axis();
        let leaf: Vec<(String, String)> =
            vec![("Blue".into(), "40".into()), ("Red".into(), "110".into())];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("TotalAmount"),
            Some("150"),
            &leaf,
            &q6_cell_props(),
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        // envelope namespaces
        assert!(
            xml.contains(r#"xmlns:ns2="urn:schemas-microsoft-com:xml-analysis:mddataset""#),
            "ns2: {xml}"
        );
        assert!(
            xml.contains(r#"xmlns:xa="urn:schemas-microsoft-com:xml-analysis""#),
            "xa: {xml}"
        );
        assert!(
            xml.contains("<xa:ExecuteResponse>"),
            "ExecuteResponse: {xml}"
        );

        // OlapInfo
        assert!(
            xml.contains("<ns2:CubeName>Model</ns2:CubeName>"),
            "CubeName: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:HierarchyInfo name="[vtest_product].[Color]">"#),
            "HierarchyInfo: {xml}"
        );
        assert!(
            xml.contains(
                r#"<ns2:PARENT_UNIQUE_NAME name="[vtest_product].[Color].[PARENT_UNIQUE_NAME]""#
            ),
            "HierarchyInfo PARENT_UNIQUE_NAME: {xml}"
        );
        assert!(xml.contains(r#"<ns2:HIERARCHY_UNIQUE_NAME name="[vtest_product].[Color].[HIERARCHY_UNIQUE_NAME]""#), "HierarchyInfo HIER_UNAME: {xml}");
        assert!(
            xml.contains(r#"<ns2:Value name="VALUE" />"#),
            "CellInfo VALUE: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:FormatString name="FORMAT_STRING""#),
            "CellInfo FORMAT_STRING: {xml}"
        );

        // All member tuple
        assert!(
            xml.contains("<ns2:UName>[vtest_product].[Color].[All]</ns2:UName>"),
            "All UName: {xml}"
        );
        assert!(
            xml.contains("<ns2:LName>[vtest_product].[Color].[(All)]</ns2:LName>"),
            "All LName: {xml}"
        );
        assert!(
            xml.contains("<ns2:DisplayInfo>66536</ns2:DisplayInfo>"),
            "All DisplayInfo 66536: {xml}"
        );
        assert!(
            xml.contains(
                "<ns2:HIERARCHY_UNIQUE_NAME>[vtest_product].[Color]</ns2:HIERARCHY_UNIQUE_NAME>"
            ),
            "All HIER_UNAME: {xml}"
        );

        // Blue leaf tuple (non-last → DisplayInfo=0)
        assert!(
            xml.contains("<ns2:UName>[vtest_product].[Color].&amp;[Blue]</ns2:UName>"),
            "Blue UName: {xml}"
        );
        assert!(
            xml.contains("<ns2:Caption>Blue</ns2:Caption>"),
            "Blue caption: {xml}"
        );
        assert!(
            xml.contains("<ns2:LName>[vtest_product].[Color].[Color]</ns2:LName>"),
            "leaf LName: {xml}"
        );
        assert!(
            xml.contains("<ns2:DisplayInfo>0</ns2:DisplayInfo>"),
            "Blue DisplayInfo 0: {xml}"
        );
        assert!(
            xml.contains(
                "<ns2:PARENT_UNIQUE_NAME>[vtest_product].[Color].[All]</ns2:PARENT_UNIQUE_NAME>"
            ),
            "Blue PARENT: {xml}"
        );

        // Red leaf tuple (last → DisplayInfo=131072)
        assert!(
            xml.contains("<ns2:UName>[vtest_product].[Color].&amp;[Red]</ns2:UName>"),
            "Red UName: {xml}"
        );
        assert!(
            xml.contains("<ns2:DisplayInfo>131072</ns2:DisplayInfo>"),
            "Red DisplayInfo 131072: {xml}"
        );

        // CellData
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="0">"#),
            "Cell 0: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "total value 150: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="1">"#),
            "Cell 1: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "Blue value 40: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="2">"#),
            "Cell 2: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>110</ns2:Value>"),
            "Red value 110: {xml}"
        );
        assert!(
            !xml.contains("<ns2:FmtValue>"),
            "no FmtValue in cells for FORMAT_STRING property: {xml}"
        );
    }

    #[test]
    fn cellset_q8_all_only_empty_celldata() {
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "vtest_product".into(),
            hier: "ProductType".into(),
            level: "(All)".into(),
            dax_column: "vtest_product[ProductType]".into(),
            dim_props: vec!["MEMBER_TYPE".into()],
            include_all: true,
            all_only: true,
            second_hier: None,
        };
        let cell_props = vec!["CELL_ORDINAL".to_string()];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            None,
            None,
            &[],
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        // HierarchyInfo with MEMBER_TYPE
        assert!(
            xml.contains(r#"<ns2:HierarchyInfo name="[vtest_product].[ProductType]">"#),
            "HierarchyInfo ProductType: {xml}"
        );
        assert!(xml.contains(r#"<ns2:MEMBER_TYPE name="[vtest_product].[ProductType].[MEMBER_TYPE]" type="xs:int" />"#), "HierarchyInfo MEMBER_TYPE: {xml}");

        // CellInfo
        assert!(
            xml.contains(r#"<ns2:CellOrdinal name="CELL_ORDINAL" type="xs:unsignedInt" />"#),
            "CellOrdinal in CellInfo: {xml}"
        );

        // All-only member tuple — DisplayInfo=1000 (alone), MEMBER_TYPE=2
        assert!(
            xml.contains("<ns2:UName>[vtest_product].[ProductType].[All]</ns2:UName>"),
            "ProductType All UName: {xml}"
        );
        assert!(
            xml.contains("<ns2:LName>[vtest_product].[ProductType].[(All)]</ns2:LName>"),
            "ProductType All LName: {xml}"
        );
        assert!(
            xml.contains("<ns2:DisplayInfo>1000</ns2:DisplayInfo>"),
            "All-alone DisplayInfo 1000: {xml}"
        );
        assert!(
            xml.contains("<ns2:MEMBER_TYPE>2</ns2:MEMBER_TYPE>"),
            "All MEMBER_TYPE=2: {xml}"
        );

        // No leaf tuples, empty CellData
        assert!(!xml.contains("&amp;["), "no leaf UNames: {xml}");
        assert!(xml.contains("<ns2:CellData />"), "empty CellData: {xml}");
    }

    #[test]
    fn cellset_q9_key_filter_values() {
        let axis = color_axis();
        let leaf: Vec<(String, String)> =
            vec![("Blue".into(), "40".into()), ("Red".into(), "60".into())];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("TotalAmount"),
            Some("100"),
            &leaf,
            &q6_cell_props(),
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "total 100: {xml}"
        );
        assert!(xml.contains("<ns2:Value>40</ns2:Value>"), "Blue 40: {xml}");
        assert!(xml.contains("<ns2:Value>60</ns2:Value>"), "Red 60: {xml}");
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="0">"#),
            "ordinal 0: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="1">"#),
            "ordinal 1: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="2">"#),
            "ordinal 2: {xml}"
        );
    }

    // ── Generate/Ascendants XMLA shape — stage 3 ────────────────────────────

    fn make_color_axis() -> crate::mdx::AxisPlan {
        crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "MEMBER_TYPE".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        }
    }

    #[test]
    fn cellset_generate_ascendants_has_measures_axis1() {
        let axis = make_color_axis();
        let leaf_members = vec![("Blue".to_string(), "0".to_string())];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("cChildren"),
            Some("2"),
            &leaf_members,
            &[],
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
        );
        assert!(
            xml.contains(r#"<ns2:AxisInfo name="Axis1">"#),
            "Axis1 AxisInfo missing from AxesInfo: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Axis name="Axis1">"#),
            "Axis1 Axis element missing: {xml}"
        );
        assert!(
            xml.contains("[Measures].[cChildren]"),
            "measure member UName missing: {xml}"
        );
    }

    #[test]
    fn cellset_generate_ascendants_cell_info_not_empty() {
        let axis = make_color_axis();
        let leaf_members = vec![("Blue".to_string(), "0".to_string())];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("cChildren"),
            Some("2"),
            &leaf_members,
            &[],
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
        );
        let ci_start = xml.find("<ns2:CellInfo>").expect("no <ns2:CellInfo>");
        let ci_end = xml.find("</ns2:CellInfo>").expect("no </ns2:CellInfo>");
        let ci = &xml[ci_start + "<ns2:CellInfo>".len()..ci_end];
        assert!(
            !ci.trim().is_empty(),
            "CellInfo must not be empty when no CELL PROPERTIES given"
        );
        assert!(ci.contains("VALUE"), "CellInfo must declare VALUE");
    }

    #[test]
    fn cellset_generate_ascendants_correct_cell_values() {
        let axis = make_color_axis();
        let leaf_members = vec![("Blue".to_string(), "0".to_string())];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("cChildren"),
            Some("2"),
            &leaf_members,
            &[],
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="0">"#),
            "Cell 0 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>2</ns2:Value>"),
            "Cell 0 (All row) should have value 2: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:Cell CellOrdinal="1">"#),
            "Cell 1 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>0</ns2:Value>"),
            "Cell 1 (Blue row) should have value 0: {xml}"
        );
    }

    // ── DrilldownLevel / slicer-measure — stage 3 ───────────────────────────

    #[test]
    fn cellset_drilldown_slicer_measure_no_axis1() {
        // Measure in WHERE clause → show_measures_axis=false → no Axis1 in response.
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let leaf_members = vec![
            ("Blue".to_string(), "40".to_string()),
            ("Red".to_string(), "110".to_string()),
        ];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("CIMAmount"),
            Some("150"),
            &leaf_members,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        assert!(
            !xml.contains(r#"<ns2:AxisInfo name="Axis1">"#),
            "slicer measure must NOT produce AxisInfo Axis1: {xml}"
        );
        assert!(
            !xml.contains(r#"<ns2:Axis name="Axis1">"#),
            "slicer measure must NOT produce Axis1: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "All row total 150: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "Blue value 40: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>110</ns2:Value>"),
            "Red value 110: {xml}"
        );
        assert!(
            xml.contains(r#"<ns2:FormatString name="FORMAT_STRING""#),
            "CellInfo should declare FormatString for FORMAT_STRING property: {xml}"
        );
        assert!(
            !xml.contains("<ns2:FmtValue>"),
            "no FmtValue in cells for FORMAT_STRING property: {xml}"
        );
    }

    // ── CrossJoin(measures, dim) col-matrix XMLA shape — stage 3 ────────────

    #[test]
    fn cellset_crossjoin_measures_first_col_matrix_shape() {
        // CrossJoin({Amount,Qty}, Color) ON COLUMNS + ProductType ON ROWS.
        // COLUMNS axis: (Amount,All), (Amount,Red), (Amount,Blue), (Qty,All), (Qty,Red), (Qty,Blue)
        // ROWS axis: All, Widget, Thingy
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let row_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let matrix_measures = vec![
            (
                "CIMSummen af Amount".to_string(),
                "SUM('Sales'[Amount])".to_string(),
            ),
            (
                "CIMSummen af Quantity".to_string(),
                "SUM('Sales'[Quantity])".to_string(),
            ),
        ];
        // (col_val, row_val, [M0, M1])
        let cells: Vec<(String, String, Vec<Option<String>>)> = vec![
            (
                "Red".into(),
                "Widget".into(),
                vec![Some("60".into()), Some("6".into())],
            ),
            (
                "Red".into(),
                "Thingy".into(),
                vec![Some("50".into()), Some("5".into())],
            ),
            (
                "Blue".into(),
                "Widget".into(),
                vec![Some("40".into()), Some("4".into())],
            ),
        ];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset_col_matrix(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &matrix_measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
            false,
        );

        // Both measures appear in the COLUMNS axis
        assert!(
            xml.contains("[Measures].[CIMSummen af Amount]"),
            "Amount measure missing: {xml}"
        );
        assert!(
            xml.contains("[Measures].[CIMSummen af Quantity]"),
            "Quantity measure missing: {xml}"
        );

        // Color members on COLUMNS
        assert!(
            xml.contains("[Product].[Color].[All]"),
            "Color All missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Red]"),
            "Red missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Blue]"),
            "Blue missing: {xml}"
        );

        // ProductType members on ROWS
        assert!(
            xml.contains("[Product].[ProductType].[All]"),
            "ProductType All missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[ProductType].&amp;[Widget]"),
            "Widget missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[ProductType].&amp;[Thingy]"),
            "Thingy missing: {xml}"
        );

        // Cell values present
        assert!(
            xml.contains("<ns2:Value>60</ns2:Value>"),
            "60 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "50 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "40 missing: {xml}"
        );
        assert!(xml.contains("<ns2:Value>6</ns2:Value>"), "6 missing: {xml}");
        assert!(xml.contains("<ns2:Value>5</ns2:Value>"), "5 missing: {xml}");
        assert!(xml.contains("<ns2:Value>4</ns2:Value>"), "4 missing: {xml}");
    }

    // ── Col-matrix HierarchyInfo order — stage 3 ─────────────────────────────

    #[allow(clippy::type_complexity)]
    fn make_col_matrix_axes() -> (
        crate::mdx::AxisPlan,
        crate::mdx::AxisPlan,
        Vec<(String, String)>,
        Vec<(String, String, Vec<Option<String>>)>,
    ) {
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let row_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Quantity".to_string(), "SUM('Sales'[Quantity])".to_string()),
        ];
        let cells: Vec<(String, String, Vec<Option<String>>)> = vec![
            (
                "Red".into(),
                "Widget".into(),
                vec![Some("60".into()), Some("6".into())],
            ),
            (
                "Blue".into(),
                "Widget".into(),
                vec![Some("40".into()), Some("4".into())],
            ),
        ];
        (col_axis, row_axis, measures, cells)
    }

    #[test]
    fn cellset_col_matrix_measures_first_hierarchy_order() {
        // CrossJoin({measures}, dim) → [Measures] HierarchyInfo must come BEFORE [Product].[Color].
        let (col_axis, row_axis, measures, cells) = make_col_matrix_axes();
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_col_matrix(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
            false,
        );

        let pos_measures = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        let pos_color = xml
            .find(r#"HierarchyInfo name="[Product].[Color]""#)
            .expect("[Product].[Color] HierarchyInfo missing");
        assert!(
            pos_measures < pos_color,
            "[Measures] must appear before [Product].[Color] in Axis0 when measures_first=true\nXML: {xml}"
        );

        // First Members block in MembersLookup must be measures.
        let pos_ml = xml
            .find("<ns5:MembersLookup>")
            .expect("MembersLookup missing");
        let pos_amt_member = xml
            .find("[Measures].[Amount]")
            .expect("[Measures].[Amount] missing");
        let pos_color_all = xml
            .find("[Product].[Color].[All]")
            .expect("Color All missing");
        assert!(
            pos_amt_member > pos_ml && pos_amt_member < pos_color_all,
            "Measures Members block must precede Color Members block in MembersLookup: {xml}"
        );

        // Cell ordinal 0 = grand total of first measure (Amount), ordinal n_col_members = grand total of Quantity.
        // n_col_members = 1 (All) + 2 leaves (Red, Blue) = 3.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (grand total Quantity) missing: {xml}"
        );
    }

    #[test]
    fn cellset_col_matrix_dim_first_hierarchy_order() {
        // CrossJoin(dim, {measures}) → [Product].[Color] HierarchyInfo must come BEFORE [Measures].
        let (col_axis, row_axis, measures, cells) = make_col_matrix_axes();
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_col_matrix(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
            false,
        );

        let pos_color = xml
            .find(r#"HierarchyInfo name="[Product].[Color]""#)
            .expect("[Product].[Color] HierarchyInfo missing");
        let pos_measures = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        assert!(
            pos_color < pos_measures,
            "[Product].[Color] must appear before [Measures] in Axis0 when measures_first=false\nXML: {xml}"
        );

        // First Members block in MembersLookup must be col dim.
        let pos_ml = xml
            .find("<ns5:MembersLookup>")
            .expect("MembersLookup missing");
        let pos_color_all = xml
            .find("[Product].[Color].[All]")
            .expect("Color All missing");
        let pos_amt_member = xml
            .find("[Measures].[Amount]")
            .expect("[Measures].[Amount] missing");
        assert!(
            pos_color_all > pos_ml && pos_color_all < pos_amt_member,
            "Color Members block must precede Measures Members block in MembersLookup: {xml}"
        );

        // With dim_first: ordinal 0 = (All col, first measure), ordinal n_measures = (Red, first measure).
        // n_measures = 2, so ordinal 2 = (Red, Amount).
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="2""#),
            "ordinal 2 (Red+Amount col subtotal) missing: {xml}"
        );
    }

    // ── Row-matrix HierarchyInfo order — stage 3 ─────────────────────────────

    #[allow(clippy::type_complexity)]
    fn make_row_matrix_axes() -> (
        crate::mdx::AxisPlan,
        crate::mdx::AxisPlan,
        Vec<(String, String)>,
        Vec<(String, String, Vec<Option<String>>)>,
    ) {
        // col_axis = CrossJoin dim (ProductType, on MDX ROWS → axis in DaxTranslation)
        // row_axis = simple dim  (Color, on MDX COLUMNS → row_axis in DaxTranslation)
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let row_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Quantity".to_string(), "SUM('Sales'[Quantity])".to_string()),
        ];
        let cells: Vec<(String, String, Vec<Option<String>>)> = vec![
            (
                "Widget".into(),
                "Red".into(),
                vec![Some("60".into()), Some("6".into())],
            ),
            (
                "Widget".into(),
                "Blue".into(),
                vec![Some("40".into()), Some("4".into())],
            ),
        ];
        (col_axis, row_axis, measures, cells)
    }

    #[test]
    fn cellset_row_matrix_measures_first_hierarchy_order() {
        // CrossJoin({measures}, dim) ON ROWS → Axis0 = simple col dim, Axis1 = NormTupleSet.
        // [Measures] HierarchyInfo must appear in Axis1 BEFORE [Product].[ProductType].
        let (col_axis, row_axis, measures, cells) = make_row_matrix_axes();
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_col_matrix(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            true,
            true,
        );

        // Axis0 must have only ONE HierarchyInfo (simple col dim = Color).
        let pos_color_hi = xml
            .find(r#"HierarchyInfo name="[Product].[Color]""#)
            .expect("[Product].[Color] HierarchyInfo missing");
        assert!(
            !xml.contains(r#"AxisInfo name="Axis0"><ns2:HierarchyInfo name="[Measures]""#),
            "[Measures] must NOT be in Axis0 HierarchyInfo when matrix_on_rows=true: {xml}"
        );

        // Axis1 must have [Measures] BEFORE [Product].[ProductType] (measures_first=true).
        let pos_measures_hi = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        let pos_prodtype_hi = xml
            .find(r#"HierarchyInfo name="[Product].[ProductType]""#)
            .expect("[Product].[ProductType] HierarchyInfo missing");
        assert!(
            pos_measures_hi < pos_prodtype_hi,
            "[Measures] must appear before [Product].[ProductType] in Axis1: {xml}"
        );
        // Color must precede ProductType in document order (Color is in Axis0, ProductType in Axis1).
        assert!(
            pos_color_hi < pos_measures_hi,
            "Color Axis0 must precede Measures Axis1: {xml}"
        );

        // Axis0 must contain simple Tuples (Color), Axis1 must contain NormTupleSet.
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        let pos_axis1 = xml.find(r#"Axis name="Axis1""#).unwrap();
        let pos_nts = xml.find("<ns5:NormTupleSet>").unwrap();
        assert!(
            pos_nts > pos_axis1,
            "NormTupleSet must be inside Axis1, not Axis0: {xml}"
        );
        assert!(
            xml[pos_axis0..pos_axis1].contains("<ns2:Tuple>"),
            "Axis0 must contain simple Tuples: {xml}"
        );

        // Cell ordinal 0 = (Color All, Measures All × Amount) = grand total Amount.
        // With measures_first: axis1_pos(0,0)=0, n_axis0_members=3 (All+Red+Blue), ordinal=0+3*0=0.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        // Grand total Quantity: axis1_pos(0,1)=n_col_members=2, ordinal=0+3*2=6.
        assert!(
            xml.contains(r#"CellOrdinal="6""#),
            "ordinal 6 (grand total Quantity) missing: {xml}"
        );
    }

    #[test]
    fn cellset_row_matrix_dim_first_hierarchy_order() {
        // CrossJoin(dim, {measures}) ON ROWS → measures_first=false.
        // [Product].[ProductType] HierarchyInfo must appear BEFORE [Measures] in Axis1.
        let (col_axis, row_axis, measures, cells) = make_row_matrix_axes();
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_col_matrix(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
            true,
        );

        let pos_prodtype_hi = xml
            .find(r#"HierarchyInfo name="[Product].[ProductType]""#)
            .expect("[Product].[ProductType] HierarchyInfo missing");
        let pos_measures_hi = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        assert!(
            pos_prodtype_hi < pos_measures_hi,
            "[Product].[ProductType] must appear before [Measures] in Axis1 when measures_first=false: {xml}"
        );

        // With dim_first: axis1_pos(0,0)=0, ordinal=0+3*0=0. axis1_pos(0,1)=1, ordinal=0+3*1=3.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (grand total Quantity dim_first) missing: {xml}"
        );
    }

    // ── Measures-on-rows shape — stage 3 ────────────────────────────────────

    #[test]
    fn cellset_meas_on_rows_shape() {
        // ProductType ON COLUMNS, {Amount, Quantity} ON ROWS.
        // Axis0 = simple Tuples (ProductType), Axis1 = measure Tuples (no NormTupleSet).
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Quantity".to_string(), "SUM('Sales'[Quantity])".to_string()),
        ];
        let cells: Vec<(String, Vec<Option<String>>)> = vec![
            ("Widget".into(), vec![Some("60".into()), Some("6".into())]),
            ("Thingy".into(), vec![Some("40".into()), Some("4".into())]),
        ];
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_meas_on_rows(
            None,
            "Model",
            &col_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // OlapInfo: Axis0 = ProductType, Axis1 = Measures.
        let pos_axis0_hi = xml.find(r#"AxisInfo name="Axis0""#).unwrap();
        let pos_prodtype_hi = xml
            .find(r#"HierarchyInfo name="[Product].[ProductType]""#)
            .expect("[Product].[ProductType] HierarchyInfo missing");
        let pos_axis1_hi = xml.find(r#"AxisInfo name="Axis1""#).unwrap();
        let pos_meas_hi = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        assert!(
            pos_prodtype_hi > pos_axis0_hi && pos_prodtype_hi < pos_axis1_hi,
            "ProductType must be in Axis0 AxisInfo: {xml}"
        );
        assert!(
            pos_meas_hi > pos_axis1_hi,
            "[Measures] must be in Axis1 AxisInfo: {xml}"
        );

        // Axis0 must have simple Tuples (no NormTupleSet).
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        let pos_axis1 = xml.find(r#"Axis name="Axis1""#).unwrap();
        assert!(
            !xml[pos_axis0..pos_axis1].contains("NormTupleSet"),
            "Axis0 must NOT contain NormTupleSet: {xml}"
        );
        assert!(
            xml[pos_axis0..pos_axis1].contains("[Product].[ProductType].[All]"),
            "Axis0 must contain ProductType All tuple: {xml}"
        );

        // Axis1 must have measure Tuples.
        assert!(
            xml[pos_axis1..].contains("[Measures].[Amount]"),
            "Axis1 must contain Amount measure tuple: {xml}"
        );
        assert!(
            xml[pos_axis1..].contains("[Measures].[Quantity]"),
            "Axis1 must contain Quantity measure tuple: {xml}"
        );

        // Cell ordinals: n_col_members = 1(All) + 2(leaves) = 3.
        // (All, Amount)   → ordinal = 0 + 3*0 = 0
        // (Widget, Amount)→ ordinal = 1 + 3*0 = 1
        // (All, Quantity) → ordinal = 0 + 3*1 = 3
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 (grand total Amount) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 (Widget Amount) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (grand total Quantity) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="4""#),
            "ordinal 4 (Widget Quantity) missing: {xml}"
        );

        // Values present.
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "grand total Amount 100 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>60</ns2:Value>"),
            "Widget Amount 60 missing: {xml}"
        );
    }

    // ── Measures-on-cols shape — stage 3 ────────────────────────────────────

    #[test]
    fn cellset_meas_on_cols_shape() {
        // {Amount, Quantity} ON COLUMNS, Color ON ROWS.
        // Axis0 = measure Tuples, Axis1 = simple Color Tuples.
        let dim_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Quantity".to_string(), "SUM('Sales'[Quantity])".to_string()),
        ];
        // DAX returns (Color_value, M0, M1) — one row per leaf.
        let cells: Vec<(String, Vec<Option<String>>)> = vec![
            ("Red".into(), vec![Some("60".into()), Some("6".into())]),
            ("Blue".into(), vec![Some("40".into()), Some("4".into())]),
        ];
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_meas_on_cols(
            None,
            "Model",
            &dim_axis,
            &measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // OlapInfo: Axis0 = [Measures], Axis1 = [Product].[Color].
        let pos_axis0_hi = xml.find(r#"AxisInfo name="Axis0""#).unwrap();
        let pos_meas_hi = xml
            .find(r#"HierarchyInfo name="[Measures]""#)
            .expect("[Measures] HierarchyInfo missing");
        let pos_axis1_hi = xml.find(r#"AxisInfo name="Axis1""#).unwrap();
        let pos_color_hi = xml
            .find(r#"HierarchyInfo name="[Product].[Color]""#)
            .expect("[Product].[Color] HierarchyInfo missing");
        assert!(
            pos_meas_hi > pos_axis0_hi && pos_meas_hi < pos_axis1_hi,
            "[Measures] must be in Axis0 AxisInfo: {xml}"
        );
        assert!(
            pos_color_hi > pos_axis1_hi,
            "[Product].[Color] must be in Axis1 AxisInfo: {xml}"
        );

        // Axis0 must have measure Tuples (no NormTupleSet).
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        let pos_axis1 = xml.find(r#"Axis name="Axis1""#).unwrap();
        assert!(
            !xml[pos_axis0..pos_axis1].contains("NormTupleSet"),
            "Axis0 must NOT contain NormTupleSet: {xml}"
        );
        assert!(
            xml[pos_axis0..pos_axis1].contains("[Measures].[Amount]"),
            "Axis0 must contain Amount measure tuple: {xml}"
        );
        assert!(
            xml[pos_axis0..pos_axis1].contains("[Measures].[Quantity]"),
            "Axis0 must contain Quantity measure tuple: {xml}"
        );

        // Axis1 must have Color Tuples (All + leaves).
        assert!(
            xml[pos_axis1..].contains("[Product].[Color].[All]"),
            "Axis1 must contain Color All tuple: {xml}"
        );
        assert!(
            xml[pos_axis1..].contains("[Product].[Color].&amp;[Red]"),
            "Axis1 must contain Red leaf: {xml}"
        );

        // Cell ordinals: n_measures=2.
        // (Amount, All)  → ordinal = 0 + 2*0 = 0
        // (Quantity, All)→ ordinal = 1 + 2*0 = 1
        // (Amount, Red)  → ordinal = 0 + 2*1 = 2
        // (Quantity, Red)→ ordinal = 1 + 2*1 = 3
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 (grand total Amount) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 (grand total Quantity) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="2""#),
            "ordinal 2 (Red Amount) missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (Red Quantity) missing: {xml}"
        );

        // Values present.
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "grand total Amount 100 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>10</ns2:Value>"),
            "grand total Quantity 10 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>60</ns2:Value>"),
            "Red Amount 60 missing: {xml}"
        );
    }

    // ── Integer WHERE key filter XMLA shape — stage 3 ───────────────────────

    #[test]
    fn cellset_two_dim_integer_key_filter_correct_axes() {
        // WHERE ([Product].[ProductSK].&[2], ...) → only ProductSK=2 (Thingy, Red).
        // COLUMNS: All + Thingy.  ROWS: All + Red.  Cell value: 50.
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let row_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let cells: Vec<(String, String, Option<String>)> =
            vec![("Thingy".into(), "Red".into(), Some("50".into()))];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset_two_dim_axis_measure(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        assert!(
            xml.contains("[Product].[ProductType].[All]"),
            "ProductType All missing"
        );
        assert!(
            xml.contains("[Product].[ProductType].&amp;[Thingy]"),
            "Thingy missing"
        );
        assert!(
            !xml.contains("[Product].[ProductType].&amp;[Widget]"),
            "Widget should be absent"
        );

        assert!(xml.contains("[Product].[Color].[All]"), "Color All missing");
        assert!(xml.contains("[Product].[Color].&amp;[Red]"), "Red missing");
        assert!(
            !xml.contains("[Product].[Color].&amp;[Blue]"),
            "Blue should be absent"
        );

        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "cell value 50 missing: {xml}"
        );
    }

    // ── Two-dim-axis subquery FROM XMLA shape — stage 3 ─────────────────────

    #[test]
    fn cellset_two_dim_subquery_from_rows_axis_shows_red_only() {
        // ProductType ON COLUMNS, Color ON ROWS, subquery filter = Red only.
        // DAX returns only Red rows: (Widget,Red,60) and (Thingy,Red,50).
        // ROWS axis: All + Red. COLUMNS axis: All + Widget + Thingy.
        let col_axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "ProductType".into(),
            level: "ProductType".into(),
            dax_column: "Product[ProductType]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let row_axis = crate::mdx::AxisPlan {
            axis_id: 1,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        // Only Red rows come back from the filtered DAX
        let cells: Vec<(String, String, Option<String>)> = vec![
            ("Widget".into(), "Red".into(), Some("60".into())),
            ("Thingy".into(), "Red".into(), Some("50".into())),
        ];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset_two_dim_axis_measure(
            None,
            "Model",
            &col_axis,
            &row_axis,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // COLUMNS axis: All + Widget + Thingy
        assert!(
            xml.contains("[Product].[ProductType].[All]"),
            "ProductType All missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[ProductType].&amp;[Widget]"),
            "Widget missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[ProductType].&amp;[Thingy]"),
            "Thingy missing: {xml}"
        );

        // ROWS axis: All + Red (Blue excluded because filter returned only Red rows)
        assert!(
            xml.contains("[Product].[Color].[All]"),
            "Color All missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Red]"),
            "Red member missing: {xml}"
        );
        assert!(
            !xml.contains("[Product].[Color].&amp;[Blue]"),
            "Blue should be absent (filtered): {xml}"
        );

        // Cell values present
        assert!(
            xml.contains("<ns2:Value>60</ns2:Value>"),
            "Widget-Red=60 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "Thingy-Red=50 missing: {xml}"
        );
    }

    // ── Subquery FROM XMLA shape — stage 3 ───────────────────────────────────

    #[test]
    fn cellset_subquery_from_two_member_filter_axis_and_values() {
        // Subquery FROM restricts Color to Red+Blue (both values).
        // Axis0: All + Red + Blue (3 members).  Cells: 150, 110, 40.
        // Measure is in WHERE slicer → no Axis1.
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let leaf_members = vec![
            ("Red".to_string(), "110".to_string()),
            ("Blue".to_string(), "40".to_string()),
        ];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("CIMSummen af Amount"),
            Some("150"),
            &leaf_members,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        assert!(
            !xml.contains(r#"<ns2:AxisInfo name="Axis1">"#),
            "slicer measure must NOT produce AxisInfo Axis1: {xml}"
        );

        let member_count = xml.matches("<ns2:Member>").count();
        assert_eq!(
            member_count, 3,
            "Axis0 must have 3 members (All + Red + Blue), got {member_count}"
        );

        assert!(
            xml.contains("[Product].[Color].[All]"),
            "All member missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Red]"),
            "Red member missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Blue]"),
            "Blue member missing: {xml}"
        );

        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "All total 150 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>110</ns2:Value>"),
            "Red value 110 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "Blue value 40 missing: {xml}"
        );
    }

    #[test]
    fn cellset_subquery_from_red_filter_axis_and_values() {
        // Subquery FROM restricts Color to Red only.
        // Axis0: All + Red (2 members).  Both cells = 110 (Red total).
        // Measure is in WHERE slicer → no Axis1.
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let leaf_members = vec![("Red".to_string(), "110".to_string())];
        let cell_props: Vec<String> = vec![
            "VALUE".into(),
            "FORMAT_STRING".into(),
            "LANGUAGE".into(),
            "BACK_COLOR".into(),
            "FORE_COLOR".into(),
            "FONT_FLAGS".into(),
        ];
        let (xml, _) = execute_mdx_cellset(
            None,
            "Model",
            &axis,
            Some("CIMSummen af Amount"),
            Some("110"),
            &leaf_members,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        assert!(
            !xml.contains(r#"<ns2:AxisInfo name="Axis1">"#),
            "slicer measure must NOT produce AxisInfo Axis1: {xml}"
        );
        assert!(
            !xml.contains(r#"<ns2:Axis name="Axis1">"#),
            "slicer measure must NOT produce Axis1: {xml}"
        );

        let all_count = xml.matches("<ns2:Member>").count();
        assert_eq!(
            all_count, 2,
            "Axis0 must have 2 members (All + Red), got {all_count}: {xml}"
        );

        assert!(
            xml.contains("[Product].[Color].[All]"),
            "All member UName missing: {xml}"
        );
        assert!(
            xml.contains("[Product].[Color].&amp;[Red]"),
            "Red member UName missing: {xml}"
        );

        let cell0_pos = xml
            .find(r#"<ns2:Cell CellOrdinal="0">"#)
            .expect("Cell 0 missing");
        let cell1_pos = xml
            .find(r#"<ns2:Cell CellOrdinal="1">"#)
            .expect("Cell 1 missing");
        let cell0 = &xml[cell0_pos..cell1_pos];
        let rest = &xml[cell1_pos..];
        assert!(
            cell0.contains("<ns2:Value>110</ns2:Value>"),
            "Cell 0 (All) should be 110: {cell0}"
        );
        assert!(
            rest.contains("<ns2:Value>110</ns2:Value>"),
            "Cell 1 (Red) should be 110: {rest}"
        );
    }

    // ── Scalar response — stage 3 ─────────────────────────────────────────────

    #[test]
    fn cellset_scalar_single_value() {
        let cell_props: Vec<String> = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_scalar(
            None,
            "Model",
            Some("42"),
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // SlicerAxis is present but empty (no member tuple — matches Fabric behaviour).
        assert!(
            xml.contains(r#"Axis name="SlicerAxis""#),
            "SlicerAxis missing: {xml}"
        );
        assert!(
            !xml.contains("<ns2:Tuple>"),
            "SlicerAxis must be empty (no Tuple): {xml}"
        );
        // AxisInfo for SlicerAxis must be self-closing / empty.
        assert!(
            xml.contains(r#"AxisInfo name="SlicerAxis" />"#),
            "AxisInfo SlicerAxis must be empty: {xml}"
        );

        // No Axis0/Axis1 in Axes section.
        assert!(
            !xml.contains(r#"Axis name="Axis0""#),
            "must not have Axis0: {xml}"
        );
        assert!(
            !xml.contains(r#"Axis name="Axis1""#),
            "must not have Axis1: {xml}"
        );

        // Single cell at ordinal 0.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>42</ns2:Value>"),
            "value 42 missing: {xml}"
        );
    }

    // ── Measures-only-COLUMNS response — stage 3 ─────────────────────────────

    #[test]
    fn cellset_meas_only_cols_two_measures() {
        let matrix_measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Qty".to_string(), "SUM('Sales'[Qty])".to_string()),
        ];
        let values: Vec<Option<String>> = vec![Some("100".to_string()), Some("10".to_string())];
        let cell_props: Vec<String> = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_meas_only_cols(
            None,
            "Model",
            &matrix_measures,
            &values,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // OlapInfo: Axis0 = [Measures], no Axis1.
        assert!(
            xml.contains(r#"AxisInfo name="Axis0""#),
            "AxisInfo Axis0 missing: {xml}"
        );
        assert!(
            !xml.contains(r#"AxisInfo name="Axis1""#),
            "must not have AxisInfo Axis1: {xml}"
        );
        assert!(
            xml.contains(r#"AxisInfo name="SlicerAxis""#),
            "AxisInfo SlicerAxis missing: {xml}"
        );

        // Axis0 has one Tuple per measure.
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        assert!(
            xml[pos_axis0..].contains("[Measures].[Amount]"),
            "Amount tuple missing: {xml}"
        );
        assert!(
            xml[pos_axis0..].contains("[Measures].[Qty]"),
            "Qty tuple missing: {xml}"
        );

        // No dim (Axis1) in the Axes section.
        assert!(
            !xml.contains(r#"Axis name="Axis1""#),
            "must not have Axis1: {xml}"
        );

        // Cells: ordinal 0 = Amount, ordinal 1 = Qty.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 missing: {xml}"
        );
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "Amount=100 missing: {xml}"
        );
        assert!(
            xml.contains("<ns2:Value>10</ns2:Value>"),
            "Qty=10 missing: {xml}"
        );
    }

    #[test]
    fn cellset_single_axis_crossjoin_dim_first() {
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: None,
        };
        let matrix_measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Qty".to_string(), "SUM('Sales'[Qty])".to_string()),
        ];
        // Two leaf rows: Blue → (40, 4), Red → (110, 11)
        let cells: Vec<(String, Option<String>, Vec<Option<String>>)> = vec![
            (
                "Blue".into(),
                None,
                vec![Some("40".into()), Some("4".into())],
            ),
            (
                "Red".into(),
                None,
                vec![Some("110".into()), Some("11".into())],
            ),
        ];
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_single_axis_crossjoin(
            None,
            "Model",
            &axis,
            &matrix_measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        // OlapInfo: Axis0 has dim + measures HierarchyInfo, no Axis1.
        assert!(
            xml.contains(r#"AxisInfo name="Axis0""#),
            "AxisInfo Axis0 missing"
        );
        assert!(
            !xml.contains(r#"AxisInfo name="Axis1""#),
            "must not have AxisInfo Axis1"
        );
        assert!(
            xml.contains(r#"AxisInfo name="SlicerAxis""#),
            "AxisInfo SlicerAxis missing"
        );

        // Axis0 uses NormTupleSet.
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        assert!(
            xml[pos_axis0..].contains("NormTupleSet"),
            "Axis0 must use NormTupleSet"
        );

        // MembersLookup has Color members then Measures members.
        assert!(
            xml.contains("[Product].[Color].[All]"),
            "All member missing"
        );
        assert!(
            xml.contains("[Measures].[Amount]"),
            "Amount measure missing"
        );
        assert!(xml.contains("[Measures].[Qty]"), "Qty measure missing");

        // No Axis1 in the Axes section.
        assert!(!xml.contains(r#"Axis name="Axis1""#), "must not have Axis1");

        // Cell ordinals (dim-first): All×Amount=0, All×Qty=1, Blue×Amount=2, Blue×Qty=3, Red×Amount=4, Red×Qty=5.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 (All/Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 (All/Qty) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="2""#),
            "ordinal 2 (Blue/Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (Blue/Qty) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="4""#),
            "ordinal 4 (Red/Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="5""#),
            "ordinal 5 (Red/Qty) missing"
        );

        // Grand totals: Amount=150 (40+110), Qty=15 (4+11).
        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "grand total Amount=150 missing"
        );
        assert!(
            xml.contains("<ns2:Value>15</ns2:Value>"),
            "grand total Qty=15 missing"
        );

        // Leaf values.
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "Blue Amount=40 missing"
        );
        assert!(
            xml.contains("<ns2:Value>4</ns2:Value>"),
            "Blue Qty=4 missing"
        );
        assert!(
            xml.contains("<ns2:Value>110</ns2:Value>"),
            "Red Amount=110 missing"
        );
        assert!(
            xml.contains("<ns2:Value>11</ns2:Value>"),
            "Red Qty=11 missing"
        );
    }

    #[test]
    fn cellset_single_axis_two_hier_crossjoin_dim_first() {
        // CrossJoin(Hierarchize(DrilldownMember(CrossJoin(Color, ProductType), ...)), {Measures})
        // Everything on Axis0. Three hierarchies: Color, ProductType, Measures.
        let axis = crate::mdx::AxisPlan {
            axis_id: 0,
            table: "Product".into(),
            hier: "Color".into(),
            level: "Color".into(),
            dax_column: "Product[Color]".into(),
            dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
            include_all: true,
            all_only: false,
            second_hier: Some(crate::mdx::SecondHierPlan {
                table: "Product".into(),
                hier: "ProductType".into(),
                level: "ProductType".into(),
                dax_column: "Product[ProductType]".into(),
            }),
        };
        let matrix_measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Qty".to_string(), "SUM('Sales'[Qty])".to_string()),
        ];
        // DAX returns 3 (Color, ProductType) pairs — Blue×Widget, Red×Thingy, Red×Widget.
        let cells: Vec<(String, Option<String>, Vec<Option<String>>)> = vec![
            (
                "Blue".into(),
                Some("Widget".into()),
                vec![Some("50".into()), Some("5".into())],
            ),
            (
                "Red".into(),
                Some("Thingy".into()),
                vec![Some("40".into()), Some("4".into())],
            ),
            (
                "Red".into(),
                Some("Widget".into()),
                vec![Some("60".into()), Some("6".into())],
            ),
        ];
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_single_axis_crossjoin(
            None,
            "Model",
            &axis,
            &matrix_measures,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
            false,
        );

        // OlapInfo: Axis0 has Color + ProductType + Measures HierarchyInfo, no Axis1.
        assert!(
            xml.contains(r#"AxisInfo name="Axis0""#),
            "AxisInfo Axis0 missing"
        );
        assert!(
            !xml.contains(r#"AxisInfo name="Axis1""#),
            "must not have AxisInfo Axis1"
        );
        assert!(
            xml.contains(r#"HierarchyInfo name="[Product].[Color]""#),
            "Color HierarchyInfo missing"
        );
        assert!(
            xml.contains(r#"HierarchyInfo name="[Product].[ProductType]""#),
            "ProductType HierarchyInfo missing"
        );
        assert!(
            xml.contains(r#"HierarchyInfo name="[Measures]""#),
            "Measures HierarchyInfo missing"
        );

        // Axis0 uses NormTupleSet.
        let pos_axis0 = xml.find(r#"Axis name="Axis0""#).unwrap();
        assert!(
            xml[pos_axis0..].contains("NormTupleSet"),
            "Axis0 must use NormTupleSet"
        );

        // MembersLookup has all three member blocks.
        assert!(
            xml.contains("[Product].[Color].[All]"),
            "Color All member missing"
        );
        assert!(
            xml.contains("[Product].[ProductType].[All]"),
            "ProductType All member missing"
        );
        assert!(
            xml.contains("[Measures].[Amount]"),
            "Amount measure missing"
        );
        assert!(xml.contains("[Measures].[Qty]"), "Qty measure missing");

        // Grand totals: Amount = 50+40+60 = 150, Qty = 5+4+6 = 15.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 (grand total Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 (grand total Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "grand total Amount=150 missing"
        );
        assert!(
            xml.contains("<ns2:Value>15</ns2:Value>"),
            "grand total Qty=15 missing"
        );

        // H1 subtotals: Blue=50, Red=100. Ordinals 2,3 (Blue) and 6,7 (Red).
        assert!(
            xml.contains(r#"CellOrdinal="2""#),
            "ordinal 2 (Blue subtotal Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (Blue subtotal Qty) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="6""#),
            "ordinal 6 (Red subtotal Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="7""#),
            "ordinal 7 (Red subtotal Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "Blue subtotal Amount=50 missing"
        );
        assert!(
            xml.contains("<ns2:Value>5</ns2:Value>"),
            "Blue subtotal Qty=5 missing"
        );
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "Red subtotal Amount=100 missing"
        );
        assert!(
            xml.contains("<ns2:Value>10</ns2:Value>"),
            "Red subtotal Qty=10 missing"
        );

        // H1×H2 leaf values. Ordinals 4,5 (Blue×Widget), 8,9 (Red×Thingy), 10,11 (Red×Widget).
        assert!(
            xml.contains(r#"CellOrdinal="4""#),
            "ordinal 4 (Blue×Widget Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="11""#),
            "ordinal 11 (Red×Widget Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>60</ns2:Value>"),
            "Red×Widget Amount=60 missing"
        );
        assert!(
            xml.contains("<ns2:Value>40</ns2:Value>"),
            "Red×Thingy Amount=40 missing"
        );

        // No Axis1 in the Axes section.
        assert!(
            !xml.contains(r#"Axis name="Axis1""#),
            "must not have Axis1 in axes"
        );
    }

    #[test]
    fn cellset_single_axis_multi_dim_crossjoin() {
        // CrossJoin(CrossJoin(Color, {Measures}), ProductType) ON COLUMNS.
        // measures_position = 1: Color, Measures, ProductType.
        // Data: Blue×Thingy (Amount=50,Qty=5), Red×Widget (Amount=100,Qty=10).
        let dims = vec![
            crate::mdx::AxisPlan {
                axis_id: 0,
                table: "Product".into(),
                hier: "Color".into(),
                level: "Color".into(),
                dax_column: "Product[Color]".into(),
                dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
                include_all: true,
                all_only: false,
                second_hier: None,
            },
            crate::mdx::AxisPlan {
                axis_id: 0,
                table: "Product".into(),
                hier: "ProductType".into(),
                level: "ProductType".into(),
                dax_column: "Product[ProductType]".into(),
                dim_props: vec!["PARENT_UNIQUE_NAME".into(), "HIERARCHY_UNIQUE_NAME".into()],
                include_all: true,
                all_only: false,
                second_hier: None,
            },
        ];
        let measures = vec![
            ("Amount".to_string(), "SUM('Sales'[Amount])".to_string()),
            ("Qty".to_string(), "SUM('Sales'[Qty])".to_string()),
        ];
        // Each row: [Color, ProductType, Amount, Qty]
        let cells: Vec<Vec<Option<String>>> = vec![
            vec![
                Some("Blue".into()),
                Some("Thingy".into()),
                Some("50".into()),
                Some("5".into()),
            ],
            vec![
                Some("Red".into()),
                Some("Widget".into()),
                Some("100".into()),
                Some("10".into()),
            ],
        ];
        let cell_props = vec!["VALUE".into()];
        let (xml, _) = execute_mdx_cellset_single_axis_multi_dim_crossjoin(
            None,
            "Model",
            &dims,
            &measures,
            1,
            &cells,
            &cell_props,
            FAKE_TIMESTAMP,
            FAKE_TIMESTAMP,
        );

        // Three HierarchyInfo blocks in tuple order: Color, Measures, ProductType.
        assert!(
            xml.contains(r#"HierarchyInfo name="[Product].[Color]""#),
            "Color HI missing"
        );
        assert!(
            xml.contains(r#"HierarchyInfo name="[Measures]""#),
            "Measures HI missing"
        );
        assert!(
            xml.contains(r#"HierarchyInfo name="[Product].[ProductType]""#),
            "ProductType HI missing"
        );

        // Axis0 uses NormTupleSet.
        let pos = xml.find(r#"Axis name="Axis0""#).unwrap();
        assert!(
            xml[pos..].contains("NormTupleSet"),
            "Axis0 must use NormTupleSet"
        );

        // MembersLookup has all three member blocks.
        assert!(xml.contains("[Product].[Color].[All]"), "Color All missing");
        assert!(
            xml.contains("[Measures].[Amount]"),
            "Amount measure missing"
        );
        assert!(xml.contains("[Measures].[Qty]"), "Qty measure missing");
        assert!(
            xml.contains("[Product].[ProductType].[All]"),
            "ProductType All missing"
        );

        // Slot sizes: Color=[All,Blue,Red]=3, Measures=2, ProductType=[All,Thingy,Widget]=3.
        // Strides: Color → 2×3=6, Measures → 3, ProductType → 1.
        // Grand total ordinals: (0,0,0)=0, (0,1,0)=3.
        // Grand total Amount = 50+100 = 150, Qty = 5+10 = 15.
        assert!(
            xml.contains(r#"CellOrdinal="0""#),
            "ordinal 0 (grand total Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="3""#),
            "ordinal 3 (grand total Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>150</ns2:Value>"),
            "grand total Amount=150 missing"
        );
        assert!(
            xml.contains("<ns2:Value>15</ns2:Value>"),
            "grand total Qty=15 missing"
        );

        // Blue subtotal: ordinal (1,0,0) = 1*6+0+0 = 6, Qty (1,1,0) = 6+3 = 9.
        assert!(
            xml.contains(r#"CellOrdinal="6""#),
            "ordinal 6 (Blue subtotal Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="9""#),
            "ordinal 9 (Blue subtotal Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "Blue subtotal Amount=50 missing"
        );
        assert!(
            xml.contains("<ns2:Value>5</ns2:Value>"),
            "Blue subtotal Qty=5 missing"
        );

        // Red subtotal: ordinal (2,0,0) = 2*6 = 12, Qty (2,1,0) = 12+3 = 15.
        assert!(
            xml.contains(r#"CellOrdinal="12""#),
            "ordinal 12 (Red subtotal Amount) missing"
        );
        assert!(
            xml.contains(r#"CellOrdinal="15""#),
            "ordinal 15 (Red subtotal Qty) missing"
        );
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "Red subtotal Amount=100 missing"
        );
        assert!(
            xml.contains("<ns2:Value>10</ns2:Value>"),
            "Red subtotal Qty=10 missing"
        );

        // Thingy subtotal (All-Color × Amount × Thingy): ordinal (0,0,1) = 0+0+1 = 1.
        assert!(
            xml.contains(r#"CellOrdinal="1""#),
            "ordinal 1 (Thingy subtotal Amount) missing"
        );
        assert!(
            xml.contains("<ns2:Value>50</ns2:Value>"),
            "Thingy subtotal Amount=50 missing"
        );

        // Blue×Thingy leaf: ordinal (1,0,1) = 6+0+1 = 7.
        assert!(
            xml.contains(r#"CellOrdinal="7""#),
            "ordinal 7 (Blue×Thingy Amount) missing"
        );

        // Red×Widget leaf: ordinal (2,0,2) = 12+0+2 = 14.
        assert!(
            xml.contains(r#"CellOrdinal="14""#),
            "ordinal 14 (Red×Widget Amount) missing"
        );
        assert!(
            xml.contains("<ns2:Value>100</ns2:Value>"),
            "Red×Widget Amount=100 missing"
        );

        // No Axis1.
        assert!(!xml.contains(r#"Axis name="Axis1""#), "must not have Axis1");
    }
}
