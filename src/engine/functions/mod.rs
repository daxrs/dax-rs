pub mod aggregation;
pub mod datetime;
pub mod logical;
pub mod math;
pub mod registry;
pub use registry::{FunctionEntry, FunctionRegistry, ReturnType};

use once_cell::sync::Lazy;
pub static REGISTRY: Lazy<FunctionRegistry> = Lazy::new(FunctionRegistry::new);
