use super::math::to_f64_series;
use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::row_context::RowContext;
use polars::prelude::*;

fn coerce_a(v: AnyValue<'_>) -> Option<f64> {
    match v {
        AnyValue::Null => None,
        AnyValue::Boolean(b) => Some(if b { 1.0 } else { 0.0 }),
        AnyValue::Int8(n) => Some(n as f64),
        AnyValue::Int16(n) => Some(n as f64),
        AnyValue::Int32(n) => Some(n as f64),
        AnyValue::Int64(n) => Some(n as f64),
        AnyValue::UInt8(n) => Some(n as f64),
        AnyValue::UInt16(n) => Some(n as f64),
        AnyValue::UInt32(n) => Some(n as f64),
        AnyValue::UInt64(n) => Some(n as f64),
        AnyValue::Float32(n) => Some(n as f64),
        AnyValue::Float64(n) => Some(n),
        _ => Some(0.0), // text and other non-null types → 0
    }
}

pub fn sum(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            if s.is_empty() {
                return Ok(Value::Blank);
            }
            match s.dtype() {
                DataType::Int64 => Ok(Value::Integer(
                    s.i64()
                        .expect("dtype matched Int64 above")
                        .sum()
                        .unwrap_or(0),
                )),
                DataType::Int32 => Ok(Value::Integer(
                    s.i32()
                        .expect("dtype matched Int32 above")
                        .sum()
                        .unwrap_or(0) as i64,
                )),
                _ => Ok(Value::Number(to_f64_series(s)?.sum().unwrap_or(0.0))),
            }
        }
        other => Err(DaxError::Type(format!(
            "SUM expects a Series, got {other:?}"
        ))),
    }
}

pub fn count(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let n = s.len() - s.null_count();
            Ok(if n == 0 {
                Value::Blank
            } else {
                Value::Number(n as f64)
            })
        }
        other => Err(DaxError::Type(format!(
            "COUNT expects a column, got {other:?}"
        ))),
    }
}

pub fn average(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(s.mean().map_or(Value::Blank, Value::Number)),
        other => Err(DaxError::Type(format!(
            "AVERAGE expects a column, got {other:?}"
        ))),
    }
}

pub fn min(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => match s.dtype() {
            DataType::Int64 => Ok(s
                .i64()
                .expect("dtype matched Int64 above")
                .min()
                .map(Value::Integer)
                .unwrap_or(Value::Blank)),
            DataType::Int32 => Ok(s
                .i32()
                .expect("dtype matched Int32 above")
                .min()
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Blank)),
            DataType::Datetime(_, _) => Ok(s
                .datetime()
                .expect("dtype matched Datetime above")
                .phys
                .min()
                .map(Value::DateTime)
                .unwrap_or(Value::Blank)),
            DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32 => Ok(to_f64_series(s)?
                .f64()
                .expect("to_f64_series guarantees Float64 dtype")
                .min()
                .map_or(Value::Blank, Value::Number)),
            DataType::Float64 => Ok(s
                .f64()
                .expect("dtype matched Float64 above")
                .min()
                .map_or(Value::Blank, Value::Number)),
            other @ (DataType::Boolean
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => {
                Err(DaxError::Type(format!("MIN: unsupported dtype {other:?}")))
            }
        },
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Number(n) => Ok(Value::Number(*n)),
        Value::String(s) => Ok(Value::String(s.clone())),
        Value::Boolean(b) => Ok(Value::Boolean(*b)),
        Value::DateTime(ms) => Ok(Value::DateTime(*ms)),
        Value::Blank => Ok(Value::Blank),
        other @ Value::Table(_, _) => Err(DaxError::Type(format!(
            "MIN expects a column, got {other:?}"
        ))),
    }
}

