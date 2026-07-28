pub mod context;
pub mod context_functions;
pub mod dax;
pub mod error;
pub mod evaluator;
pub mod expressions;
pub mod functions;
pub mod ir;
pub mod measure_resolver;
mod order_by;
pub mod row_context;
pub mod table_col;

use crate::engine::context::{ExecutionContext, FilterContext};
use crate::engine::dax::ast::Definition;
use crate::engine::dax::parser::{parse_expression, parse_query};
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::evaluator::Evaluator;
use crate::engine::expressions::Value;
use crate::engine::ir::binder::{bind, bind_with_vars};
use crate::engine::ir::builder::build_expression;
use crate::engine::row_context::RowContext;
use crate::loaders::tmsl::{load_tmsl, load_tmsl_from_op};
use chrono_tz;
use opendal::blocking::Operator as BlockingOperator;
use std::sync::Arc;

pub struct Engine {
    ctx: ExecutionContext,
}

impl Engine {
    pub fn from_tmsl_file(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let catalog = load_tmsl(path)?;
        static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> =
            std::sync::LazyLock::new(|| {
                tokio::runtime::Runtime::new()
                    .expect("failed to create runtime for blocking opendal operator")
            });
        let datasets_op = Arc::new({
            let _guard = RUNTIME.enter();
            BlockingOperator::new(
                opendal::Operator::new(opendal::services::Fs::default().root("."))?.finish(),
            )?
        });
        let mut ctx = ExecutionContext::try_new(catalog, datasets_op)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        ctx.resolved_measures = measure_resolver::resolve(&ctx)?;
        Ok(Self { ctx })
    }

    pub fn from_storage(
        catalogs_op: &BlockingOperator,
        tmsl_path: &str,
        datasets_op: Arc<BlockingOperator>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let catalog = load_tmsl_from_op(catalogs_op, tmsl_path)?;
        let mut ctx = ExecutionContext::try_new(catalog, datasets_op)
            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        ctx.resolved_measures = measure_resolver::resolve(&ctx)?;
        Ok(Self { ctx })
    }

    pub fn ctx(&self) -> &ExecutionContext {
        &self.ctx
    }
    pub fn ctx_mut(&mut self) -> &mut ExecutionContext {
        &mut self.ctx
    }

    pub fn timezone(&self) -> Option<&str> {
        self.ctx.timezone.as_deref()
    }

    /// Set the timezone used by `NOW()` and `TODAY()`.
    ///
    /// Pass `None` to use the system local timezone.
    /// Pass `Some("Europe/Copenhagen")` (or any IANA name) to pin a specific zone.
    /// Returns an error if the name is not a valid IANA timezone.
    pub fn set_timezone(&mut self, tz: Option<&str>) -> Result<(), String> {
        match tz {
            None => {
                self.ctx.timezone = None;
                Ok(())
            }
            Some(name) => {
                name.parse::<chrono_tz::Tz>()
                    .map_err(|_| format!("Unknown IANA timezone: '{name}'"))?;
                self.ctx.timezone = Some(name.to_string());
                Ok(())
            }
        }
    }

    pub fn reload_data(&mut self) -> Result<(), String> {
        self.ctx.reload_tables()
    }

    /// Trigger a trivial Polars computation to force Rayon's global thread pool
    /// to initialise before the first client query arrives.
    pub fn warmup(&self) {
        use polars::prelude::IntoLazy;
        if let Some(df) = self.ctx.tables.values().next() {
            let _ = df.clone().lazy().limit(0).collect();
        }
    }

    pub fn evaluate_query(&self, query_str: &str) -> DaxResult<Vec<Value>> {
        let query = parse_query(query_str)?;

        let mut var_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut base_rc = RowContext::new();
        for def in query.define {
            let (name, expr) = match def {
                Definition::Var { name, expr } => (name, expr),
                Definition::Measure { name, expr, .. } => (name, expr),
            };
            let bound =
                build_expression(*expr).and_then(|ir| bind_with_vars(ir, &self.ctx, &var_names))?;
            let value = Evaluator::eval(bound, &self.ctx, &FilterContext::new(), &base_rc)?;
            base_rc = base_rc.with_var(name.clone(), value);
            var_names.insert(name);
        }

        if query.statements.is_empty() {
            return Err(DaxError::InvalidArgument(
                "Empty query: no EVALUATE statement".into(),
            ));
        }

        let mut results = Vec::with_capacity(query.statements.len());
        for stmt in query.statements {
            let bound_ir = build_expression(*stmt.expr)
                .and_then(|ir| bind_with_vars(ir, &self.ctx, &var_names))?;
            let value = Evaluator::eval(bound_ir, &self.ctx, &FilterContext::new(), &base_rc)?;

            let value = if stmt.order_by.is_empty() {
                value
            } else {
                order_by::apply_order_by(value, stmt.order_by, stmt.start_at)?
            };
            results.push(value);
        }
        Ok(results)
    }

    pub fn evaluate(&self, query: &str) -> DaxResult<Value> {
        let ast = parse_expression(query)?;

        let bound_ir = build_expression(ast).and_then(|ir| bind(ir, &self.ctx))?;

        Evaluator::eval(
            bound_ir,
            &self.ctx,
            &FilterContext::new(),
            &RowContext::new(),
        )
    }
}
