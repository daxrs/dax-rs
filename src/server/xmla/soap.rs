use std::collections::HashMap;

use quick_xml::de::from_str;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Envelope {
    #[serde(rename = "Header")]
    pub header: Option<Header>,
    #[serde(rename = "Body")]
    pub body: Body,
}

#[derive(Debug, Deserialize)]
pub struct Header {
    #[serde(rename = "Session")]
    pub session: Option<Session>,
    #[serde(rename = "BeginSession")]
    pub begin_session: Option<BeginSession>,
    #[serde(rename = "EndSession")]
    pub end_session: Option<EndSession>,
}

#[derive(Debug, Deserialize)]
pub struct EndSession {
    #[serde(rename = "@SessionId")]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct BeginSession {}

#[derive(Debug, Deserialize)]
pub struct Session {
    #[serde(rename = "@SessionId")]
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
pub struct Body {
    #[serde(rename = "Discover")]
    pub discover: Option<Discover>,
    #[serde(rename = "Execute")]
    pub execute: Option<Execute>,
}

#[derive(Debug, Deserialize)]
pub struct Discover {
    #[serde(rename = "RequestType")]
    pub request_type: String,
    #[serde(rename = "Restrictions")]
    pub restrictions: Option<Restrictions>,
    #[serde(rename = "Properties")]
    pub properties: Option<Properties>,
}

#[derive(Debug, Deserialize)]
pub struct Restrictions {
    #[serde(rename = "RestrictionList")]
    pub list: Option<RestrictionList>,
}

#[derive(Debug, Deserialize)]
pub struct RestrictionList {
    #[serde(rename = "PropertyName")]
    pub property_name: Option<PropertyNameValues>,
    #[serde(rename = "CUBE_SOURCE")]
    pub cube_source: Option<String>,
    #[serde(rename = "ObjectExpansion")]
    pub object_expansion: Option<String>,
    #[serde(rename = "CATALOG_NAME")]
    pub catalog_name: Option<String>,
    #[serde(rename = "DATABASE_ID")]
    pub database_id: Option<String>,
    #[serde(rename = "VERSION")]
    pub version: Option<String>,
    #[serde(rename = "ORIGIN")]
    pub origin: Option<String>,
    #[serde(rename = "PROPERTY_TYPE")]
    pub property_type: Option<String>,
    #[serde(rename = "SchemaName")]
    pub schema_name: Option<String>,
    #[serde(rename = "MEMBER_UNIQUE_NAME")]
    pub member_unique_name: Option<String>,
    #[serde(rename = "TREE_OP")]
    pub tree_op: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PropertyNameValues {
    #[serde(rename = "Value", default)]
    pub values: Vec<String>,
    #[serde(rename = "$text")]
    pub text_value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Properties {
    #[serde(rename = "PropertyList")]
    pub list: Option<PropertyList>,
}

#[derive(Debug, Deserialize)]
pub struct PropertyList {
    #[serde(rename = "Catalog")]
    pub catalog: Option<String>,
    #[serde(rename = "ExecutionMetrics")]
    pub execution_metrics: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Execute {
    #[serde(rename = "Command")]
    pub command: Option<Command>,
    #[serde(rename = "Properties")]
    pub properties: Option<Properties>,
    #[serde(rename = "Parameters")]
    pub parameters: Option<ExecuteParameters>,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteParameters {
    #[serde(rename = "Parameter", default)]
    pub list: Vec<ExecuteParameter>,
}

#[derive(Debug, Deserialize)]
pub struct ExecuteParameter {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Value")]
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct Command {
    #[serde(rename = "Statement")]
    pub statement: Option<String>,
}

impl Envelope {
    pub fn parse(xml: &str) -> Result<Self, quick_xml::DeError> {
        from_str(xml)
    }

    pub fn session_id(&self) -> Option<&str> {
        let h = self.header.as_ref()?;
        if let Some(s) = &h.session {
            return Some(s.session_id.as_str());
        }
        if let Some(e) = &h.end_session {
            return Some(e.session_id.as_str());
        }
        None
    }

    pub fn has_begin_session(&self) -> bool {
        self.header
            .as_ref()
            .and_then(|h| h.begin_session.as_ref())
            .is_some()
    }
}

impl Execute {
    pub fn is_session_management(&self) -> bool {
        self.command.as_ref().is_none_or(|c| c.statement.is_none())
    }

    pub fn statement(&self) -> Option<&str> {
        self.command.as_ref()?.statement.as_deref()
    }

    /// The catalog (database) name from `<Properties><PropertyList><Catalog>`.
    pub fn catalog(&self) -> Option<&str> {
        self.properties.as_ref()?.list.as_ref()?.catalog.as_deref()
    }

    pub fn parameters(&self) -> HashMap<String, String> {
        self.parameters
            .as_ref()
            .map(|p| {
                p.list
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn wants_execution_metrics(&self) -> bool {
        self.properties
            .as_ref()
            .and_then(|p| p.list.as_ref())
            .and_then(|l| l.execution_metrics.as_deref())
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }
}

impl Discover {
    pub fn cube_source_restriction(&self) -> Option<u16> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .cube_source
            .as_ref()?
            .parse()
            .ok()
    }

    pub fn object_expansion_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .object_expansion
            .as_deref()
    }

    pub fn property_name_restriction(&self) -> Option<Vec<String>> {
        let p = self
            .restrictions
            .as_ref()?
            .list
            .as_ref()?
            .property_name
            .as_ref()?;
        if !p.values.is_empty() {
            return Some(p.values.clone());
        }
        if let Some(text) = &p.text_value {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return Some(vec![trimmed]);
            }
        }
        None
    }

    /// `CATALOG_NAME` restriction — scopes MDSCHEMA_* / DBSCHEMA_* results to one database.
    pub fn catalog_name_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .catalog_name
            .as_deref()
    }

    /// `DATABASE_ID` restriction — alternative to CATALOG_NAME used in some clients.
    pub fn database_id_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .database_id
            .as_deref()
    }

    /// `VERSION` restriction from DISCOVER_CSDL_METADATA requests.
    pub fn csdl_version_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .version
            .as_deref()
    }

    /// `ORIGIN` restriction from MDSCHEMA_FUNCTIONS requests (3 = table, 4 = scalar).
    pub fn origin_restriction(&self) -> Option<u32> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .origin
            .as_ref()?
            .parse()
            .ok()
    }

    /// `PROPERTY_TYPE` restriction from MDSCHEMA_PROPERTIES requests.
    pub fn property_type_restriction(&self) -> Option<u16> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .property_type
            .as_ref()?
            .parse()
            .ok()
    }

