use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::row_context::RowContext;
use polars::prelude::*;

fn to_bool(v: &Value, fn_name: &str) -> DaxResult<bool> {
    match v {
        Value::Boolean(b) => Ok(*b),
        Value::Blank => Ok(false),
        other => Err(DaxError::Type(format!(
            "{fn_name}: expected Boolean, got {other:?}"
        ))),
    }
}

pub fn blank(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::Blank)
}

pub fn true_fn(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::Boolean(true))
}

pub fn false_fn(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::Boolean(false))
}

pub fn and(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match (&args[0], &args[1]) {
        (Value::Series(a), Value::Series(b)) => {
            let ca = a
                .bool()
                .map_err(|_| DaxError::Type("AND expects boolean series".into()))?;
            let cb = b
                .bool()
                .map_err(|_| DaxError::Type("AND expects boolean series".into()))?;
            Ok(Value::Series((ca & cb).into_series()))
        }
        (Value::Series(s), Value::Boolean(b)) | (Value::Boolean(b), Value::Series(s)) => {
            let ca = s
                .bool()
                .map_err(|_| DaxError::Type("AND expects boolean series".into()))?;
            if *b {
                Ok(Value::Series(ca.clone().into_series()))
            } else {
                Ok(Value::Series(
                    BooleanChunked::full("".into(), false, s.len()).into_series(),
                ))
            }
        }
        _ => {
            let a = to_bool(&args[0], "AND")?;
            let b = to_bool(&args[1], "AND")?;
            Ok(Value::Boolean(a && b))
        }
    }
}

pub fn or(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match (&args[0], &args[1]) {
        (Value::Series(a), Value::Series(b)) => {
            let ca = a
                .bool()
                .map_err(|_| DaxError::Type("OR expects boolean series".into()))?;
            let cb = b
                .bool()
                .map_err(|_| DaxError::Type("OR expects boolean series".into()))?;
            Ok(Value::Series((ca | cb).into_series()))
        }
        (Value::Series(s), Value::Boolean(b)) | (Value::Boolean(b), Value::Series(s)) => {
            let ca = s
                .bool()
                .map_err(|_| DaxError::Type("OR expects boolean series".into()))?;
            if *b {
                Ok(Value::Series(
                    BooleanChunked::full("".into(), true, s.len()).into_series(),
                ))
            } else {
                Ok(Value::Series(ca.clone().into_series()))
            }
        }
        _ => {
            let a = to_bool(&args[0], "OR")?;
            let b = to_bool(&args[1], "OR")?;
            Ok(Value::Boolean(a || b))
        }
    }
}

pub fn isblank(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match args.into_iter().next() {
        None => Err(DaxError::InvalidArgument(
            "ISBLANK requires 1 argument".into(),
        )),
        Some(Value::Blank) => Ok(Value::Boolean(true)),
        Some(Value::Series(s)) => Ok(Value::Series(s.is_null().into_series())),
        Some(_) => Ok(Value::Boolean(false)),
    }
}

pub fn not(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Boolean(b) => Ok(Value::Boolean(!b)),
        Value::Blank => Ok(Value::Boolean(true)),
        Value::Series(s) => {
            let ca = s
                .bool()
                .map_err(|_| DaxError::Type("NOT expects a boolean series".into()))?;
            Ok(Value::Series((!ca).into_series()))
        }
        other => Err(DaxError::Type(format!(
            "NOT expects a boolean value, got {other:?}"
        ))),
    }
}

pub fn error_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let msg = match args.into_iter().next() {
        Some(Value::String(s)) => s,
        Some(other) => format!("{other:?}"),
        None => "ERROR called with no message".into(),
    };
    Err(DaxError::Eval(msg))
}