pub fn max(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => match s.dtype() {
            DataType::Int64 => Ok(s
                .i64()
                .expect("dtype matched Int64 above")
                .max()
                .map(Value::Integer)
                .unwrap_or(Value::Blank)),
            DataType::Int32 => Ok(s
                .i32()
                .expect("dtype matched Int32 above")
                .max()
                .map(|v| Value::Integer(v as i64))
                .unwrap_or(Value::Blank)),
            DataType::Datetime(_, _) => Ok(s
                .datetime()
                .expect("dtype matched Datetime above")
                .phys
                .max()
                .map(Value::DateTime)
                .unwrap_or(Value::Blank)),
            DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::UInt128
            | DataType::Int8
            | DataType::Int16
            | DataType::Int128
            | DataType::Float16
            | DataType::Float32 => Ok(to_f64_series(s)?
                .f64()
                .expect("to_f64_series guarantees Float64 dtype")
                .max()
                .map_or(Value::Blank, Value::Number)),
            DataType::Float64 => Ok(s
                .f64()
                .expect("dtype matched Float64 above")
                .max()
                .map_or(Value::Blank, Value::Number)),
            other @ (DataType::Boolean
            | DataType::String
            | DataType::Binary
            | DataType::BinaryOffset
            | DataType::Date
            | DataType::Duration(_)
            | DataType::Time
            | DataType::List(_)
            | DataType::Null
            | DataType::Categorical(_, _)
            | DataType::Enum(_, _)
            | DataType::Struct(_)
            | DataType::Unknown(_)) => {
                Err(DaxError::Type(format!("MAX: unsupported dtype {other:?}")))
            }
        },
        Value::Integer(i) => Ok(Value::Integer(*i)),
        Value::Number(n) => Ok(Value::Number(*n)),
        Value::String(s) => Ok(Value::String(s.clone())),
        Value::Boolean(b) => Ok(Value::Boolean(*b)),
        Value::DateTime(ms) => Ok(Value::DateTime(*ms)),
        Value::Blank => Ok(Value::Blank),
        other @ Value::Table(_, _) => Err(DaxError::Type(format!(
            "MAX expects a column, got {other:?}"
        ))),
    }
}

pub fn countrows(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match args.into_iter().next() {
        Some(Value::Table(_, df)) => {
            let n = df.height();
            Ok(if n == 0 {
                Value::Blank
            } else {
                Value::Number(n as f64)
            })
        }
        Some(other) => Err(DaxError::Type(format!(
            "COUNTROWS expects a table, got {other:?}"
        ))),
        None => Err(DaxError::InvalidArgument(
            "COUNTROWS requires one argument".into(),
        )),
    }
}

pub fn isempty(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match args.into_iter().next() {
        Some(Value::Table(_, df)) => Ok(Value::Boolean(df.height() == 0)),
        Some(other) => Err(DaxError::Type(format!(
            "ISEMPTY expects a table, got {other:?}"
        ))),
        None => Err(DaxError::InvalidArgument(
            "ISEMPTY requires one argument".into(),
        )),
    }
}

pub fn distinctcount(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let n = s
                .n_unique()
                .expect("DAX columns are primitive scalar dtypes; n_unique cannot fail");
            Ok(if n == 0 {
                Value::Blank
            } else {
                Value::Number(n as f64)
            })
        }
        other => Err(DaxError::Type(format!(
            "DISTINCTCOUNT expects a column, got {other:?}"
        ))),
    }
}

pub fn hasonevalue(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Boolean(
            s.n_unique()
                .expect("DAX columns are primitive scalar dtypes; n_unique cannot fail")
                == 1,
        )),
        other => Err(DaxError::Type(format!(
            "HASONEVALUE expects a column, got {other:?}"
        ))),
    }
}

pub fn counta(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let n = s.len() - s.null_count();
            Ok(if n == 0 {
                Value::Blank
            } else {
                Value::Number(n as f64)
            })
        }
        other => Err(DaxError::Type(format!(
            "COUNTA expects a column, got {other:?}"
        ))),
    }
}

pub fn averagea(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let s = s.rechunk();
            let mut sum = 0.0f64;
            let mut n = 0usize;
            for v in s.iter() {
                if let Some(f) = coerce_a(v) {
                    sum += f;
                    n += 1;
                }
            }
            Ok(if n == 0 {
                Value::Blank
            } else {
                Value::Number(sum / n as f64)
            })
        }
        other => Err(DaxError::Type(format!(
            "AVERAGEA expects a column, got {other:?}"
        ))),
    }
}

pub fn mina(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(s
            .rechunk()
            .iter()
            .filter_map(coerce_a)
            .reduce(f64::min)
            .map_or(Value::Blank, Value::Number)),
        other => Err(DaxError::Type(format!(
            "MINA expects a column, got {other:?}"
        ))),
    }
}

