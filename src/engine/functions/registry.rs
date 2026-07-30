use std::collections::HashMap;

use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::error::DaxResult;
use crate::engine::expressions::Value;
use crate::engine::ir::expr_node::BoundExprNode;
use crate::engine::row_context::RowContext;
use polars::prelude::{DataType, TimeUnit};

pub struct ParamMeta {
    pub name: &'static str,
    pub description: &'static str,
    pub optional: bool,
    pub repeatable: bool,
}

pub struct FunctionMeta {
    pub description: &'static str,
    pub interface_name: &'static str,
    pub params: Vec<ParamMeta>,
}

pub type DaxFn = fn(
    args: Vec<Value>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
) -> DaxResult<Value>;

pub type ContextFn = fn(
    args: Vec<BoundExprNode>,
    ctx: &ExecutionContext,
    fc: &FilterContext,
    rc: &RowContext,
    eval: &dyn Fn(BoundExprNode, &FilterContext, &RowContext) -> DaxResult<Value>,
) -> DaxResult<Value>;

#[derive(Debug, Clone, Copy)]
pub enum ReturnType {
    Float,
    Int,
    IntSmall,
    Boolean,
    DateTime,
    /// Int64 when the first argument is an integer type, Float64 otherwise.
    SameNumeric,
    /// Table-valued — no scalar DataType.
    Table,
    /// No intrinsic type of its own (e.g. BLANK()) — always None.
    Untyped,
    /// Whatever the Nth argument's dtype is (e.g. SELECTEDVALUE/RELATED taking
    /// their type from the referenced column).
    SameAsArg(usize),
    /// Whichever of the two given argument indices has a known dtype, trying
    /// `a` first — for IF's true/false branches, where either may be an
    /// untyped BLANK() and the other carries the real type.
    SameAsEitherArg(usize, usize),
}

impl ReturnType {
    pub fn to_dtype(self, arg_dtypes: &[Option<DataType>]) -> Option<DataType> {
        match self {
            ReturnType::Float => Some(DataType::Float64),
            ReturnType::Int => Some(DataType::Int64),
            ReturnType::IntSmall => Some(DataType::Int32),
            ReturnType::Boolean => Some(DataType::Boolean),
            ReturnType::DateTime => Some(DataType::Datetime(TimeUnit::Milliseconds, None)),
            ReturnType::SameNumeric => Some(match arg_dtypes.first().and_then(Option::as_ref) {
                Some(DataType::Int64 | DataType::Int32) => DataType::Int64,
                _ => DataType::Float64,
            }),
            ReturnType::Table => None,
            ReturnType::Untyped => None,
            ReturnType::SameAsArg(idx) => arg_dtypes.get(idx).cloned().flatten(),
            ReturnType::SameAsEitherArg(a, b) => arg_dtypes
                .get(a)
                .cloned()
                .flatten()
                .or_else(|| arg_dtypes.get(b).cloned().flatten()),
        }
    }
}

pub enum FunctionEntry {
    CallByValue(DaxFn, ReturnType),
    Context(ContextFn, ReturnType),
}

impl FunctionEntry {
    pub fn return_type(&self) -> ReturnType {
        match self {
            FunctionEntry::CallByValue(_, rt) => *rt,
            FunctionEntry::Context(_, rt) => *rt,
        }
    }
}

pub struct FunctionRegistry {
    functions: HashMap<&'static str, FunctionEntry>,
    pub meta: HashMap<&'static str, FunctionMeta>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        use ReturnType::*;
        let mut reg = Self { functions: HashMap::new(), meta: HashMap::new() };

