use super::math::extract_num;
use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::expressions::Value;
use crate::engine::row_context::RowContext;
use polars::prelude::*;

fn ymd_to_ms(year: i32, month: i32, day: i32) -> i64 {
    let m0 = month - 1;
    let extra_years = m0.div_euclid(12);
    let year = year + extra_years;
    let month = m0.rem_euclid(12) + 1;

    let a = (14 - month) / 12;
    let y = year + 4800 - a;
    let m = month + 12 * a - 3;
    let jdn = day + (153 * m + 2) / 5 + 365 * y + y / 4 - y / 100 + y / 400 - 32045;
    (jdn as i64 - 2_440_588) * 86_400_000
}

fn ms_to_parts(ms: i64) -> (i32, i32, i32, i32, i32, i32) {
    let total_secs = ms.div_euclid(1_000);
    let time_secs = total_secs.rem_euclid(86_400) as i32;
    let h = time_secs / 3600;
    let m = (time_secs % 3600) / 60;
    let s = time_secs % 60;

    let days = total_secs.div_euclid(86_400);
    let jd = days + 2_440_588;
    let f = jd + 1401 + (((4 * jd + 274_277) / 146_097) * 3) / 4 - 38;
    let e = 4 * f + 3;
    let g = (e % 1461) / 4;
    let dg = 5 * g + 2;
    let day = (dg % 153) / 5 + 1;
    let month = (dg / 153 + 2) % 12 + 1;
    let year = e / 1461 - 4716 + (14 - month) / 12;
    (year as i32, month as i32, day as i32, h, m, s)
}

fn current_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn local_now_ms(tz: Option<&str>) -> i64 {
    use chrono::{Local, Utc};
    match tz {
        None => {
            let local = Local::now();
            local.naive_local().and_utc().timestamp_millis()
        }
        Some(name) => {
            let tz: chrono_tz::Tz = name
                .parse()
                .expect("timezone was validated at set_timezone");
            let local = Utc::now().with_timezone(&tz);
            local.naive_local().and_utc().timestamp_millis()
        }
    }
}

fn extract_dt(v: &Value, fn_name: &str) -> DaxResult<i64> {
    match v {
        Value::DateTime(ms) => Ok(*ms),
        other => Err(DaxError::Type(format!(
            "{fn_name}: expected DateTime, got {other:?}"
        ))),
    }
}

pub fn date_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let y = extract_num(&args[0], "DATE")? as i32;
    let m = extract_num(&args[1], "DATE")? as i32;
    let d = extract_num(&args[2], "DATE")? as i32;
    Ok(Value::DateTime(ymd_to_ms(y, m, d)))
}

pub fn utctoday_fn(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let (y, mo, d, _, _, _) = ms_to_parts(current_ms());
    Ok(Value::DateTime(ymd_to_ms(y, mo, d)))
}

pub fn utcnow_fn(
    _args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::DateTime(current_ms()))
}

pub fn today_fn(
    _args: Vec<Value>,
    ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let ms = local_now_ms(ctx.timezone.as_deref());
    let (y, mo, d, _, _, _) = ms_to_parts(ms);
    Ok(Value::DateTime(ymd_to_ms(y, mo, d)))
}

pub fn now_fn(
    _args: Vec<Value>,
    ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    Ok(Value::DateTime(local_now_ms(ctx.timezone.as_deref())))
}

pub fn year_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("YEAR expects a DateTime series".into()))?
                .year()
                .into_series(),
        )),
        _ => {
            let (y, _, _, _, _, _) = ms_to_parts(extract_dt(&args[0], "YEAR")?);
            Ok(Value::Integer(y as i64))
        }
    }
}

pub fn month_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("MONTH expects a DateTime series".into()))?
                .month()
                .into_series(),
        )),
        _ => {
            let (_, mo, _, _, _, _) = ms_to_parts(extract_dt(&args[0], "MONTH")?);
            Ok(Value::Integer(mo as i64))
        }
    }
}

