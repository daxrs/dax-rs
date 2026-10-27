use serde::{de, Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnDataCategory {
    Address,
    City,
    Continent,
    Country,
    County,
    Latitude,
    Longitude,
    Place,
    PostalCode,
    StateOrProvince,
    WebUrl,
    ImageUrl,
    Barcode,
    PhoneNumber,
    Organization,
    FaceUri,
    Other(String),
}

impl ColumnDataCategory {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Address => "Address",
            Self::City => "City",
            Self::Continent => "Continent",
            Self::Country => "Country",
            Self::County => "County",
            Self::Latitude => "Latitude",
            Self::Longitude => "Longitude",
            Self::Place => "Place",
            Self::PostalCode => "PostalCode",
            Self::StateOrProvince => "StateOrProvince",
            Self::WebUrl => "WebUrl",
            Self::ImageUrl => "ImageUrl",
            Self::Barcode => "Barcode",
            Self::PhoneNumber => "PhoneNumber",
            Self::Organization => "Organization",
            Self::FaceUri => "FaceUri",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for ColumnDataCategory {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "Address" => Self::Address,
            "City" => Self::City,
            "Continent" => Self::Continent,
            "Country" => Self::Country,
            "County" => Self::County,
            "Latitude" => Self::Latitude,
            "Longitude" => Self::Longitude,
            "Place" => Self::Place,
            "PostalCode" => Self::PostalCode,
            "StateOrProvince" => Self::StateOrProvince,
            "WebUrl" => Self::WebUrl,
            "ImageUrl" => Self::ImageUrl,
            "Barcode" => Self::Barcode,
            "PhoneNumber" => Self::PhoneNumber,
            "Organization" => Self::Organization,
            "FaceUri" => Self::FaceUri,
            other => Self::Other(other.to_string()),
        })
    }
}

impl Serialize for ColumnDataCategory {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum TableDataCategory {
    Time,
    Other(String),
}

impl TableDataCategory {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Time => "Time",
            Self::Other(s) => s.as_str(),
        }
    }
}

impl<'de> Deserialize<'de> for TableDataCategory {
    fn deserialize<D: de::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(match s.as_str() {
            "Time" => Self::Time,
            other => Self::Other(other.to_string()),
        })
    }
}

impl Serialize for TableDataCategory {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct TmslModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub tables: Vec<Table>,
    pub relationships: Vec<Relationship>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub culture: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    #[serde(
        rename = "compatibilityLevel",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub compatibility_level: Option<u32>,
    #[serde(
        rename = "defaultMode",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub default_mode: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Table {
    pub name: String,
    #[serde(rename = "dataSource")]
    pub data_source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub measures: Vec<Measure>,
    #[serde(rename = "isHidden", default, skip_serializing_if = "is_false")]
    pub is_hidden: bool,
    #[serde(
        rename = "dataCategory",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_category: Option<TableDataCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Column {
    pub name: String,

    #[serde(rename = "dataType")]
    pub data_type: DaxDataType,

    #[serde(
        rename = "summarizeBy",
        default,
        skip_serializing_if = "SummarizeBy::is_none"
    )]
    pub summarize_by: SummarizeBy,

    #[serde(rename = "isHidden", default, skip_serializing_if = "is_false")]
    pub is_hidden: bool,

    #[serde(
        rename = "formatString",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub format_string: Option<String>,

    #[serde(
        rename = "displayFolder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_folder: Option<String>,

    #[serde(
        rename = "dataCategory",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub data_category: Option<ColumnDataCategory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(
        rename = "sortByColumn",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub sort_by_column: Option<String>,
    #[serde(rename = "isKey", default, skip_serializing_if = "is_false")]
    pub is_key: bool,
    #[serde(
        rename = "isNullable",
        default = "default_true",
        skip_serializing_if = "is_true"
    )]
    pub is_nullable: bool,
    #[serde(rename = "isUnique", default, skip_serializing_if = "is_false")]
    pub is_unique: bool,
}