        reg.register(
            "SUM",
            crate::engine::functions::aggregation::sum,
            SameNumeric,
        );
        reg.register("COUNT", crate::engine::functions::aggregation::count, Float);
        reg.register(
            "COUNTA",
            crate::engine::functions::aggregation::counta,
            Float,
        );
        reg.register(
            "AVERAGE",
            crate::engine::functions::aggregation::average,
            Float,
        );
        reg.register(
            "AVERAGEA",
            crate::engine::functions::aggregation::averagea,
            Float,
        );
        reg.register(
            "MIN",
            crate::engine::functions::aggregation::min,
            SameNumeric,
        );
        reg.register("MINA", crate::engine::functions::aggregation::mina, Float);
        reg.register(
            "MAX",
            crate::engine::functions::aggregation::max,
            SameNumeric,
        );
        reg.register("MAXA", crate::engine::functions::aggregation::maxa, Float);
        reg.register("DIVIDE", crate::engine::functions::math::divide, Float);
        reg.register("ABS", crate::engine::functions::math::abs, Float);
        reg.register("ROUND", crate::engine::functions::math::round, Float);
        reg.register(
            "COUNTROWS",
            crate::engine::functions::aggregation::countrows,
            Float,
        );
        reg.register(
            "ISEMPTY",
            crate::engine::functions::aggregation::isempty,
            Boolean,
        );
        reg.register(
            "DISTINCTCOUNT",
            crate::engine::functions::aggregation::distinctcount,
            Float,
        );
        reg.register(
            "VALUES",
            crate::engine::functions::aggregation::values_fn,
            Table,
        );
        reg.register("ROW", crate::engine::functions::aggregation::row_fn, Table);
        reg.register(
            "DISTINCT",
            crate::engine::functions::aggregation::distinct_fn,
            Table,
        );
        reg.register(
            "EXCEPT",
            crate::engine::functions::aggregation::except_fn,
            Table,
        );
        reg.register(
            "INTERSECT",
            crate::engine::functions::aggregation::intersect_fn,
            Table,
        );
        reg.register(
            "UNION",
            crate::engine::functions::aggregation::union_fn,
            Table,
        );
        reg.register(
            "NATURALLEFTOUTERJOIN",
            crate::engine::functions::aggregation::natural_left_outer_join_fn,
            Table,
        );
        reg.register(
            "NATURALINNERJOIN",
            crate::engine::functions::aggregation::natural_inner_join_fn,
            Table,
        );
        reg.register(
            "HASONEVALUE",
            crate::engine::functions::aggregation::hasonevalue,
            Boolean,
        );
        reg.register("BLANK", crate::engine::functions::logical::blank, Untyped);
        reg.register("TRUE", crate::engine::functions::logical::true_fn, Boolean);
        reg.register(
            "FALSE",
            crate::engine::functions::logical::false_fn,
            Boolean,
        );
        reg.register("AND", crate::engine::functions::logical::and, Boolean);
        reg.register("OR", crate::engine::functions::logical::or, Boolean);
        reg.register("NOT", crate::engine::functions::logical::not, Boolean);
        reg.register(
            "ISBLANK",
            crate::engine::functions::logical::isblank,
            Boolean,
        );
        reg.register_context(
            "KEEPFILTERS",
            crate::engine::context_functions::keepfilters_fn,
            Table,
        );
        reg.register_context(
            "SELECTCOLUMNS",
            crate::engine::context_functions::selectcolumns_fn,
            Table,
        );
        reg.register(
            "DATE",
            crate::engine::functions::datetime::date_fn,
            DateTime,
        );
        reg.register(
            "UTCTODAY",
            crate::engine::functions::datetime::utctoday_fn,
            DateTime,
        );
        reg.register(
            "UTCNOW",
            crate::engine::functions::datetime::utcnow_fn,
            DateTime,
        );
        reg.register(
            "TODAY",
            crate::engine::functions::datetime::today_fn,
            DateTime,
        );
        reg.register("NOW", crate::engine::functions::datetime::now_fn, DateTime);
        reg.register(
            "YEAR",
            crate::engine::functions::datetime::year_fn,
            IntSmall,
        );
        reg.register(
            "MONTH",
            crate::engine::functions::datetime::month_fn,
            IntSmall,
        );
        reg.register("DAY", crate::engine::functions::datetime::day_fn, IntSmall);
        reg.register(
            "HOUR",
            crate::engine::functions::datetime::hour_fn,
            IntSmall,
        );
        reg.register(
            "MINUTE",
            crate::engine::functions::datetime::minute_fn,
            IntSmall,
        );
        reg.register(
            "SECOND",
            crate::engine::functions::datetime::second_fn,
            IntSmall,
        );
        reg.register(
            "EDATE",
            crate::engine::functions::datetime::edate_fn,
            DateTime,
        );
        reg.register(
            "EOMONTH",
            crate::engine::functions::datetime::eomonth_fn,
            DateTime,
        );
        reg.register(
            "DATEDIFF",
            crate::engine::functions::datetime::datediff_fn,
            Float,
        );
        reg.register(
            "QUARTER",
            crate::engine::functions::datetime::quarter_fn,
            IntSmall,
        );
        reg.register(
            "WEEKDAY",
            crate::engine::functions::datetime::weekday_fn,
            IntSmall,
        );
        reg.register(
            "WEEKNUM",
            crate::engine::functions::datetime::weeknum_fn,
            IntSmall,
        );

