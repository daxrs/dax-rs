use crate::engine::error::{DaxError, DaxResult};
use crate::engine::row_context::ScalarValue;
use polars::prelude::{AnyValue, DataFrame, NamedFrom, Series};

#[derive(Debug, Clone)]
pub enum Value {
    Integer(i64),
    Number(f64),
    String(String),
    Boolean(bool),
    Blank,
    /// Milliseconds since Unix epoch (UTC), matching Polars Datetime(Milliseconds).
    DateTime(i64),
    Table(String, DataFrame),
    Series(Series),
}

impl<'a> TryFrom<AnyValue<'a>> for Value {
    type Error = DaxError;
    fn try_from(av: AnyValue<'a>) -> DaxResult<Self> {
        Ok(ScalarValue::try_from(av)?.into())
    }
}

impl Value {
    pub fn to_series(values: &[Self], name: &str) -> crate::engine::error::DaxResult<Series> {
        use crate::engine::error::DaxError;
        let first = values.iter().find(|v| !matches!(v, Value::Blank));
        match first {
            Some(Value::Integer(_)) if !values.iter().any(|v| matches!(v, Value::Number(_))) => {
                let data: Result<Vec<Option<i64>>, DaxError> = values
                    .iter()
                    .map(|v| match v {
                        Value::Integer(i) => Ok(Some(*i)),
                        Value::Blank => Ok(None),
                        other => Err(DaxError::Type(format!("mixed types in column: {other:?}"))),
                    })
                    .collect();
                Ok(Series::new(name.into(), data?))
            }
            Some(Value::Integer(_) | Value::Number(_)) => {
                let data: Result<Vec<Option<f64>>, DaxError> = values
                    .iter()
                    .map(|v| match v {
                        Value::Number(n) => Ok(Some(*n)),
                        Value::Integer(i) => Ok(Some(*i as f64)),
                        Value::Blank => Ok(None),
                        other => Err(DaxError::Type(format!("mixed types in column: {other:?}"))),
                    })
                    .collect();
                Ok(Series::new(name.into(), data?))
            }
            Some(Value::String(_)) => {
                let data: Result<Vec<Option<&str>>, DaxError> = values
                    .iter()
                    .map(|v| match v {
                        Value::String(s) => Ok(Some(s.as_str())),
                        Value::Blank => Ok(None),
                        other => Err(DaxError::Type(format!("mixed types in column: {other:?}"))),
                    })
                    .collect();
                Ok(Series::new(name.into(), data?))
            }
            Some(Value::Boolean(_)) => {
                let data: Result<Vec<Option<bool>>, DaxError> = values
                    .iter()
                    .map(|v| match v {
                        Value::Boolean(b) => Ok(Some(*b)),
                        Value::Blank => Ok(None),
                        other => Err(DaxError::Type(format!("mixed types in column: {other:?}"))),
                    })
                    .collect();
                Ok(Series::new(name.into(), data?))
            }
            None | Some(Value::Blank) => Ok(Series::new_null(name.into(), values.len())),
            Some(other) => Err(DaxError::Type(format!(
                "unsupported column type: {other:?}"
            ))),
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Integer(x), Value::Integer(y)) => x == y,
            (Value::Integer(x), Value::Number(y)) => (*x as f64) == *y,
            (Value::Number(x), Value::Integer(y)) => *x == (*y as f64),
            (Value::Number(x), Value::Number(y)) => x == y,
            (Value::Boolean(x), Value::Boolean(y)) => x == y,
            (Value::String(x), Value::String(y)) => x == y,
            (Value::DateTime(x), Value::DateTime(y)) => x == y,
            (Value::Blank, Value::Blank) => true,
            _ => false,
        }
    }
}

impl From<ScalarValue> for Value {
    fn from(sv: ScalarValue) -> Self {
        match sv {
            ScalarValue::Integer(i) => Value::Integer(i),
            ScalarValue::Number(n) => Value::Number(n),
            ScalarValue::Text(s) => Value::String(s),
            ScalarValue::Boolean(b) => Value::Boolean(b),
            ScalarValue::DateTime(ms) => Value::DateTime(ms),
            ScalarValue::Blank => Value::Blank,
        }
    }
}
