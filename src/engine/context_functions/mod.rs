mod filter;
mod iterator;
mod relationship;
mod scalar;
mod summarize;
mod table;

pub use filter::*;
pub use iterator::*;
pub use relationship::*;
pub use scalar::*;
pub use summarize::*;
pub use table::*;

use polars::prelude::DataFrame;

use crate::engine::error::{DaxError, DaxResult};

pub(crate) fn select_unique(
    df: &DataFrame,
    col_names: &[String],
    fn_name: &str,
) -> DaxResult<DataFrame> {
    df.select(col_names)
        .map_err(|e| DaxError::Eval(format!("{fn_name}: select failed: {e}")))?
        .unique_stable(
            Some(col_names),
            polars::prelude::UniqueKeepStrategy::First,
            None,
        )
        .map_err(|e| DaxError::Eval(format!("{fn_name}: unique_stable failed: {e}")))
}