        reg.register("PI", crate::engine::functions::math::pi_fn, Float);
        reg.register("SQRT", crate::engine::functions::math::sqrt_fn, Float);
        reg.register("EXP", crate::engine::functions::math::exp_fn, Float);
        reg.register("LN", crate::engine::functions::math::ln_fn, Float);
        reg.register("LOG", crate::engine::functions::math::log_fn, Float);
        reg.register("LOG10", crate::engine::functions::math::log10_fn, Float);
        reg.register("FLOOR", crate::engine::functions::math::floor_fn, Float);
        reg.register("CEILING", crate::engine::functions::math::ceiling_fn, Float);
        reg.register("TRUNC", crate::engine::functions::math::trunc_fn, Float);
        reg.register("INT", crate::engine::functions::math::int_fn, Int);
        reg.register("SIGN", crate::engine::functions::math::sign_fn, Int);
        reg.register("POWER", crate::engine::functions::math::power_fn, Float);
        reg.register("MOD", crate::engine::functions::math::mod_fn, Float);
        reg.register("FACT", crate::engine::functions::math::fact_fn, Float);
        reg.register("EVEN", crate::engine::functions::math::even_fn, Int);
        reg.register("ODD", crate::engine::functions::math::odd_fn, Int);
        reg.register("MROUND", crate::engine::functions::math::mround_fn, Float);
        reg.register("ROUNDUP", crate::engine::functions::math::roundup_fn, Float);
        reg.register(
            "ROUNDDOWN",
            crate::engine::functions::math::rounddown_fn,
            Float,
        );
        reg.register("GCD", crate::engine::functions::math::gcd_fn, Int);
        reg.register("LCM", crate::engine::functions::math::lcm_fn, Int);
        reg.register("SQRTPI", crate::engine::functions::math::sqrtpi_fn, Float);
        reg.register("DEGREES", crate::engine::functions::math::degrees_fn, Float);
        reg.register("RADIANS", crate::engine::functions::math::radians_fn, Float);
        reg.register("SIN", crate::engine::functions::math::sin_fn, Float);
        reg.register("COS", crate::engine::functions::math::cos_fn, Float);
        reg.register("TAN", crate::engine::functions::math::tan_fn, Float);
        reg.register("ASIN", crate::engine::functions::math::asin_fn, Float);
        reg.register("ACOS", crate::engine::functions::math::acos_fn, Float);
        reg.register("ATAN", crate::engine::functions::math::atan_fn, Float);
        reg.register("ATAN2", crate::engine::functions::math::atan2_fn, Float);
        reg.register("SINH", crate::engine::functions::math::sinh_fn, Float);
        reg.register("COSH", crate::engine::functions::math::cosh_fn, Float);
        reg.register("TANH", crate::engine::functions::math::tanh_fn, Float);
        reg.register("ACOSH", crate::engine::functions::math::acosh_fn, Float);
        reg.register("ASINH", crate::engine::functions::math::asinh_fn, Float);
        reg.register("ATANH", crate::engine::functions::math::atanh_fn, Float);
        reg.register("COT", crate::engine::functions::math::cot_fn, Float);
        reg.register("COTH", crate::engine::functions::math::coth_fn, Float);
        reg.register("ACOT", crate::engine::functions::math::acot_fn, Float);
        reg.register("ACOTH", crate::engine::functions::math::acoth_fn, Float);

        reg.register_context("FILTER", crate::engine::context_functions::filter_fn, Table);
        reg.register_context("SUMX", crate::engine::context_functions::sumx_fn, Float);
        reg.register_context(
            "AVERAGEX",
            crate::engine::context_functions::averagex_fn,
            Float,
        );
        reg.register_context(
            "MAXX",
            crate::engine::context_functions::maxx_fn,
            SameNumeric,
        );
        reg.register_context(
            "MINX",
            crate::engine::context_functions::minx_fn,
            SameNumeric,
        );
        reg.register_context("COUNTX", crate::engine::context_functions::countx_fn, Float);
        reg.register_context(
            "COUNTAX",
            crate::engine::context_functions::countax_fn,
            Float,
        );
        reg.register_context(
            "IF",
            crate::engine::context_functions::if_fn,
            SameAsEitherArg(1, 2),
        );
        reg.register(
            "ERROR",
            crate::engine::functions::logical::error_fn,
            Boolean,
        );
        reg.register_context(
            "SELECTEDVALUE",
            crate::engine::context_functions::selectedvalue_fn,
            SameAsArg(0),
        );
        reg.register_context(
            "RELATED",
            crate::engine::context_functions::related_fn,
            SameAsArg(0),
        );
        reg.register_context(
            "RELATEDTABLE",
            crate::engine::context_functions::relatedtable_fn,
            Table,
        );
        reg.register_context(
            "ISINSCOPE",
            crate::engine::context_functions::isinscope_fn,
            Boolean,
        );
        reg.register_context(
            "ISFILTERED",
            crate::engine::context_functions::isfiltered_fn,
            Boolean,
        );
        reg.register_context(
            "SWITCH",
            crate::engine::context_functions::switch_fn,
            Boolean,
        );
        reg.register_context("TOPN", crate::engine::context_functions::topn_fn, Table);
        reg.register_context("SAMPLE", crate::engine::context_functions::sample_fn, Table);
        reg.register_context("ALL", crate::engine::context_functions::all_fn, Table);
        reg.register_context(
            "ALLSELECTED",
            crate::engine::context_functions::allselected_fn,
            Table,
        );
        reg.register_context(
            "ALLEXCEPT",
            crate::engine::context_functions::allexcept_fn,
            Table,
        );
        reg.register_context(
            "REMOVEFILTERS",
            crate::engine::context_functions::removefilters_fn,
            Table,
        );
        reg.register_context(
            "CONTAINS",
            crate::engine::context_functions::contains_fn,
            Boolean,
        );
        reg.register_context(
            "LOOKUPVALUE",
            crate::engine::context_functions::lookupvalue_fn,
            Table,
        );
        reg.register_context(
            "ISSUBTOTAL",
            crate::engine::context_functions::issubtotal_fn,
            Boolean,
        );
        reg.register_context(
            "TREATAS",
            crate::engine::context_functions::treatas_fn,
            Table,
        );
        reg.register_context(
            "USERELATIONSHIP",
            crate::engine::context_functions::userelationship_fn,
            Table,
        );
        reg.register_context(
            "CROSSFILTER",
            crate::engine::context_functions::crossfilter_fn,
            Table,
        );
        reg.register_context(
            "SUBSTITUTEWITHINDEX",
            crate::engine::context_functions::substitutewithindex_fn,
            Table,
        );
        reg.register_context(
            "CROSSJOIN",
            crate::engine::context_functions::crossjoin_fn,
            Table,
        );
        reg.register_context(
            "GENERATE",
            crate::engine::context_functions::generate_fn,
            Table,
        );
        reg.register_context(
            "GENERATEALL",
            crate::engine::context_functions::generateall_fn,
            Table,
        );
        reg.register_context(
            "CURRENTGROUP",
            crate::engine::context_functions::currentgroup_fn,
            Table,
        );
        reg.register_context(
            "GROUPBY",
            crate::engine::context_functions::groupby_fn,
            Table,
        );
        reg.register_context(
            "ADDCOLUMNS",
            crate::engine::context_functions::addcolumns_fn,
            Table,
        );

