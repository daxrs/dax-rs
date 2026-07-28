use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::row_context::RowContext;
use polars::prelude::*;

pub(crate) fn to_f64_series(s: &Series) -> DaxResult<Series> {
    match s.dtype() {
        DataType::Float64 => Ok(s.clone()),
        DataType::UInt8
        | DataType::UInt16
        | DataType::UInt32
        | DataType::UInt64
        | DataType::UInt128
        | DataType::Int8
        | DataType::Int16
        | DataType::Int32
        | DataType::Int64
        | DataType::Int128
        | DataType::Float16
        | DataType::Float32 => s
            .cast(&DataType::Float64)
            .map_err(|e| DaxError::Type(format!("Failed to cast to Float64: {e}"))),
        other @ (DataType::Boolean
        | DataType::String
        | DataType::Binary
        | DataType::BinaryOffset
        | DataType::Date
        | DataType::Datetime(_, _)
        | DataType::Duration(_)
        | DataType::Time
        | DataType::List(_)
        | DataType::Null
        | DataType::Categorical(_, _)
        | DataType::Enum(_, _)
        | DataType::Struct(_)
        | DataType::Unknown(_)) => Err(DaxError::Type(format!(
            "Unsupported dtype for numeric operation: {other:?}"
        ))),
    }
}

pub(crate) fn extract_num(v: &Value, fn_name: &str) -> DaxResult<f64> {
    match v {
        Value::Number(n) => Ok(*n),
        Value::Integer(i) => Ok(*i as f64),
        other => Err(DaxError::Type(format!(
            "{fn_name}: expected Number, got {other:?}"
        ))),
    }
}

fn apply_unary(v: &Value, name: &str, f: impl Fn(f64) -> f64) -> DaxResult<Value> {
    match v {
        Value::Integer(i) => Ok(Value::Number(f(*i as f64))),
        Value::Number(n) => Ok(Value::Number(f(*n))),
        Value::Series(s) => {
            let f64_s = to_f64_series(s)?;
            let ca = f64_s.f64().expect("to_f64_series guarantees Float64 dtype");
            Ok(Value::Series(ca.apply(|o| o.map(&f)).into_series()))
        }
        other => Err(DaxError::Type(format!(
            "{name}: expected a number, got {other:?}"
        ))),
    }
}

fn apply_unary_int(v: &Value, name: &str, f: impl Fn(f64) -> i64) -> DaxResult<Value> {
    match v {
        Value::Integer(i) => Ok(Value::Integer(f(*i as f64))),
        Value::Number(n) => Ok(Value::Integer(f(*n))),
        Value::Series(s) => {
            let f64_s = to_f64_series(s)?;
            let result = f64_s
                .f64()
                .expect("to_f64_series guarantees Float64 dtype")
                .apply(|opt| opt.map(|v| f(v) as f64))
                .into_series()
                .cast(&DataType::Int64)
                .map_err(|e| DaxError::Type(format!("{name}: cast to Int64 failed: {e}")))?;
            Ok(Value::Series(result))
        }
        other => Err(DaxError::Type(format!(
            "{name}: expected a number, got {other:?}"
        ))),
    }
}

pub fn divide(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let numerator = match &args[0] {
        Value::Number(n) => *n,
        Value::Integer(i) => *i as f64,
        other => {
            return Err(DaxError::Type(format!(
                "DIVIDE expects numbers, got {other:?}"
            )))
        }
    };
    let denominator = match &args[1] {
        Value::Number(n) => *n,
        Value::Integer(i) => *i as f64,
        other => {
            return Err(DaxError::Type(format!(
                "DIVIDE expects numbers, got {other:?}"
            )))
        }
    };
    if denominator == 0.0 {
        Ok(args.into_iter().nth(2).unwrap_or(Value::Blank))
    } else {
        Ok(Value::Number(numerator / denominator))
    }
}