pub fn day_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("DAY expects a DateTime series".into()))?
                .day()
                .into_series(),
        )),
        _ => {
            let (_, _, d, _, _, _) = ms_to_parts(extract_dt(&args[0], "DAY")?);
            Ok(Value::Integer(d as i64))
        }
    }
}

pub fn hour_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("HOUR expects a DateTime series".into()))?
                .hour()
                .into_series(),
        )),
        _ => {
            let (_, _, _, h, _, _) = ms_to_parts(extract_dt(&args[0], "HOUR")?);
            Ok(Value::Integer(h as i64))
        }
    }
}

pub fn minute_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("MINUTE expects a DateTime series".into()))?
                .minute()
                .into_series(),
        )),
        _ => {
            let (_, _, _, _, m, _) = ms_to_parts(extract_dt(&args[0], "MINUTE")?);
            Ok(Value::Integer(m as i64))
        }
    }
}

pub fn second_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("SECOND expects a DateTime series".into()))?
                .second()
                .into_series(),
        )),
        _ => {
            let (_, _, _, _, _, s) = ms_to_parts(extract_dt(&args[0], "SECOND")?);
            Ok(Value::Integer(s as i64))
        }
    }
}

pub fn quarter_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    match &args[0] {
        Value::Series(s) => Ok(Value::Series(
            s.datetime()
                .map_err(|_| DaxError::Type("QUARTER expects a DateTime series".into()))?
                .quarter()
                .into_series(),
        )),
        _ => {
            let (_, mo, _, _, _, _) = ms_to_parts(extract_dt(&args[0], "QUARTER")?);
            Ok(Value::Integer(((mo - 1) / 3 + 1) as i64))
        }
    }
}

/// DAX weekday return_type semantics:
///   1 (default) → Sun=1, Mon=2, …, Sat=7
///   2           → Mon=1, Tue=2, …, Sun=7
///   3           → Mon=0, Tue=1, …, Sun=6
///
/// Polars weekday: Mon=0 … Sun=6.
pub fn weekday_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let return_type = args
        .get(1)
        .map(|v| extract_num(v, "WEEKDAY"))
        .transpose()?
        .map(|n| n as i32)
        .unwrap_or(1);
    match &args[0] {
        Value::Series(s) => {
            let ms_series = s
                .cast(&DataType::Int64)
                .map_err(|_| DaxError::Type("WEEKDAY expects a DateTime series".into()))?;
            let ms_i64 = ms_series
                .i64()
                .map_err(|_| DaxError::Type("WEEKDAY: Int64 cast failed".into()))?;
            let mapped: Int64Chunked = ms_i64.apply(|opt| {
                opt.map(|ms| {
                    let days = ms.div_euclid(86_400_000);
                    let polars_wd = ((days + 3).rem_euclid(7)) as i32;
                    weekday_remap(polars_wd, return_type) as i64
                })
            });
            Ok(Value::Series(mapped.into_series()))
        }
        _ => {
            let ms = extract_dt(&args[0], "WEEKDAY")?;
            let days = ms.div_euclid(86_400_000);
            let polars_wd = ((days + 3).rem_euclid(7)) as i32;
            Ok(Value::Integer(weekday_remap(polars_wd, return_type) as i64))
        }
    }
}

fn weekday_remap(polars_wd: i32, return_type: i32) -> i32 {
    match return_type {
        2 => polars_wd + 1,           // Mon=1 … Sun=7
        3 => polars_wd,               // Mon=0 … Sun=6
        _ => (polars_wd + 1) % 7 + 1, // Sun=1, Mon=2 … Sat=7
    }
}

