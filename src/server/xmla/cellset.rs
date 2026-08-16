//! Purpose-built Multidimensional-format XMLA cellset response type. Mirrors
//! `tabular.rs`'s split: typed axis/tuple/member/cell data plus one renderer,
//! with no per-shape string-building functions. Field presence rules (e.g.
//! `PARENT_UNIQUE_NAME` omitted for the "All" member, `CellInfo` reflecting
//! only requested cell properties) match the existing, real-capture-validated
//! `handlers.rs` cellset functions.

use super::xml_util::{cellset_xml, xml_escape_value, MDDATASET_SCHEMA};
use axum::response::Response;
use std::collections::{HashMap, HashSet};

pub struct CellsetMember {
    pub uname: String,
    pub caption: String,
    pub lname: String,
    pub lnum: u32,
    pub display_info: u32,
    pub parent_uname: Option<String>,
    pub hierarchy_uname: Option<String>,
    /// 1 = regular member, 2 = "All" member (MDX `MEMBER_TYPE` cell property).
    pub member_type: Option<u8>,
}

pub struct CellsetTuple {
    pub members: Vec<CellsetMember>,
}

pub struct CellsetHierarchyInfo {
    pub hier_uname: String,
    pub has_parent_unique_name: bool,
    pub has_hierarchy_unique_name: bool,
    pub has_member_type: bool,
}

pub struct CellsetAxis {
    /// `"Axis0"`, `"Axis1"`, or `"SlicerAxis"`.
    pub name: String,
    pub hierarchies: Vec<CellsetHierarchyInfo>,
    pub tuples: Vec<CellsetTuple>,
}

pub struct CellsetCell {
    pub ordinal: u32,
    pub value: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CellProp {
    Value,
    FormattedValue,
    FormatString,
    Language,
    BackColor,
    ForeColor,
    FontFlags,
    CellOrdinal,
}

#[derive(Default)]
pub struct CellsetResponse {
    cube_name: String,
    last_data_update: String,
    last_schema_update: String,
    axes: Vec<CellsetAxis>,
    cells: Vec<CellsetCell>,
    cell_props: Vec<CellProp>,
}

impl CellsetResponse {
    pub fn new(
        cube_name: impl Into<String>,
        last_data_update: impl Into<String>,
        last_schema_update: impl Into<String>,
    ) -> Self {
        Self {
            cube_name: cube_name.into(),
            last_data_update: last_data_update.into(),
            last_schema_update: last_schema_update.into(),
            ..Self::default()
        }
    }

    pub fn add_axis(&mut self, axis: CellsetAxis) {
        self.axes.push(axis);
    }

    pub fn set_cell_props(&mut self, cell_props: Vec<CellProp>) {
        self.cell_props = cell_props;
    }

    pub fn set_cells(&mut self, cells: Vec<CellsetCell>) {
        self.cells = cells;
    }

    pub fn render(&self, session_id: Option<&str>) -> (String, Response) {
        let body = format!(
            "<ns2:root>{schema}{olap}{axes}{cells}</ns2:root>",
            schema = MDDATASET_SCHEMA,
            olap = self.render_olap_info(),
            axes = self.render_axes(),
            cells = self.render_cell_data(),
        );
        cellset_xml(session_id, body)
    }