        reg.populate_meta();
        reg
    }

    pub fn register_meta(&mut self, name: &'static str, meta: FunctionMeta) {
        self.meta.insert(name, meta);
    }

    pub fn iter_meta(&self) -> impl Iterator<Item = (&str, &FunctionMeta)> {
        self.meta.iter().map(|(k, v)| (*k, v))
    }

    pub fn get_meta(&self, name: &str) -> Option<&FunctionMeta> {
        self.meta.get(name)
    }

    fn populate_meta(&mut self) {
        macro_rules! p {
            ($n:literal, $d:literal) => {
                ParamMeta {
                    name: $n,
                    description: $d,
                    optional: false,
                    repeatable: false,
                }
            };
            ($n:literal, $d:literal, opt) => {
                ParamMeta { name: $n, description: $d, optional: true, repeatable: false }
            };
            ($n:literal, $d:literal, rep) => {
                ParamMeta { name: $n, description: $d, optional: false, repeatable: true }
            };
            ($n:literal, $d:literal, opt rep) => {
                ParamMeta { name: $n, description: $d, optional: true, repeatable: true }
            };
        }
        macro_rules! m {
            ($name:literal, $iface:literal, $desc:literal, [$($p:expr),* $(,)?]) => {
                self.register_meta($name, FunctionMeta {
                    description: $desc,
                    interface_name: $iface,
                    params: vec![$($p),*],
                })
            };
        }

        // Aggregation / iterator
        m!(
            "SUM",
            "STATISTICAL",
            "Returns the sum of all values in a column.",
            [p!("column", "The numeric column to sum.")]
        );
        m!(
            "COUNT",
            "STATISTICAL",
            "Counts the number of non-blank numeric values in a column.",
            [p!("column", "The column to count.")]
        );
        m!(
            "COUNTA",
            "STATISTICAL",
            "Counts all non-blank values in a column (numbers, text, booleans).",
            [p!("column", "The column to count.")]
        );
        m!(
            "AVERAGE",
            "STATISTICAL",
            "Returns the arithmetic mean of all values in a column.",
            [p!("column", "The numeric column to average.")]
        );
        m!(
            "AVERAGEA",
            "STATISTICAL",
            "Returns the average treating text as 0 and booleans as 1/0.",
            [p!("column", "The column to average.")]
        );
        m!(
            "MIN",
            "STATISTICAL",
            "Returns the smallest value in a column.",
            [p!("column", "The column to find the minimum of.")]
        );
        m!(
            "MINA",
            "STATISTICAL",
            "Returns the minimum treating text as 0 and booleans as 1/0.",
            [p!("column", "The column to find the minimum of.")]
        );
        m!(
            "MAX",
            "STATISTICAL",
            "Returns the largest value in a column.",
            [p!("column", "The column to find the maximum of.")]
        );
        m!(
            "MAXA",
            "STATISTICAL",
            "Returns the maximum treating text as 0 and booleans as 1/0.",
            [p!("column", "The column to find the maximum of.")]
        );
        m!(
            "COUNTROWS",
            "STATISTICAL",
            "Counts the number of rows in a table.",
            [p!("table", "The table to count rows in.", opt)]
        );
        m!(
            "ISEMPTY",
            "STATISTICAL",
            "Returns TRUE if the specified table expression returns an empty table.",
            [p!("table", "The table expression to test.")]
        );
        m!(
            "DISTINCTCOUNT",
            "STATISTICAL",
            "Counts the number of distinct values in a column.",
            [p!("column", "The column to count distinct values in.")]
        );
        m!(
            "HASONEVALUE",
            "STATISTICAL",
            "Returns TRUE when a column is filtered to exactly one distinct value.",
            [p!("column", "The column to test.")]
        );
        m!(
            "SUMX",
            "STATISTICAL",
            "Returns the sum of an expression evaluated for each row in a table.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to sum.")
            ]
        );
        m!(
            "AVERAGEX",
            "STATISTICAL",
            "Calculates the average of an expression evaluated for each row.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to average.")
            ]
        );
        m!(
            "MAXX",
            "STATISTICAL",
            "Returns the maximum of an expression evaluated for each row.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to maximize.")
            ]
        );
        m!(
            "MINX",
            "STATISTICAL",
            "Returns the minimum of an expression evaluated for each row.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to minimize.")
            ]
        );
        m!(
            "COUNTX",
            "STATISTICAL",
            "Counts non-blank numeric results of an expression for each row.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to count.")
            ]
        );
        m!(
            "COUNTAX",
            "STATISTICAL",
            "Counts all non-blank results (numbers, text, booleans) for each row.",
            [
                p!("table", "The table to iterate."),
                p!("expression", "The expression to count.")
            ]
        );

        // Date and time
        m!(
            "DATE",
            "DATETIME",
            "Returns the specified date in datetime format.",
            [
                p!("year", "The year."),
                p!("month", "The month (1–12)."),
                p!("day", "The day (1–31).")
            ]
        );
        m!(
            "DATEDIFF",
            "DATETIME",
            "Returns the number of interval boundaries crossed between two dates.",
            [
                p!("date1", "The start date."),
                p!("date2", "The end date."),
                p!(
                    "interval",
                    "The interval unit (YEAR, QUARTER, MONTH, DAY, HOUR, MINUTE, SECOND)."
                )
            ]
        );
        m!(
            "EDATE",
            "DATETIME",
            "Returns the date a given number of months before or after a start date.",
            [
                p!("startDate", "The start date."),
                p!("months", "The number of months to add or subtract.")
            ]
        );
        m!(
            "EOMONTH",
            "DATETIME",
            "Returns the last day of the month a given number of months from a start date.",
            [
                p!("startDate", "The start date."),
                p!("months", "The number of months to add or subtract.")
            ]
        );
        m!("UTCTODAY", "DATETIME", "Returns the current UTC date.", []);
        m!(
            "UTCNOW",
            "DATETIME",
            "Returns the current UTC date and time.",
            []
        );
        m!("TODAY", "DATETIME", "Returns the current date.", []);
        m!("NOW", "DATETIME", "Returns the current date and time.", []);
        m!(
            "YEAR",
            "DATETIME",
            "Returns the year of a date as a four-digit integer.",
            [p!("date", "A date value.")]
        );
        m!(
            "MONTH",
            "DATETIME",
            "Returns the month as a number from 1 to 12.",
            [p!("date", "A date value.")]
        );
        m!(
            "DAY",
            "DATETIME",
            "Returns the day of the month as a number from 1 to 31.",
            [p!("date", "A date value.")]
        );
        m!(
            "HOUR",
            "DATETIME",
            "Returns the hour as a number from 0 to 23.",
            [p!("datetime", "A datetime value.")]
        );
        m!(
            "MINUTE",
            "DATETIME",
            "Returns the minute as a number from 0 to 59.",
            [p!("datetime", "A datetime value.")]
        );
        m!(
            "SECOND",
            "DATETIME",
            "Returns the seconds as a number from 0 to 59.",
            [p!("datetime", "A datetime value.")]
        );
        m!(
            "QUARTER",
            "DATETIME",
            "Returns the quarter as a number from 1 to 4.",
            [p!("date", "A date value.")]
        );
        m!(
            "WEEKDAY",
            "DATETIME",
            "Returns the day of the week as a number from 1 to 7.",
            [
                p!("date", "A date value."),
                p!(
                    "returnType",
                    "Determines the return value (default 1).",
                    opt
                )
            ]
        );
        m!(
            "WEEKNUM",
            "DATETIME",
            "Returns the week number for the given date.",
            [
                p!("date", "A date value."),
                p!(
                    "returnType",
                    "Determines which day the week begins (default 1).",
                    opt
                )
            ]
        );

        // Math
        m!(
            "DIVIDE",
            "MATH",
            "Performs division and returns an alternate result on division by zero.",
            [
                p!("numerator", "The numerator."),
                p!("denominator", "The denominator."),
                p!(
                    "alternateResult",
                    "The value to return on division by zero.",
                    opt
                )
            ]
        );
        m!(
            "ABS",
            "MATH",
            "Returns the absolute value of a number.",
            [p!("number", "The number.")]
        );
        m!(
            "ROUND",
            "MATH",
            "Rounds a number to the specified number of digits.",
            [
                p!("number", "The number to round."),
                p!("numDigits", "The number of digits.")
            ]
        );
        m!(
            "PI",
            "MATH",
            "Returns the value of Pi, 3.14159265358979.",
            []
        );
        m!(
            "SQRT",
            "MATH",
            "Returns the square root of a number.",
            [p!("number", "A positive number.")]
        );
        m!(
            "EXP",
            "MATH",
            "Returns e raised to the power of a number.",
            [p!("number", "The exponent.")]
        );
        m!(
            "LN",
            "MATH",
            "Returns the natural logarithm of a number.",
            [p!("number", "A positive number.")]
        );
        m!(
            "LOG",
            "MATH",
            "Returns the logarithm of a number to the specified base.",
            [
                p!("number", "A positive number."),
                p!("base", "The logarithm base (default 10).", opt)
            ]
        );
        m!(
            "LOG10",
            "MATH",
            "Returns the base-10 logarithm of a number.",
            [p!("number", "A positive number.")]
        );
        m!(
            "FLOOR",
            "MATH",
            "Rounds a number down to the nearest multiple of significance.",
            [
                p!("number", "The number to round."),
                p!("significance", "The multiple to round down to.")
            ]
        );
        m!(
            "CEILING",
            "MATH",
            "Rounds a number up to the nearest multiple of significance.",
            [
                p!("number", "The number to round."),
                p!("significance", "The multiple to round up to.")
            ]
        );
        m!(
            "TRUNC",
            "MATH",
            "Truncates a number to an integer by removing the decimal part.",
            [
                p!("number", "The number to truncate."),
                p!("numDigits", "The precision (default 0).", opt)
            ]
        );
        m!(
            "INT",
            "MATH",
            "Rounds a number down to the nearest integer.",
            [p!("number", "The number to round down.")]
        );
        m!(
            "SIGN",
            "MATH",
            "Returns 1, 0, or -1 based on the sign of a number.",
            [p!("number", "The number.")]
        );
        m!(
            "POWER",
            "MATH",
            "Returns a number raised to a power.",
            [
                p!("number", "The base number."),
                p!("power", "The exponent.")
            ]
        );
        m!(
            "MOD",
            "MATH",
            "Returns the remainder after dividing a number by a divisor.",
            [p!("number", "The dividend."), p!("divisor", "The divisor.")]
        );
        m!(
            "FACT",
            "MATH",
            "Returns the factorial of a number.",
            [p!("number", "A non-negative integer.")]
        );
        m!(
            "EVEN",
            "MATH",
            "Rounds a number up to the nearest even integer.",
            [p!("number", "The number to round.")]
        );
        m!(
            "ODD",
            "MATH",
            "Rounds a number up to the nearest odd integer.",
            [p!("number", "The number to round.")]
        );
        m!(
            "MROUND",
            "MATH",
            "Returns a number rounded to the desired multiple.",
            [
                p!("number", "The number to round."),
                p!("multiple", "The multiple to round to.")
            ]
        );
        m!(
            "ROUNDUP",
            "MATH",
            "Rounds a number up, away from zero.",
            [
                p!("number", "The number to round."),
                p!("numDigits", "The number of digits.")
            ]
        );
        m!(
            "ROUNDDOWN",
            "MATH",
            "Rounds a number down, toward zero.",
            [
                p!("number", "The number to round."),
                p!("numDigits", "The number of digits.")
            ]
        );
        m!(
            "GCD",
            "MATH",
            "Returns the greatest common divisor of two or more integers.",
            [
                p!("number1", "The first number."),
                p!("number2", "Additional numbers.", opt rep)
            ]
        );
        m!(
            "LCM",
            "MATH",
            "Returns the least common multiple of integers.",
            [
                p!("number1", "The first number."),
                p!("number2", "Additional numbers.", opt rep)
            ]
        );
        m!(
            "SQRTPI",
            "MATH",
            "Returns the square root of a number multiplied by pi.",
            [p!("number", "The multiplier of pi.")]
        );
        m!(
            "DEGREES",
            "MATH",
            "Converts radians to degrees.",
            [p!("angle", "The angle in radians.")]
        );
        m!(
            "RADIANS",
            "MATH",
            "Converts degrees to radians.",
            [p!("angle", "The angle in degrees.")]
        );
        m!(
            "SIN",
            "MATH",
            "Returns the sine of an angle in radians.",
            [p!("angle", "The angle in radians.")]
        );
        m!(
            "COS",
            "MATH",
            "Returns the cosine of an angle in radians.",
            [p!("angle", "The angle in radians.")]
        );
        m!(
            "TAN",
            "MATH",
            "Returns the tangent of an angle in radians.",
            [p!("angle", "The angle in radians.")]
        );
        m!(
            "ASIN",
            "MATH",
            "Returns the arcsine of a number in radians.",
            [p!("number", "A value between -1 and 1.")]
        );
        m!(
            "ACOS",
            "MATH",
            "Returns the arccosine of a number in radians.",
            [p!("number", "A value between -1 and 1.")]
        );
        m!(
            "ATAN",
            "MATH",
            "Returns the arctangent of a number in radians.",
            [p!("number", "The tangent value.")]
        );
        m!(
            "ATAN2",
            "MATH",
            "Returns the arctangent of the given x and y coordinates.",
            [
                p!("xNum", "The x coordinate."),
                p!("yNum", "The y coordinate.")
            ]
        );
        m!(
            "SINH",
            "MATH",
            "Returns the hyperbolic sine of a number.",
            [p!("number", "A real number.")]
        );
        m!(
            "COSH",
            "MATH",
            "Returns the hyperbolic cosine of a number.",
            [p!("number", "A real number.")]
        );
        m!(
            "TANH",
            "MATH",
            "Returns the hyperbolic tangent of a number.",
            [p!("number", "A real number.")]
        );
        m!(
            "ACOSH",
            "MATH",
            "Returns the inverse hyperbolic cosine of a number.",
            [p!("number", "A number >= 1.")]
        );
        m!(
            "ASINH",
            "MATH",
            "Returns the inverse hyperbolic sine of a number.",
            [p!("number", "A real number.")]
        );
        m!(
            "ATANH",
            "MATH",
            "Returns the inverse hyperbolic tangent of a number.",
            [p!("number", "A value between -1 and 1.")]
        );
        m!(
            "COT",
            "MATH",
            "Returns the cotangent of an angle in radians.",
            [p!("angle", "The angle in radians.")]
        );
        m!(
            "COTH",
            "MATH",
            "Returns the hyperbolic cotangent of a number.",
            [p!("angle", "A non-zero real number.")]
        );
        m!(
            "ACOT",
            "MATH",
            "Returns the arccotangent of a number in radians.",
            [p!("number", "A real number.")]
        );
        m!(
            "ACOTH",
            "MATH",
            "Returns the inverse hyperbolic cotangent of a number.",
            [p!("number", "A value with absolute value > 1.")]
        );

        // Logical
        m!(
            "AND",
            "LOGICAL",
            "Returns TRUE if both arguments are TRUE.",
            [
                p!("logical1", "The first logical value."),
                p!("logical2", "The second logical value.")
            ]
        );
        m!(
            "OR",
            "LOGICAL",
            "Returns TRUE if any argument is TRUE.",
            [
                p!("logical1", "The first logical value."),
                p!("logical2", "The second logical value.")
            ]
        );
        m!(
            "NOT",
            "LOGICAL",
            "Changes FALSE to TRUE or TRUE to FALSE.",
            [p!("logical", "The logical value to negate.")]
        );
        m!("TRUE", "LOGICAL", "Returns the logical value TRUE.", []);
        m!("FALSE", "LOGICAL", "Returns the logical value FALSE.", []);
        m!(
            "IF",
            "LOGICAL",
            "Returns one value when a condition is TRUE and another when FALSE.",
            [
                p!("condition", "The logical condition."),
                p!("trueValue", "The value when condition is TRUE."),
                p!("falseValue", "The value when condition is FALSE.", opt)
            ]
        );
        m!(
            "SWITCH",
            "LOGICAL",
            "Evaluates an expression against a list of values and returns a matching result.",
            [
                p!("expression", "The expression to evaluate."),
                p!("value", "A value to match."),
                p!("result", "The result when expression matches value."),
                p!("else", "The result when no match is found.", opt)
            ]
        );
        m!(
            "ISBLANK",
            "LOGICAL",
            "Returns TRUE if the value is blank.",
            [p!("value", "The value to test.")]
        );
        m!("BLANK", "LOGICAL", "Returns a blank value.", []);
        m!(
            "ERROR",
            "LOGICAL",
            "Raises an error with a custom message.",
            [p!("message", "The error message.")]
        );

        // Table manipulation
        m!(
            "VALUES",
            "TABLE",
            "Returns a one-column table of distinct values, including blanks.",
            [p!("columnOrTable", "A column or table expression.")]
        );
        m!(
            "DISTINCT",
            "TABLE",
            "Returns a one-column table of distinct values, excluding blanks.",
            [p!("columnOrTable", "A column or table expression.")]
        );
        m!(
            "EXCEPT",
            "TABLE",
            "Returns rows from the left table that do not appear in the right table.",
            [
                p!("leftTable", "The left table."),
                p!("rightTable", "The right table.")
            ]
        );
        m!(
            "INTERSECT",
            "TABLE",
            "Returns the row intersection of two tables.",
            [
                p!("leftTable", "The left table."),
                p!("rightTable", "The right table.")
            ]
        );
        m!(
            "UNION",
            "TABLE",
            "Returns a union of two or more tables.",
            [
                p!("table1", "The first table."),
                p!("table2", "Additional tables.", rep)
            ]
        );
        m!(
            "NATURALLEFTOUTERJOIN",
            "TABLE",
            "Left outer joins two tables on their common columns.",
            [
                p!("leftTable", "The left table."),
                p!("rightTable", "The right table.")
            ]
        );
        m!(
            "NATURALINNERJOIN",
            "TABLE",
            "Inner joins two tables on their common columns, keeping only rows with a match in both.",
            [
                p!("leftTable", "The left table."),
                p!("rightTable", "The right table.")
            ]
        );
        m!(
            "FILTER",
            "TABLE",
            "Returns a subset of a table that satisfies a condition.",
            [
                p!("table", "The table to filter."),
                p!(
                    "filterExpression",
                    "A boolean expression evaluated for each row."
                )
            ]
        );
        m!(
            "ALL",
            "TABLE",
            "Returns all rows in a table or column, ignoring any filters.",
            [p!("tableOrColumn", "A table or column to remove filters from.", opt rep)]
        );
        m!(
            "ALLSELECTED",
            "TABLE",
            "Returns rows visible in the outer filter context (before the nearest CALCULATE).",
            [p!("tableOrColumn", "A table or column.", opt rep)]
        );
        m!(
            "ALLEXCEPT",
            "TABLE",
            "Removes all filters from a table except those on the specified columns.",
            [
                p!("table", "The table to remove filters from."),
                p!("column", "A column to keep filters on.", rep)
            ]
        );
        m!(
            "REMOVEFILTERS",
            "TABLE",
            "Removes filters from the specified tables or columns.",
            [p!("tableOrColumn", "A table or column to clear filters from.", opt rep)]
        );
        m!(
            "KEEPFILTERS",
            "TABLE",
            "Applies a table as a filter without removing existing filters on the same columns.",
            [p!("table", "The table expression to apply as a filter.")]
        );
        m!(
            "SELECTCOLUMNS",
            "TABLE",
            "Returns a table with selected columns and new calculated columns.",
            [
                p!("table", "The source table."),
                p!("name", "The name of the new column."),
                p!("expression", "The expression for the new column.", rep)
            ]
        );
        m!(
            "TOPN",
            "TABLE",
            "Returns the top N rows of a table based on an expression.",
            [
                p!("n", "The number of rows to return."),
                p!("table", "The table."),
                p!("expression", "The expression to order by."),
                p!("order", "ASC or DESC.", opt)
            ]
        );
        m!(
            "SAMPLE",
            "TABLE",
            "Returns n rows evenly distributed across the sorted table.",
            [
                p!("n", "The number of rows to return."),
                p!("table", "The table."),
                p!("expression", "The expression to order by."),
                p!("order", "ASC or DESC.", opt)
            ]
        );
        m!(
            "TREATAS",
            "TABLE",
            "Applies a table expression as filters to columns of an unrelated table.",
            [
                p!("table", "The table expression."),
                p!("column", "The column to apply the filter to.", rep)
            ]
        );
        m!(
            "SUBSTITUTEWITHINDEX",
            "TABLE",
            "Returns a left semijoin table with an index column replacing a set of columns.",
            [
                p!("table", "The source table."),
                p!("name", "Name of the index column."),
                p!("indexTable", "The index table."),
                p!("column", "The ordering column."),
                p!("order", "ASC or DESC.")
            ]
        );
        m!(
            "RELATEDTABLE",
            "TABLE",
            "Returns all rows from the related table that match the current filter context.",
            [p!("table", "The related table.")]
        );
        m!(
            "CROSSJOIN",
            "TABLE",
            "Returns the Cartesian product of two or more tables.",
            [
                p!("table1", "The first table."),
                p!("table2", "Additional tables.", rep)
            ]
        );
        m!(
            "GENERATE",
            "TABLE",
            "Returns the cross-join of table1 with table2 evaluated per row; drops empty.",
            [
                p!("table1", "The table to iterate."),
                p!("table2", "The expression evaluated for each row.")
            ]
        );
        m!(
            "GENERATEALL",
            "TABLE",
            "Like GENERATE but keeps table1 rows even when table2 returns empty.",
            [
                p!("table1", "The table to iterate."),
                p!("table2", "The expression evaluated for each row.")
            ]
        );
        m!(
            "CURRENTGROUP",
            "TABLE",
            "Returns the rows belonging to the current group inside a GROUPBY() call.",
            []
        );
        m!(
            "GROUPBY",
            "TABLE",
            "Groups a table and evaluates expressions via CURRENTGROUP() for each group.",
            [
                p!("table", "The table to group."),
                p!("column", "A grouping column.", rep),
                p!("name", "Extension column name.", rep),
                p!(
                    "expression",
                    "Extension expression using CURRENTGROUP().",
                    rep
                )
            ]
        );
        m!(
            "SUMMARIZE",
            "TABLE",
            "Returns a summary table with totals for a set of groups.",
            [
                p!("table", "The table to summarize."),
                p!("column", "A grouping column.", rep),
                p!("name", "The name of an extension column.", opt rep)
            ]
        );
        m!(
            "SUMMARIZECOLUMNS",
            "TABLE",
            "Returns a summary table over a set of groups with optional filters.",
            [
                p!("column", "A grouping column.", opt rep),
                p!("filter", "An optional filter expression.", opt rep),
                p!("name", "The name of an extension column.", opt rep)
            ]
        );

        // Information / filter context
        m!(
            "SELECTEDVALUE",
            "INFORMATION",
            "Returns the value when a column is filtered to exactly one distinct value.",
            [
                p!("column", "The column to check."),
                p!(
                    "alternateResult",
                    "The value to return if not a single value.",
                    opt
                )
            ]
        );
        m!(
            "RELATED",
            "INFORMATION",
            "Returns a related value from another table following the active relationship.",
            [p!("column", "The related column.")]
        );
        m!("ISINSCOPE",     "INFORMATION", "Returns TRUE when the column is an active grouping axis in the current SUMMARIZECOLUMNS context.", [p!("column", "The column to test.")]);
        m!(
            "ISFILTERED",
            "INFORMATION",
            "Returns TRUE when the column has direct filters applied.",
            [p!("column", "The column to check.")]
        );
        m!(
            "CONTAINS",
            "INFORMATION",
            "Returns TRUE if the specified values exist in the given columns of the table.",
            [
                p!("table", "The table to search."),
                p!("column", "A column to search."),
                p!("value", "The value to find.", rep)
            ]
        );
        m!("LOOKUPVALUE",   "INFORMATION", "Returns the value for result_column in the row where all search_column=value pairs match.", [p!("resultColumnName", "The column whose value to return."), p!("searchColumnName", "A column to match against.", rep), p!("searchValue", "The value to search for.", rep), p!("alternateResult", "Value to return when no match is found.", opt)]);
        m!(
            "ISSUBTOTAL",
            "INFORMATION",
            "Returns TRUE for a row that contains a subtotal for the specified column.",
            [p!("column", "The column to check.")]
        );
    }

    pub fn register(&mut self, name: &'static str, f: DaxFn, rt: ReturnType) {
        self.functions
            .insert(name, FunctionEntry::CallByValue(f, rt));
    }

    pub fn register_context(&mut self, name: &'static str, f: ContextFn, rt: ReturnType) {
        self.functions.insert(name, FunctionEntry::Context(f, rt));
    }

    pub fn get(&self, name: &str) -> Option<&FunctionEntry> {
        self.functions.get(name)
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