pub fn maxa(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(s
            .rechunk()
            .iter()
            .filter_map(coerce_a)
            .reduce(f64::max)
            .map_or(Value::Blank, Value::Number)),
        other => Err(DaxError::Type(format!(
            "MAXA expects a column, got {other:?}"
        ))),
    }
}

pub fn values_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let unique = s
                .unique()
                .expect("DAX columns are primitive scalar dtypes; unique cannot fail");
            let df = DataFrame::new_infer_height(vec![unique.clone().into()])
                .expect("single-column frame height is trivially consistent");
            Ok(Value::Table(s.name().to_string(), df))
        }
        Value::Table(name, df) => {
            let deduped = df
                .unique_stable(None, polars::prelude::UniqueKeepStrategy::First, None)
                .expect("DAX columns are primitive scalar dtypes; unique_stable cannot fail");
            Ok(Value::Table(name.clone(), deduped))
        }
        other => Err(DaxError::Type(format!(
            "VALUES expects a column or table, got {other:?}"
        ))),
    }
}

pub fn distinct_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => {
            let unique = s
                .unique()
                .expect("DAX columns are primitive scalar dtypes; unique cannot fail")
                .drop_nulls();
            let df = DataFrame::new_infer_height(vec![unique.clone().into()])
                .expect("single-column frame height is trivially consistent");
            Ok(Value::Table(s.name().to_string(), df))
        }
        Value::Table(name, df) => {
            let deduped = df
                .unique_stable(None, polars::prelude::UniqueKeepStrategy::First, None)
                .expect("DAX columns are primitive scalar dtypes; unique_stable cannot fail");
            // Drop rows where every column is null (blank rows introduced by relationships).
            let filtered = deduped
                .columns()
                .iter()
                .map(|s| s.is_not_null())
                .reduce(|a, b| a | b)
                .map(|mask| {
                    deduped
                        .filter(&mask)
                        .expect("mask derived from deduped's own columns matches its row count")
                })
                .unwrap_or(deduped);
            Ok(Value::Table(name.clone(), filtered))
        }
        other => Err(DaxError::Type(format!(
            "DISTINCT expects a column or table, got {other:?}"
        ))),
    }
}