pub fn abs(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Integer(i) => Ok(Value::Integer(i.abs())),
        Value::Number(n) => Ok(Value::Number(n.abs())),
        Value::Series(s) => Ok(Value::Series(match s.dtype() {
            DataType::Int64 => s
                .i64()
                .expect("dtype matched Int64 above")
                .apply(|o| o.map(|v| v.abs()))
                .into_series(),
            DataType::Int32 => s
                .i32()
                .expect("dtype matched Int32 above")
                .apply(|o| o.map(|v| v.abs()))
                .into_series(),
            _ => to_f64_series(s)?
                .f64()
                .expect("to_f64_series guarantees Float64 dtype")
                .apply(|o| o.map(|v: f64| v.abs()))
                .into_series(),
        })),
        other => Err(DaxError::Type(format!(
            "ABS expects a number, got {other:?}"
        ))),
    }
}

pub fn round(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let digits = match &args[1] {
        Value::Number(n) => *n as i32,
        Value::Integer(i) => *i as i32,
        other => {
            return Err(DaxError::Type(format!(
                "ROUND: expected digits as a number, got {other:?}"
            )))
        }
    };
    let factor = 10f64.powi(digits);
    apply_unary(&args[0], "ROUND", move |n| (n * factor).round() / factor)
}

pub fn pi_fn(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::Number(std::f64::consts::PI))
}

pub fn sqrt_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "SQRT", |n| n.sqrt())
}

pub fn exp_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "EXP", |n| n.exp())
}

pub fn ln_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "LN", |n| n.ln())
}

pub fn log_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let base = args
        .get(1)
        .map(|v| extract_num(v, "LOG"))
        .transpose()?
        .unwrap_or(10.0);
    apply_unary(&args[0], "LOG", move |n| n.log(base))
}

pub fn log10_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "LOG10", |n| n.log10())
}

pub fn floor_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "FLOOR", |n| n.floor())
}

pub fn ceiling_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "CEILING", |n| n.ceil())
}

pub fn trunc_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "TRUNC", |n| n.trunc())
}

pub fn int_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary_int(&args[0], "INT", |n| n.floor() as i64)
}

pub fn sign_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary_int(&args[0], "SIGN", |n| {
        if n > 0.0 {
            1
        } else if n < 0.0 {
            -1
        } else {
            0
        }
    })
}

pub fn power_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let exp = extract_num(&args[1], "POWER")?;
    apply_unary(&args[0], "POWER", move |base| base.powf(exp))
}

pub fn mod_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let d = extract_num(&args[1], "MOD")?;
    if d == 0.0 {
        return Err(DaxError::Eval("MOD: divisor cannot be zero".into()));
    }
    apply_unary(&args[0], "MOD", move |n| n - d * (n / d).floor())
}

pub fn fact_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Integer(i) => {
            let k = *i;
            if k < 0 {
                return Err(DaxError::InvalidArgument(
                    "FACT: argument must be >= 0".into(),
                ));
            }
            if k > 170 {
                return Ok(Value::Number(f64::INFINITY));
            }
            Ok(Value::Number((2..=k).map(|i| i as f64).product::<f64>()))
        }
        Value::Number(n) => {
            let k = *n as i64;
            if k < 0 {
                return Err(DaxError::InvalidArgument(
                    "FACT: argument must be >= 0".into(),
                ));
            }
            if k > 170 {
                return Ok(Value::Number(f64::INFINITY));
            }
            Ok(Value::Number((2..=k).map(|i| i as f64).product::<f64>()))
        }
        other => Err(DaxError::Type(format!(
            "FACT: expected a number, got {other:?}"
        ))),
    }
}

pub fn even_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary_int(&args[0], "EVEN", |n| {
        let sign = if n < 0.0 { -1i64 } else { 1i64 };
        let abs_ceil = n.abs().ceil() as i64;
        let result = if abs_ceil % 2 == 0 {
            abs_ceil
        } else {
            abs_ceil + 1
        };
        sign * result
    })
}