/// DAX weeknum return_type semantics:
///   1 (default) → week starts Sunday,  Jan 1 is always in week 1
///   2           → week starts Monday,  Jan 1 is always in week 1
pub fn weeknum_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let return_type = args
        .get(1)
        .map(|v| extract_num(v, "WEEKNUM"))
        .transpose()?
        .map(|n| n as i32)
        .unwrap_or(1);
    match &args[0] {
        Value::Series(s) => {
            let dt = s
                .datetime()
                .map_err(|_| DaxError::Type("WEEKNUM expects a DateTime series".into()))?;
            let series = if return_type == 2 {
                dt.week().into_series()
            } else {
                let shifted = (s
                    .cast(&DataType::Int64)
                    .map_err(|e| DaxError::Type(format!("WEEKNUM: cast failed: {e}")))?)
                    + Series::new("".into(), &[86_400_000i64]);
                let shifted_dt = shifted
                    .expect("length-1 broadcast add of same-dtype Int64 series cannot fail")
                    .cast(&DataType::Datetime(
                        polars::prelude::TimeUnit::Milliseconds,
                        None,
                    ))
                    .map_err(|e| DaxError::Type(format!("WEEKNUM: datetime cast failed: {e}")))?;
                shifted_dt
                    .datetime()
                    .expect("cast to Datetime above guarantees this")
                    .week()
                    .into_series()
            };
            Ok(Value::Series(series))
        }
        _ => {
            let ms = extract_dt(&args[0], "WEEKNUM")?;
            Ok(Value::Integer(weeknum_scalar(ms, return_type) as i64))
        }
    }
}

fn weeknum_scalar(ms: i64, return_type: i32) -> i32 {
    let (y, mo, d, _, _, _) = ms_to_parts(ms);
    let jan1_ms = ymd_to_ms(y, 1, 1);
    let day_of_year = ((ms - jan1_ms) / 86_400_000) as i32;
    let jan1_days = jan1_ms.div_euclid(86_400_000);
    let jan1_wd = ((jan1_days + 3).rem_euclid(7)) as i32;
    let _ = (mo, d);
    if return_type == 2 {
        (day_of_year + jan1_wd) / 7 + 1
    } else {
        let sun_based_jan1 = (jan1_wd + 1) % 7;
        (day_of_year + sun_based_jan1) / 7 + 1
    }
}

pub fn edate_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let ms = extract_dt(&args[0], "EDATE")?;
    let months = extract_num(&args[1], "EDATE")? as i32;
    let (y, mo, d, _, _, _) = ms_to_parts(ms);
    Ok(Value::DateTime(ymd_to_ms(y, mo + months, d)))
}

pub fn eomonth_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let ms = extract_dt(&args[0], "EOMONTH")?;
    let months = extract_num(&args[1], "EOMONTH")? as i32;
    let (y, mo, _, _, _, _) = ms_to_parts(ms);
    Ok(Value::DateTime(
        ymd_to_ms(y, mo + months + 1, 1) - 86_400_000,
    ))
}

pub fn datediff_fn(
    args: Vec<Value>,
    _ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let ms1 = extract_dt(&args[0], "DATEDIFF")?;
    let ms2 = extract_dt(&args[1], "DATEDIFF")?;
    let unit = match &args[2] {
        Value::String(s) => s.to_ascii_uppercase(),
        other => {
            return Err(DaxError::Type(format!(
                "DATEDIFF: unit must be a string, got {other:?}"
            )))
        }
    };
    let diff: i64 = match unit.as_str() {
        "SECOND" => (ms2 - ms1) / 1_000,
        "MINUTE" => (ms2 - ms1) / 60_000,
        "HOUR" => (ms2 - ms1) / 3_600_000,
        "DAY" => (ms2 - ms1) / 86_400_000,
        "WEEK" => (ms2 - ms1) / (86_400_000 * 7),
        "MONTH" => {
            let (y1, mo1, _, _, _, _) = ms_to_parts(ms1);
            let (y2, mo2, _, _, _, _) = ms_to_parts(ms2);
            ((y2 - y1) * 12 + (mo2 - mo1)) as i64
        }
        "QUARTER" => {
            let (y1, mo1, _, _, _, _) = ms_to_parts(ms1);
            let (y2, mo2, _, _, _, _) = ms_to_parts(ms2);
            (((y2 - y1) * 12 + (mo2 - mo1)) / 3) as i64
        }
        "YEAR" => {
            let (y1, _, _, _, _, _) = ms_to_parts(ms1);
            let (y2, _, _, _, _, _) = ms_to_parts(ms2);
            (y2 - y1) as i64
        }
        other => {
            return Err(DaxError::InvalidArgument(format!(
                "DATEDIFF: unknown interval unit '{other}'"
            )))
        }
    };
    Ok(Value::Number(diff as f64))
}