    /// `SchemaName` restriction from DISCOVER_SCHEMA_ROWSETS requests.
    pub fn schema_name_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .schema_name
            .as_deref()
    }

    /// `MEMBER_UNIQUE_NAME` restriction from MDSCHEMA_MEMBERS requests.
    pub fn member_unique_name_restriction(&self) -> Option<&str> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .member_unique_name
            .as_deref()
    }

    /// `TREE_OP` restriction from MDSCHEMA_MEMBERS requests (8 = SELF).
    pub fn tree_op_restriction(&self) -> Option<u32> {
        self.restrictions
            .as_ref()?
            .list
            .as_ref()?
            .tree_op
            .as_ref()?
            .parse()
            .ok()
    }

    /// The catalog (database) name from `<Properties><PropertyList><Catalog>`.
    pub fn catalog(&self) -> Option<&str> {
        self.properties.as_ref()?.list.as_ref()?.catalog.as_deref()
    }

    /// Resolves the target database name, preferring the restriction over the property.
    /// Clients that scope a schema query send CATALOG_NAME; clients that set context
    /// send it in PropertyList.Catalog.
    pub fn resolved_catalog(&self) -> Option<&str> {
        self.catalog_name_restriction()
            .or_else(|| self.database_id_restriction())
            .or_else(|| self.catalog())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const DISCOVER_PROPS: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Header><Version xmlns="http://schemas.microsoft.com/analysisservices/2008/engine/100" Sequence="926"/></soap:Header><soap:Body><Discover xmlns="urn:schemas-microsoft-com:xml-analysis"><RequestType>DISCOVER_PROPERTIES</RequestType><Restrictions><RestrictionList><PropertyName><Value>DbpropMsmdSubqueries</Value><Value>DbpropMsmdOptimizeResponse</Value><Value>DbpropMsmdActivityID</Value></PropertyName></RestrictionList></Restrictions><Properties><PropertyList/></Properties></Discover></soap:Body></soap:Envelope>"#;

    #[test]
    fn parses_discover_properties() {
        let env = Envelope::parse(DISCOVER_PROPS).unwrap();
        assert!(env.header.is_some());
        assert!(env.session_id().is_none());
        let disc = env.body.discover.as_ref().unwrap();
        assert_eq!(disc.request_type, "DISCOVER_PROPERTIES");
        let restriction = disc.property_name_restriction().unwrap();
        assert_eq!(
            restriction,
            [
                "DbpropMsmdSubqueries",
                "DbpropMsmdOptimizeResponse",
                "DbpropMsmdActivityID"
            ]
        );
    }

    const DISCOVER_MEASURES_WITH_CATALOG: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Discover xmlns="urn:schemas-microsoft-com:xml-analysis"><RequestType>MDSCHEMA_MEASURES</RequestType><Restrictions><RestrictionList><CATALOG_NAME>SalesModel</CATALOG_NAME></RestrictionList></Restrictions><Properties><PropertyList><Catalog>SalesModel</Catalog></PropertyList></Properties></Discover></soap:Body></soap:Envelope>"#;

    #[test]
    fn parses_catalog_name_restriction() {
        let env = Envelope::parse(DISCOVER_MEASURES_WITH_CATALOG).unwrap();
        let disc = env.body.discover.as_ref().unwrap();
        assert_eq!(disc.catalog_name_restriction(), Some("SalesModel"));
        assert_eq!(disc.catalog(), Some("SalesModel"));
        assert_eq!(disc.resolved_catalog(), Some("SalesModel"));
    }

    const EXECUTE_WITH_CATALOG: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Execute xmlns="urn:schemas-microsoft-com:xml-analysis"><Command><Statement>EVALUATE Sales</Statement></Command><Properties><PropertyList><Catalog>SalesModel</Catalog></PropertyList></Properties></Execute></soap:Body></soap:Envelope>"#;

    #[test]
    fn parses_execute_catalog() {
        let env = Envelope::parse(EXECUTE_WITH_CATALOG).unwrap();
        let exec = env.body.execute.as_ref().unwrap();
        assert_eq!(exec.statement(), Some("EVALUATE Sales"));
        assert_eq!(exec.catalog(), Some("SalesModel"));
    }

    const DISCOVER_XML_METADATA_WITH_DATABASE_ID: &str = r#"<soap:Envelope xmlns:soap="http://schemas.xmlsoap.org/soap/envelope/"><soap:Body><Discover xmlns="urn:schemas-microsoft-com:xml-analysis"><RequestType>DISCOVER_XML_METADATA</RequestType><Restrictions><RestrictionList><DATABASE_ID>FinanceModel</DATABASE_ID><ObjectExpansion>ExpandFull</ObjectExpansion></RestrictionList></Restrictions><Properties><PropertyList/></Properties></Discover></soap:Body></soap:Envelope>"#;

    #[test]
    fn parses_database_id_restriction() {
        let env = Envelope::parse(DISCOVER_XML_METADATA_WITH_DATABASE_ID).unwrap();
        let disc = env.body.discover.as_ref().unwrap();
        assert_eq!(disc.database_id_restriction(), Some("FinanceModel"));
        assert_eq!(disc.object_expansion_restriction(), Some("ExpandFull"));
        assert_eq!(disc.resolved_catalog(), Some("FinanceModel"));
    }
}