fn default_true() -> bool {
    true
}
fn is_false(b: &bool) -> bool {
    !b
}
fn is_true(b: &bool) -> bool {
    *b
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SummarizeBy {
    #[default]
    None,
    Sum,
    Min,
    Max,
    Count,
    Average,
    DistinctCount,
}

impl SummarizeBy {
    fn is_none(&self) -> bool {
        matches!(self, SummarizeBy::None)
    }
}

impl SummarizeBy {
    pub fn as_csdl_str(&self) -> &str {
        match self {
            SummarizeBy::None => "None",
            SummarizeBy::Sum => "Sum",
            SummarizeBy::Min => "Min",
            SummarizeBy::Max => "Max",
            SummarizeBy::Count => "Count",
            SummarizeBy::Average => "Average",
            SummarizeBy::DistinctCount => "DistinctCount",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Measure {
    pub name: String,
    pub expression: String,
    #[serde(rename = "isHidden", default, skip_serializing_if = "is_false")]
    pub is_hidden: bool,
    #[serde(
        rename = "formatString",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub format_string: Option<String>,
    #[serde(
        rename = "displayFolder",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub display_folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Relationship {
    pub name: String,
    #[serde(rename = "fromTable")]
    pub from_table: String,
    #[serde(rename = "fromColumn")]
    pub from_column: String,
    #[serde(rename = "toTable")]
    pub to_table: String,
    #[serde(rename = "toColumn")]
    pub to_column: String,
    #[serde(
        deserialize_with = "deserialize_crossfilter",
        serialize_with = "serialize_crossfilter",
        rename = "crossFilteringBehavior"
    )]
    pub bidirectional: bool,
    #[serde(rename = "isActive")]
    pub active: bool,
}

fn deserialize_crossfilter<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: de::Deserializer<'de>,
{
    let s: &str = de::Deserialize::deserialize(deserializer)?;
    match s {
        "single" => Ok(false),
        "both" => Ok(true),
        _ => Err(de::Error::unknown_variant(s, &["single", "both"])),
    }
}

fn serialize_crossfilter<S: serde::Serializer>(v: &bool, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(if *v { "both" } else { "single" })
}

#[derive(Debug, Clone, PartialEq)]
pub enum DaxDataType {
    Automatic,
    String,
    Int64,
    Double,
    DateTime,
    Decimal,
    Boolean,
    Binary,
    Unknown,
    Variant,
}

impl DaxDataType {
    /// Every variant. Keep in sync with the enum.
    const ALL: [DaxDataType; 10] = [
        DaxDataType::Automatic,
        DaxDataType::String,
        DaxDataType::Int64,
        DaxDataType::Double,
        DaxDataType::DateTime,
        DaxDataType::Decimal,
        DaxDataType::Boolean,
        DaxDataType::Binary,
        DaxDataType::Unknown,
        DaxDataType::Variant,
    ];

    /// The name as written in TMSL (camelCase, as in the TMSL schema).
    pub fn tmsl_name(&self) -> &'static str {
        match self {
            DaxDataType::Automatic => "automatic",
            DaxDataType::String => "string",
            DaxDataType::Int64 => "int64",
            DaxDataType::Double => "double",
            DaxDataType::DateTime => "dateTime",
            DaxDataType::Decimal => "decimal",
            DaxDataType::Boolean => "boolean",
            DaxDataType::Binary => "binary",
            DaxDataType::Unknown => "unknown",
            DaxDataType::Variant => "variant",
        }
    }

    /// Case-insensitive: "int64", "Int64" and "INT64" are the same type.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|t| t.tmsl_name().eq_ignore_ascii_case(name))
    }
}

impl Serialize for DaxDataType {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.tmsl_name())
    }
}

// Not derived: the derive only accepts the Rust variant names ("Int64"), but
// Serialize writes the TMSL names ("int64"), so a saved model couldn't be read
// back. Accepting both spellings reads existing files and saved ones.
impl<'de> Deserialize<'de> for DaxDataType {
    fn deserialize<D: de::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::parse(&name).ok_or_else(|| {
            const EXPECTED: &[&str] = &[
                "automatic",
                "string",
                "int64",
                "double",
                "dateTime",
                "decimal",
                "boolean",
                "binary",
                "unknown",
                "variant",
            ];
            de::Error::unknown_variant(&name, EXPECTED)
        })
    }
}

impl From<&str> for DaxDataType {
    fn from(s: &str) -> Self {
        Self::parse(s).unwrap_or(DaxDataType::Unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::DaxDataType;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn every_data_type_survives_saving_and_loading() -> TestResult {
        for data_type in DaxDataType::ALL {
            let json = serde_json::to_string(&data_type)?;
            let back: DaxDataType = serde_json::from_str(&json)?;
            assert_eq!(back, data_type, "round trip of {json}");
        }
        Ok(())
    }

    #[test]
    fn data_types_are_read_case_insensitively() -> TestResult {
        for (json, expected) in [
            ("\"int64\"", DaxDataType::Int64),
            ("\"Int64\"", DaxDataType::Int64),
            ("\"INT64\"", DaxDataType::Int64),
            ("\"string\"", DaxDataType::String),
            ("\"String\"", DaxDataType::String),
            ("\"dateTime\"", DaxDataType::DateTime),
            ("\"DateTime\"", DaxDataType::DateTime),
            ("\"double\"", DaxDataType::Double),
        ] {
            let parsed: DaxDataType = serde_json::from_str(json)?;
            assert_eq!(parsed, expected, "{json}");
        }
        Ok(())
    }

    #[test]
    fn unknown_data_type_names_are_rejected() {
        let result = serde_json::from_str::<DaxDataType>("\"int32\"");
        assert!(
            matches!(&result, Err(e) if e.to_string().contains("int32")),
            "expected an error naming 'int32', got {result:?}"
        );
    }
}