    fn render_olap_info(&self) -> String {
        let mut axes_info = String::new();
        for axis in &self.axes {
            axes_info.push_str(&format!(r#"<ns2:AxisInfo name="{}">"#, axis.name));
            for h in &axis.hierarchies {
                axes_info.push_str(&render_hierarchy_info(h));
            }
            axes_info.push_str("</ns2:AxisInfo>");
        }
        format!(
            concat!(
                "<ns2:OlapInfo>",
                "<ns2:CubeInfo><ns2:Cube>",
                "<ns2:CubeName>{cube}</ns2:CubeName>",
                "<ns4:LastDataUpdate>{last_data}</ns4:LastDataUpdate>",
                "<ns4:LastSchemaUpdate>{last_schema}</ns4:LastSchemaUpdate>",
                "</ns2:Cube></ns2:CubeInfo>",
                "<ns2:AxesInfo>{axes_info}</ns2:AxesInfo>",
                "<ns2:CellInfo>{cell_info}</ns2:CellInfo>",
                "</ns2:OlapInfo>",
            ),
            cube = xml_escape_value(&self.cube_name),
            last_data = xml_escape_value(&self.last_data_update),
            last_schema = xml_escape_value(&self.last_schema_update),
            axes_info = axes_info,
            cell_info = self.render_cell_info(),
        )
    }

    fn render_cell_info(&self) -> String {
        if self.cell_props.is_empty() {
            return concat!(
                r#"<ns2:Value name="VALUE" />"#,
                r#"<ns2:FmtValue name="FORMATTED_VALUE" type="xs:string" />"#,
                r#"<ns2:CellOrdinal name="CELL_ORDINAL" type="xs:unsignedInt" />"#,
            )
            .to_string();
        }
        let mut out = String::new();
        for p in &self.cell_props {
            out.push_str(match p {
                CellProp::Value => r#"<ns2:Value name="VALUE" />"#,
                CellProp::FormattedValue => {
                    r#"<ns2:FmtValue name="FORMATTED_VALUE" type="xs:string" />"#
                }
                CellProp::FormatString => {
                    r#"<ns2:FormatString name="FORMAT_STRING" type="xs:string" />"#
                }
                CellProp::Language => r#"<ns2:Language name="LANGUAGE" type="xs:unsignedInt" />"#,
                CellProp::BackColor => {
                    r#"<ns2:BackColor name="BACK_COLOR" type="xs:unsignedInt" />"#
                }
                CellProp::ForeColor => {
                    r#"<ns2:ForeColor name="FORE_COLOR" type="xs:unsignedInt" />"#
                }
                CellProp::FontFlags => r#"<ns2:FontFlags name="FONT_FLAGS" type="xs:int" />"#,
                CellProp::CellOrdinal => {
                    r#"<ns2:CellOrdinal name="CELL_ORDINAL" type="xs:unsignedInt" />"#
                }
            });
        }
        out
    }

    fn render_axes(&self) -> String {
        let mut out = String::from("<ns2:Axes>");
        for axis in &self.axes {
            out.push_str(&render_axis(axis));
        }
        out.push_str("</ns2:Axes>");
        out
    }

    fn render_cell_data(&self) -> String {
        if self.cells.is_empty() {
            return "<ns2:CellData />".to_string();
        }
        let has_value = self.cell_props.is_empty() || self.cell_props.contains(&CellProp::Value);
        let has_fmt =
            self.cell_props.is_empty() || self.cell_props.contains(&CellProp::FormattedValue);

        let mut out = String::from("<ns2:CellData>");
        for cell in &self.cells {
            out.push_str(&format!(r#"<ns2:Cell CellOrdinal="{}">"#, cell.ordinal));
            if let Some(v) = &cell.value {
                if has_value {
                    out.push_str(&format!("<ns2:Value>{}</ns2:Value>", xml_escape_value(v)));
                }
                if has_fmt {
                    out.push_str(&format!(
                        "<ns2:FmtValue>{}</ns2:FmtValue>",
                        xml_escape_value(v)
                    ));
                }
            }
            out.push_str("</ns2:Cell>");
        }
        out.push_str("</ns2:CellData>");
        out
    }
}

fn render_hierarchy_info(h: &CellsetHierarchyInfo) -> String {
    let hier = &h.hier_uname;
    let prop = |elem: &str, name: &str, typ: &str| {
        format!(r#"<ns2:{elem} name="{hier}.[{name}]" type="{typ}" />"#)
    };

    let mut out = format!(r#"<ns2:HierarchyInfo name="{hier}">"#);
    out.push_str(&prop("UName", "MEMBER_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("Caption", "MEMBER_CAPTION", "xs:string"));
    out.push_str(&prop("LName", "LEVEL_UNIQUE_NAME", "xs:string"));
    out.push_str(&prop("LNum", "LEVEL_NUMBER", "xs:int"));
    out.push_str(&prop("DisplayInfo", "DISPLAY_INFO", "xs:unsignedInt"));
    if h.has_parent_unique_name {
        out.push_str(&prop(
            "PARENT_UNIQUE_NAME",
            "PARENT_UNIQUE_NAME",
            "xs:string",
        ));
    }
    if h.has_hierarchy_unique_name {
        out.push_str(&prop(
            "HIERARCHY_UNIQUE_NAME",
            "HIERARCHY_UNIQUE_NAME",
            "xs:string",
        ));
    }
    if h.has_member_type {
        out.push_str(&prop("MEMBER_TYPE", "MEMBER_TYPE", "xs:int"));
    }
    out.push_str("</ns2:HierarchyInfo>");
    out
}

/// Picks the literal `Tuples`/`Tuple`/`Member` encoding or the compact
/// `NormTupleSet`/`MembersLookup` one real Fabric uses for crossjoin axes
/// with real member duplication (both are valid XMLA - this only affects
/// response size). `CellsetAxis`/`CellsetTuple`/`CellsetMember` stay fully
/// encoding-agnostic; this is the only place the choice is made.
fn render_axis(axis: &CellsetAxis) -> String {
    let hierarchy_count = axis.tuples.first().map_or(0, |t| t.members.len());
    if hierarchy_count > 1 {
        let mut seen: Vec<HashSet<&str>> = vec![HashSet::new(); hierarchy_count];
        for tuple in &axis.tuples {
            for (pos, m) in tuple.members.iter().enumerate() {
                seen[pos].insert(m.uname.as_str());
            }
        }
        let distinct_total: usize = seen.iter().map(|s| s.len()).sum();
        if distinct_total < axis.tuples.len() * hierarchy_count {
            return render_axis_normalized(axis);
        }
    }
    render_axis_literal(axis)
}

fn render_axis_literal(axis: &CellsetAxis) -> String {
    let mut out = format!(r#"<ns2:Axis name="{}">"#, axis.name);
    if axis.tuples.is_empty() {
        out.push_str("<ns2:Tuples />");
    } else {
        out.push_str("<ns2:Tuples>");
        for t in &axis.tuples {
            out.push_str("<ns2:Tuple>");
            for m in &t.members {
                out.push_str(&render_member_literal(m));
            }
            out.push_str("</ns2:Tuple>");
        }
        out.push_str("</ns2:Tuples>");
    }
    out.push_str("</ns2:Axis>");
    out
}

fn render_axis_normalized(axis: &CellsetAxis) -> String {
    let hierarchy_count = axis.tuples.first().map_or(0, |t| t.members.len());

    // One dedup lookup list per hierarchy position, keyed by UName, in
    // first-seen order - the members MembersLookup declares once each.
    let mut lookups: Vec<Vec<&CellsetMember>> = vec![Vec::new(); hierarchy_count];
    let mut ordinal_of: Vec<HashMap<&str, usize>> = vec![HashMap::new(); hierarchy_count];
    for tuple in &axis.tuples {
        for (pos, m) in tuple.members.iter().enumerate() {
            if !ordinal_of[pos].contains_key(m.uname.as_str()) {
                ordinal_of[pos].insert(m.uname.as_str(), lookups[pos].len());
                lookups[pos].push(m);
            }
        }
    }

    let mut norm_tuples = String::new();
    for tuple in &axis.tuples {
        norm_tuples.push_str("<ns5:NormTuple>");
        for (pos, m) in tuple.members.iter().enumerate() {
            let ord = ordinal_of[pos][m.uname.as_str()];
            norm_tuples.push_str(&format!(
                concat!(
                    "<ns5:MemberRef>",
                    "<ns5:MemberOrdinal>{ord}</ns5:MemberOrdinal>",
                    "<ns5:MemberDispInfo>{di}</ns5:MemberDispInfo>",
                    "</ns5:MemberRef>",
                ),
                ord = ord,
                di = m.display_info,
            ));
        }
        norm_tuples.push_str("</ns5:NormTuple>");
    }

    let mut members_lookup = String::from("<ns5:MembersLookup>");
    for lookup in &lookups {
        members_lookup.push_str("<ns2:Members>");
        for m in lookup {
            members_lookup.push_str(&render_lookup_member(m));
        }
        members_lookup.push_str("</ns2:Members>");
    }
    members_lookup.push_str("</ns5:MembersLookup>");

    format!(
        r#"<ns2:Axis name="{name}"><ns5:NormTupleSet><ns5:NormTuples>{norm_tuples}</ns5:NormTuples>{members_lookup}</ns5:NormTupleSet></ns2:Axis>"#,
        name = axis.name,
    )
}

fn render_member_literal(m: &CellsetMember) -> String {
    let mut out = format!(
        concat!(
            "<ns2:Member>",
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>{caption}</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>{lnum}</ns2:LNum>",
            "<ns2:DisplayInfo>{di}</ns2:DisplayInfo>",
        ),
        uname = xml_escape_value(&m.uname),
        caption = xml_escape_value(&m.caption),
        lname = xml_escape_value(&m.lname),
        lnum = m.lnum,
        di = m.display_info,
    );
    render_member_common_tail(m, &mut out);
    out
}

/// `MembersLookup` entries carry no `DisplayInfo` — that's tuple-position
/// specific and lives on each `NormTuple`'s `MemberRef`/`MemberDispInfo`
/// instead, since the same looked-up member can appear at the same position
/// in more than one tuple with a different `DisplayInfo` each time.
fn render_lookup_member(m: &CellsetMember) -> String {
    let mut out = format!(
        concat!(
            "<ns2:Member>",
            "<ns2:UName>{uname}</ns2:UName>",
            "<ns2:Caption>{caption}</ns2:Caption>",
            "<ns2:LName>{lname}</ns2:LName>",
            "<ns2:LNum>{lnum}</ns2:LNum>",
        ),
        uname = xml_escape_value(&m.uname),
        caption = xml_escape_value(&m.caption),
        lname = xml_escape_value(&m.lname),
        lnum = m.lnum,
    );
    render_member_common_tail(m, &mut out);
    out
}

fn render_member_common_tail(m: &CellsetMember, out: &mut String) {
    if let Some(parent) = &m.parent_uname {
        out.push_str(&format!(
            "<ns2:PARENT_UNIQUE_NAME>{}</ns2:PARENT_UNIQUE_NAME>",
            xml_escape_value(parent)
        ));
    }
    if let Some(hier) = &m.hierarchy_uname {
        out.push_str(&format!(
            "<ns2:HIERARCHY_UNIQUE_NAME>{}</ns2:HIERARCHY_UNIQUE_NAME>",
            xml_escape_value(hier)
        ));
    }
    if let Some(mt) = m.member_type {
        out.push_str(&format!("<ns2:MEMBER_TYPE>{mt}</ns2:MEMBER_TYPE>"));
    }
    out.push_str("</ns2:Member>");
}