pub fn datevalue_fn(
    args: Vec<Value>,
    ctx: &ExecutionContext,
    _fc: &FilterContext,
    _rc: &RowContext,
) -> DaxResult<Value> {
    let culture = ctx.catalog.culture.as_deref().unwrap_or("en-US");
    let text = match &args[0] {
        Value::Blank => return Ok(Value::Blank),
        Value::DateTime(ms) => return Ok(Value::DateTime(ms - ms.rem_euclid(86_400_000))),
        Value::String(s) => s.clone(),
        Value::Integer(n) => n.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string().to_uppercase(),
        other => {
            return Err(DaxError::Type(format!(
                "DATEVALUE: expected text, got {other:?}"
            )))
        }
    };
    Ok(Value::DateTime(parse_date_text(&text, culture)?))
}

fn parse_date_text(text: &str, culture: &str) -> DaxResult<i64> {
    let invalid = || {
        DaxError::InvalidArgument(format!(
            "DATEVALUE: cannot parse '{}' as a date",
            text.trim()
        ))
    };
    let mut words: Vec<&str> = text.split_whitespace().collect();
    if words.len() > 2
        && words
            .last()
            .is_some_and(|w| w.eq_ignore_ascii_case("AM") || w.eq_ignore_ascii_case("PM"))
    {
        words.pop();
    }
    if words.len() > 1 && words.last().is_some_and(|w| w.contains(':')) {
        words.pop();
    }
    let date = words.join(" ");
    parse_numeric_date(&date, culture)
        .or_else(|| parse_named_date(&date))
        .map(|d| ymd_to_ms(d.0, d.1, d.2))
        .ok_or_else(invalid)
}

/// `Y-M-D` when the first part has four digits, otherwise the culture's order.
/// `en-US` is month first and falls back to day first when that is invalid.
fn parse_numeric_date(text: &str, culture: &str) -> Option<(i32, i32, i32)> {
    let sep = text.chars().find(|c| !c.is_ascii_digit())?;
    if !matches!(sep, '/' | '-' | '.') {
        return None;
    }
    let parts: Vec<&str> = text.split(sep).collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    let num = |p: &str| p.parse::<i32>().ok();
    let year = |p: &str| {
        // .NET two-digit year window: 00-49 is 20xx, 50-99 is 19xx.
        let y = num(p)?;
        Some(match p.len() {
            1 | 2 if y < 50 => 2000 + y,
            1 | 2 => 1900 + y,
            _ => y,
        })
    };
    let valid = |y: i32, m: i32, d: i32| {
        chrono::NaiveDate::from_ymd_opt(y, m as u32, d as u32).map(|_| (y, m, d))
    };
    if parts[0].len() == 4 {
        return valid(num(parts[0])?, num(parts[1])?, num(parts[2])?);
    }
    let (a, b, y) = (num(parts[0])?, num(parts[1])?, year(parts[2])?);
    if culture.eq_ignore_ascii_case("en-US") {
        valid(y, a, b).or_else(|| valid(y, b, a))
    } else {
        valid(y, b, a)
    }
}

