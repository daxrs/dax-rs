use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableCol {
    pub table: String,
    pub col: String,
}

impl TableCol {
    pub fn new(table: impl Into<String>, col: impl Into<String>) -> Self {
        Self { table: table.into(), col: col.into() }
    }

    pub fn try_parse(s: &str) -> Option<Self> {
        if let Some(stripped) = s.strip_prefix('\'') {
            let end = stripped.find('\'')?;
            let table = stripped[..end].to_string();
            let rest = &stripped[end + 1..];
            let col = rest.strip_prefix('[')?.strip_suffix(']')?.to_string();
            if col.is_empty() {
                return None;
            }
            Some(Self { table, col })
        } else if let Some(bracket) = s.find('[') {
            let table = s[..bracket].to_string();
            let col = s[bracket + 1..].strip_suffix(']')?.to_string();
            if table.is_empty() || col.is_empty() {
                return None;
            }
            Some(Self { table, col })
        } else {
            None
        }
    }
}

impl fmt::Display for TableCol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.table.contains(' ') {
            write!(f, "'{}'[{}]", self.table, self.col)
        } else {
            write!(f, "{}[{}]", self.table, self.col)
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn display_simple_name() {
        assert_eq!(
            TableCol::new("Product", "Color").to_string(),
            "Product[Color]"
        );
    }

    #[test]
    fn display_name_with_spaces() {
        assert_eq!(
            TableCol::new("My Table", "Column").to_string(),
            "'My Table'[Column]"
        );
    }

    #[test]
    fn parse_simple() {
        let tc = TableCol::try_parse("Product[Color]").unwrap();
        assert_eq!(tc.table, "Product");
        assert_eq!(tc.col, "Color");
    }

    #[test]
    fn parse_quoted() {
        let tc = TableCol::try_parse("'My Table'[Column]").unwrap();
        assert_eq!(tc.table, "My Table");
        assert_eq!(tc.col, "Column");
    }

    #[test]
    fn parse_bare_name_returns_none() {
        assert!(TableCol::try_parse("TotalAmount").is_none());
    }

    #[test]
    fn roundtrip() {
        for s in &["Product[Color]", "'My Table'[Column]"] {
            let tc = TableCol::try_parse(s).unwrap();
            assert_eq!(&tc.to_string(), s);
        }
    }

    #[test]
    fn ord_table_precedes_col() {
        let a = TableCol::new("Apple", "Z");
        let b = TableCol::new("Banana", "A");
        assert!(a < b, "table name should take precedence over col name");

        let c = TableCol::new("Sales", "Amount");
        let d = TableCol::new("Sales", "Quantity");
        assert!(c < d, "when tables are equal, col name is the tiebreaker");
    }
}
