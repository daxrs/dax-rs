use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use uuid::Uuid;

use crate::server::config::ServerConfig;
use crate::server::provider::{
    ColumnMeta, DatabaseMeta, MeasureMeta, ModelMeta, QueryResult, RelationshipMeta, TableMeta,
};

use super::xml_util::{
    execute_xml, make_tabular_schema, rowset, xml_escape_attr, xml_escape_value,
};

type Row = Vec<(String, String)>;

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
        let schema = make_tabular_schema(
            &result
                .columns
                .iter()
                .map(|(name, xsd)| (name.as_str(), Some(xsd.as_str())))
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
}