/// English month names, e.g. `1 Aug 2009` or `August 1, 2009`. A missing year
/// means the current year.
fn parse_named_date(text: &str) -> Option<(i32, i32, i32)> {
    use chrono::Datelike;
    const FORMATS: [&str; 4] = ["%d %B %Y", "%d %b %Y", "%B %d %Y", "%b %d %Y"];
    let text = text.replace(',', " ");
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let year = chrono::Local::now().year();
    [text.clone(), format!("{text} {year}")]
        .iter()
        .flat_map(|t| {
            FORMATS
                .iter()
                .map(move |f| chrono::NaiveDate::parse_from_str(t, f))
        })
        .find_map(Result::ok)
        .map(|d| (d.year(), d.month() as i32, d.day() as i32))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_text_accepts_supported_formats() {
        let expected = ymd_to_ms(2009, 8, 1);
        for text in [
            "2009-08-01",
            "2009/8/1",
            "8/1/2009",
            "1 August 2009",
            "1 Aug 2009",
            "August 1, 2009",
            "Aug 1, 2009",
            "  2009-08-01  ",
            "2009-08-01 10:30",
        ] {
            assert_eq!(
                parse_date_text(text, "en-US").unwrap(),
                expected,
                "input: {text}"
            );
        }
    }

    #[test]
    fn parse_date_text_reads_numeric_dates_by_culture() {
        let expected = ymd_to_ms(2009, 12, 31);
        assert_eq!(parse_date_text("12/31/2009", "en-US").unwrap(), expected);
        for culture in ["en-GB", "pt-PT", "de-DE"] {
            for text in ["31/12/2009", "31.12.2009", "31-12-2009", "2009-12-31"] {
                assert_eq!(
                    parse_date_text(text, culture).unwrap(),
                    expected,
                    "input: {text}, culture: {culture}"
                );
            }
        }
        assert_eq!(
            parse_date_text("1/8/2009", "en-GB").unwrap(),
            ymd_to_ms(2009, 8, 1)
        );
        assert!(parse_date_text("12/31/2009", "en-GB").is_err());
    }

    #[test]
    fn parse_date_text_rejects_invalid_dates() {
        for text in ["2023-02-30", "2023-13-01", "2020-01-01-x", "not a date", ""] {
            assert!(
                matches!(
                    parse_date_text(text, "en-US"),
                    Err(DaxError::InvalidArgument(_))
                ),
                "input: {text}"
            );
        }
    }

    #[test]
    fn parse_date_text_rejects_iso_t_separator() {
        for text in ["2009-08-01T10:30:00", "2009-08-01T10:30"] {
            assert!(parse_date_text(text, "en-US").is_err(), "input: {text}");
        }
    }

    #[test]
    fn parse_date_text_strips_time_after_space() {
        for text in ["2009-08-01 10:30", "8/1/2009 10:30", "8/1/2009 10:30:00 AM"] {
            assert_eq!(
                parse_date_text(text, "en-US").ok(),
                Some(ymd_to_ms(2009, 8, 1)),
                "input: {text}"
            );
        }
    }

    #[test]
    fn parse_date_text_named_month_with_time() {
        assert_eq!(
            parse_date_text("August 1, 2009 10:30", "en-US").ok(),
            Some(ymd_to_ms(2009, 8, 1))
        );
    }

    #[test]
    fn parse_date_text_en_us_falls_back_to_day_first() {
        assert_eq!(
            parse_date_text("31/12/2009", "en-US").ok(),
            Some(ymd_to_ms(2009, 12, 31))
        );
        assert_eq!(
            parse_date_text("8/1/2009", "en-US").ok(),
            Some(ymd_to_ms(2009, 8, 1))
        );
    }

    #[test]
    fn parse_date_text_two_digit_year() {
        assert_eq!(
            parse_date_text("8/1/09", "en-US").ok(),
            Some(ymd_to_ms(2009, 8, 1))
        );
    }

    #[test]
    fn parse_date_text_short_dash_date_uses_culture_order() {
        assert_eq!(
            parse_date_text("10-11-12", "en-US").ok(),
            Some(ymd_to_ms(2012, 10, 11))
        );
    }

    #[test]
    fn parse_date_text_missing_year_uses_current_year() {
        use chrono::Datelike;
        let year = chrono::Local::now().year();
        assert_eq!(
            parse_date_text("Aug 1", "en-US").ok(),
            Some(ymd_to_ms(year, 8, 1))
        );
    }
}