pub fn odd_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary_int(&args[0], "ODD", |n| {
        let sign = if n < 0.0 { -1i64 } else { 1i64 };
        let abs_ceil = n.abs().ceil() as i64;
        let result = if abs_ceil % 2 != 0 {
            abs_ceil
        } else {
            abs_ceil + 1
        };
        sign * result
    })
}

pub fn mround_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let m = extract_num(&args[1], "MROUND")?;
    if m == 0.0 {
        return Err(DaxError::Eval("MROUND: multiple cannot be zero".into()));
    }
    apply_unary(&args[0], "MROUND", move |n| (n / m).round() * m)
}

pub fn roundup_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let digits = extract_num(&args[1], "ROUNDUP")? as i32;
    let factor = 10f64.powi(digits);
    apply_unary(&args[0], "ROUNDUP", move |n| {
        let result = if n >= 0.0 {
            (n * factor).ceil()
        } else {
            (n * factor).floor()
        };
        result / factor
    })
}

pub fn rounddown_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let digits = extract_num(&args[1], "ROUNDDOWN")? as i32;
    let factor = 10f64.powi(digits);
    apply_unary(&args[0], "ROUNDDOWN", move |n| {
        (n * factor).trunc() / factor
    })
}

pub fn gcd_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    fn gcd(a: i64, b: i64) -> i64 {
        let (mut a, mut b) = (a.abs(), b.abs());
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }
    let mut result = extract_num(&args[0], "GCD")? as i64;
    for v in &args[1..] {
        result = gcd(result, extract_num(v, "GCD")? as i64);
    }
    Ok(Value::Integer(result))
}

pub fn lcm_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    fn gcd(a: i64, b: i64) -> i64 {
        let (mut a, mut b) = (a.abs(), b.abs());
        while b != 0 {
            let t = b;
            b = a % b;
            a = t;
        }
        a
    }
    fn lcm(a: i64, b: i64) -> i64 {
        if a == 0 || b == 0 {
            0
        } else {
            (a / gcd(a, b)) * b
        }
    }
    let mut result = extract_num(&args[0], "LCM")? as i64;
    for v in &args[1..] {
        result = lcm(result, extract_num(v, "LCM")? as i64);
    }
    Ok(Value::Integer(result))
}

pub fn sqrtpi_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "SQRTPI", |n| (n * std::f64::consts::PI).sqrt())
}

pub fn degrees_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "DEGREES", |n| n.to_degrees())
}

pub fn radians_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "RADIANS", |n| n.to_radians())
}

pub fn sin_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "SIN", |n| n.sin())
}

pub fn cos_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "COS", |n| n.cos())
}

pub fn tan_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "TAN", |n| n.tan())
}

pub fn asin_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ASIN", |n| n.asin())
}

pub fn acos_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ACOS", |n| n.acos())
}

pub fn atan_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ATAN", |n| n.atan())
}

pub fn atan2_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let y = extract_num(&args[1], "ATAN2")?;
    apply_unary(&args[0], "ATAN2", move |x| y.atan2(x))
}

pub fn sinh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "SINH", |n| n.sinh())
}

pub fn cosh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "COSH", |n| n.cosh())
}

pub fn tanh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "TANH", |n| n.tanh())
}

pub fn acosh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ACOSH", |n| n.acosh())
}

pub fn asinh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ASINH", |n| n.asinh())
}

pub fn atanh_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ATANH", |n| n.atanh())
}

pub fn cot_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "COT", |n| 1.0 / n.tan())
}

pub fn coth_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "COTH", |n| n.cosh() / n.sinh())
}

pub fn acot_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ACOT", |n| std::f64::consts::FRAC_PI_2 - n.atan())
}

pub fn acoth_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    apply_unary(&args[0], "ACOTH", |n| 0.5 * ((n + 1.0) / (n - 1.0)).ln())
}