pub fn except_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let mut it = args.into_iter();
    let (name1, df1) = match it.next() {
        Some(Value::Table(name, df)) => (name, df),
        Some(other) => {
            return Err(DaxError::Type(format!(
                "EXCEPT: first argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "EXCEPT: missing first argument".into(),
            ))
        }
    };
    let df2 = match it.next() {
        Some(Value::Table(_, df)) => df,
        Some(other) => {
            return Err(DaxError::Type(format!(
                "EXCEPT: second argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "EXCEPT: missing second argument".into(),
            ))
        }
    };

    let left_cols: Vec<String> = df1
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let right_cols: Vec<String> = df2
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    // BLANK is a matchable DAX value (e.g. ROLLUP grand-total rows) — a blank
    // row must be recognized as a duplicate, not treated as "not found".
    let mut join_args = JoinArgs::new(JoinType::Anti);
    join_args.nulls_equal = true;
    let result = df1
        .join(&df2, &left_cols, &right_cols, join_args, None)
        .map_err(|e| DaxError::Eval(format!("EXCEPT: join failed: {e}")))?;

    Ok(Value::Table(name1, result))
}

pub fn union_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let mut it = args.into_iter();
    let (name, first_df) = match it.next() {
        Some(Value::Table(name, df)) => (name, df),
        Some(other) => {
            return Err(DaxError::Type(format!(
                "UNION: arguments must be tables, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "UNION: requires at least one argument".into(),
            ))
        }
    };

    let result = it.try_fold(first_df, |acc, arg| {
        let df = match arg {
            Value::Table(_, df) => df,
            other => {
                return Err(DaxError::Type(format!(
                    "UNION: arguments must be tables, got {other:?}"
                )))
            }
        };
        acc.vstack(&df)
            .map_err(|e| DaxError::Eval(format!("UNION: vstack failed: {e}")))
    })?;

    Ok(Value::Table(name, result))
}

pub fn intersect_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let mut it = args.into_iter();
    let (name1, df1) = match it.next() {
        Some(Value::Table(name, df)) => (name, df),
        Some(other) => {
            return Err(DaxError::Type(format!(
                "INTERSECT: first argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "INTERSECT: missing first argument".into(),
            ))
        }
    };
    let df2 = match it.next() {
        Some(Value::Table(_, df)) => df,
        Some(other) => {
            return Err(DaxError::Type(format!(
                "INTERSECT: second argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "INTERSECT: missing second argument".into(),
            ))
        }
    };

    let left_cols: Vec<String> = df1
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();
    let right_cols: Vec<String> = df2
        .get_column_names()
        .iter()
        .map(|s| s.to_string())
        .collect();

    let mut join_args = JoinArgs::new(JoinType::Semi);
    join_args.nulls_equal = true;
    let result = df1
        .join(&df2, &left_cols, &right_cols, join_args, None)
        .map_err(|e| DaxError::Eval(format!("INTERSECT: join failed: {e}")))?;

    Ok(Value::Table(name1, result))
}

pub fn row_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    if args.len() < 2 || !args.len().is_multiple_of(2) {
        return Err(DaxError::InvalidArgument(
            "ROW requires one or more name/expression pairs".into(),
        ));
    }

    let mut columns: Vec<Column> = Vec::new();
    let mut it = args.into_iter();
    loop {
        match it.next() {
            None => break,
            Some(Value::String(name)) => {
                let val = it.next().expect("length validated above");
                columns.push(Value::to_series(&[val], &name)?.into());
            }
            Some(other) => {
                return Err(DaxError::InvalidArgument(format!(
                    "ROW: expected string column name, got {other:?}"
                )))
            }
        }
    }

    let df = DataFrame::new_infer_height(columns)
        .map_err(|e| DaxError::Eval(format!("ROW: DataFrame construction failed: {e}")))?;
    Ok(Value::Table(String::new(), df))
}

pub fn natural_left_outer_join_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let mut it = args.into_iter();
    let (left_name, left_df) = match it.next() {
        Some(Value::Table(name, df)) => (name, df),
        Some(other) => {
            return Err(DaxError::Type(format!(
                "NATURALLEFTOUTERJOIN: first argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "NATURALLEFTOUTERJOIN: missing first argument".into(),
            ))
        }
    };
    let right_df = match it.next() {
        Some(Value::Table(_, df)) => df,
        Some(other) => {
            return Err(DaxError::Type(format!(
                "NATURALLEFTOUTERJOIN: second argument must be a table, got {other:?}"
            )))
        }
        None => {
            return Err(DaxError::InvalidArgument(
                "NATURALLEFTOUTERJOIN: missing second argument".into(),
            ))
        }
    };

    fn bare(s: &str) -> &str {
        if let Some(start) = s.rfind('[') {
            if s.ends_with(']') {
                return &s[start + 1..s.len() - 1];
            }
        }
        s
    }

    fn is_qualified(s: &str) -> bool {
        s.contains('[') && s.ends_with(']')
    }

    let left_col_names = left_df.get_column_names();
    let mut left_keys: Vec<String> = Vec::new();
    let mut right_keys: Vec<String> = Vec::new();
    for right_col in right_df.get_column_names() {
        let right_str = right_col.as_str();
        let left_match = if is_qualified(right_str) {
            left_col_names.iter().find(|n| n.as_str() == right_str)
        } else {
            left_col_names
                .iter()
                .find(|n| bare(n.as_str()) == bare(right_str))
        };
        if let Some(left_col) = left_match {
            left_keys.push(left_col.to_string());
            right_keys.push(right_col.to_string());
        }
    }

    if left_keys.is_empty() {
        return Err(DaxError::InvalidArgument(
            "NATURALLEFTOUTERJOIN: left and right tables share no column names".into(),
        ));
    }

    let mut join_args = JoinArgs::new(JoinType::Left);
    join_args.nulls_equal = true;
    let result = left_df
        .join(&right_df, &left_keys, &right_keys, join_args, None)
        .map_err(|e| DaxError::Eval(format!("NATURALLEFTOUTERJOIN: join failed: {e}")))?;

    Ok(Value::Table(left_name, result))
}
