use std::collections::{HashMap, HashSet};

use crate::engine::context::ExecutionContext;
use crate::engine::dax::parser::parse_expression;
use crate::engine::error::{DaxError, DaxResult};
use crate::engine::functions::REGISTRY;
use crate::engine::ir::binder::bind;
use crate::engine::ir::builder::build_expression;
use crate::engine::ir::expr_node::{BoundExprNode, BoundExprNode::*};
use crate::engine::ir::operator::{
    infer_binary_dtype, BoundBinaryOp, BoundCalculate, BoundFunction, BoundSummarize,
    BoundSummarizeColumns, BoundUnaryOp, BoundVar, UnaryOperator,
};
use polars::prelude::DataType;

pub fn resolve(ctx: &ExecutionContext) -> DaxResult<HashMap<String, BoundExprNode>> {
    let mut shallow: HashMap<String, BoundExprNode> = HashMap::new();
    for (name, expr_str) in &ctx.catalog.measures {
        let ast = parse_expression(expr_str)
            .map_err(|e| DaxError::Parse(format!("measure '{name}': {e}")))?;
        let ir = build_expression(ast)
            .map_err(|e| DaxError::Eval(format!("build error in measure '{name}': {e}")))?;
        let bound = bind(ir, ctx)
            .map_err(|e| DaxError::Eval(format!("bind error in measure '{name}': {e}")))?;
        shallow.insert(name.clone(), bound);
    }

    // Phase 1b — build adjacency list by walking each shallow-bound tree.
    // Bare [Name] references that aren't real catalog measures are virtual
    // row-context columns (SUMMARIZE extension columns, dimension columns pulled
    // in via a relationship join, ROLLUPADDISSUBTOTAL flags, etc.) — they're
    // resolved dynamically at eval time via row context, not by this pass, so
    // they're excluded from the dependency graph entirely.
    let known_measures: HashSet<String> = ctx.catalog.measures.keys().cloned().collect();
    let mut deps: HashMap<String, Vec<String>> = known_measures
        .iter()
        .map(|n| (n.clone(), Vec::new()))
        .collect();

    for (name, tree) in &shallow {
        collect_deps(
            tree,
            &known_measures,
            deps.get_mut(name)
                .expect("name was inserted into deps from the same key set"),
        );
    }

    // Phase 2 — DFS topological sort; detect back-edges (cycles).
    let order = topo_sort(&deps)?;

    // Phase 3 — inline stubs in dependency-first order, then annotate types.
    let mut resolved: HashMap<String, BoundExprNode> = HashMap::new();
    for name in &order {
        let inlined = inline_measures(
            shallow
                .remove(name)
                .expect("topo sort only produces names present in shallow"),
            &resolved,
        );
        let annotated = annotate(inlined);
        resolved.insert(name.clone(), annotated);
    }

    Ok(resolved)
}

fn collect_deps(node: &BoundExprNode, known: &HashSet<String>, deps: &mut Vec<String>) {
    match node {
        Measure(m) => {
            if known.contains(&m.name) {
                deps.push(m.name.clone());
            }
        }
        BinaryOp(op) => {
            collect_deps(&op.left, known, deps);
            collect_deps(&op.right, known, deps);
        }
        Function(f) => f.args.iter().for_each(|a| collect_deps(a, known, deps)),
        Calculate(c) => {
            collect_deps(&c.expression, known, deps);
            c.filters.iter().for_each(|f| collect_deps(f, known, deps));
        }
        UnaryOp(op) => collect_deps(&op.expr, known, deps),
        BoundExprNode::Var(v) => {
            v.bindings
                .iter()
                .for_each(|(_, e)| collect_deps(e, known, deps));
            collect_deps(&v.result, known, deps);
        }
        // VarRef names a local VAR binding, never a catalog measure.
        _ => {}
    }
}

#[derive(PartialEq)]
enum VisitState {
    Unvisited,
    InProgress,
    Done,
}

fn topo_sort(deps: &HashMap<String, Vec<String>>) -> DaxResult<Vec<String>> {
    let mut state: HashMap<String, VisitState> = deps
        .keys()
        .map(|n| (n.clone(), VisitState::Unvisited))
        .collect();
    let mut order = Vec::new();

    for name in deps.keys() {
        if state[name] == VisitState::Unvisited {
            dfs(name, deps, &mut state, &mut order, &mut Vec::new())?;
        }
    }

    Ok(order)
}

fn dfs(
    name: &str,
    deps: &HashMap<String, Vec<String>>,
    state: &mut HashMap<String, VisitState>,
    order: &mut Vec<String>,
    path: &mut Vec<String>,
) -> DaxResult<()> {
    state.insert(name.to_string(), VisitState::InProgress);
    path.push(name.to_string());

    for dep in deps.get(name).map(Vec::as_slice).unwrap_or(&[]) {
        match state.get(dep.as_str()) {
            Some(VisitState::InProgress) => {
                let cycle_start = path
                    .iter()
                    .position(|n| n == dep)
                    .expect("dep is InProgress so it must be in path");
                let cycle = path[cycle_start..].join(" → ");
                return Err(DaxError::Eval(format!(
                    "Circular measure dependency: {cycle} → {dep}"
                )));
            }
            Some(VisitState::Unvisited) => {
                dfs(dep, deps, state, order, path)?;
            }
            _ => {}
        }
    }

    path.pop();
    state.insert(name.to_string(), VisitState::Done);
    order.push(name.to_string());
    Ok(())
}

fn inline_measures(
    node: BoundExprNode,
    resolved: &HashMap<String, BoundExprNode>,
) -> BoundExprNode {
    match node {
        // `collect_deps` only tracks names that are real catalog measures, so
        // topo sort guarantees any such name is present in `resolved` here.
        // A miss means `m.name` was never a real measure to begin with — it's a
        // bare [Name] reference to a virtual row-context column (SUMMARIZE
        // extension column, joined dimension column, etc.), left as a stub for
        // the evaluator's row-context fallback to resolve dynamically.
        Measure(m) => resolved.get(&m.name).cloned().unwrap_or(Measure(m)),

        BinaryOp(op) => BoundExprNode::BinaryOp(BoundBinaryOp {
            left: Box::new(inline_measures(*op.left, resolved)),
            right: Box::new(inline_measures(*op.right, resolved)),
            op: op.op,
            dtype: None, // filled in by annotate
        }),

        Function(f) => BoundExprNode::Function(BoundFunction {
            name: f.name,
            args: f
                .args
                .into_iter()
                .map(|a| inline_measures(a, resolved))
                .collect(),
            dtype: None,
        }),

        Calculate(c) => BoundExprNode::Calculate(BoundCalculate {
            expression: Box::new(inline_measures(*c.expression, resolved)),
            filters: c
                .filters
                .into_iter()
                .map(|f| inline_measures(f, resolved))
                .collect(),
            dtype: None,
        }),

        UnaryOp(op) => BoundExprNode::UnaryOp(BoundUnaryOp {
            op: op.op,
            expr: Box::new(inline_measures(*op.expr, resolved)),
            dtype: None,
        }),

        BoundExprNode::TableConstructor(rows) => BoundExprNode::TableConstructor(
            rows.into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|e| inline_measures(e, resolved))
                        .collect()
                })
                .collect(),
        ),

        BoundExprNode::Var(v) => BoundExprNode::Var(BoundVar {
            bindings: v
                .bindings
                .into_iter()
                .map(|(n, e)| (n, inline_measures(e, resolved)))
                .collect(),
            result: Box::new(inline_measures(*v.result, resolved)),
        }),

        leaf => leaf,
    }
}

fn annotate(node: BoundExprNode) -> BoundExprNode {
    match node {
        BoundExprNode::Literal(_) | BoundExprNode::Column(_) | BoundExprNode::Table(_) => node,

        BoundExprNode::BinaryOp(op) => {
            let left = Box::new(annotate(*op.left));
            let right = Box::new(annotate(*op.right));
            // A child may be an unresolved Measure stub (a virtual row-context
            // column only known at eval time). Its dtype can't be inferred
            // statically here, so the whole node's dtype stays unresolved too.
            let dtype = match (left.dtype(), right.dtype()) {
                (Some(l), Some(r)) => Some(infer_binary_dtype(&op.op, &l, &r)),
                _ => None,
            };
            BoundExprNode::BinaryOp(BoundBinaryOp { left, right, op: op.op, dtype })
        }

        BoundExprNode::Function(f) => {
            let args: Vec<BoundExprNode> = f.args.into_iter().map(annotate).collect();
            let arg_dtypes: Vec<Option<DataType>> = args.iter().map(|a| a.dtype()).collect();
            let dtype = REGISTRY
                .get(&f.name)
                .unwrap_or_else(|| {
                    panic!(
                        "annotate: unknown function '{}' — should have been caught by binder",
                        f.name
                    )
                })
                .return_type()
                .to_dtype(&arg_dtypes);
            BoundExprNode::Function(BoundFunction { name: f.name, args, dtype })
        }

        BoundExprNode::Calculate(c) => {
            let expression = Box::new(annotate(*c.expression));
            let filters = c.filters.into_iter().map(annotate).collect();
            let dtype = expression.dtype();
            BoundExprNode::Calculate(BoundCalculate { expression, filters, dtype })
        }

        BoundExprNode::UnaryOp(op) => {
            let expr = Box::new(annotate(*op.expr));
            let dtype = match &op.op {
                UnaryOperator::Not => Some(DataType::Boolean),
                // Unresolved Measure stub operand: dtype unknown until eval time.
                UnaryOperator::Negate => expr.dtype(),
            };
            BoundExprNode::UnaryOp(BoundUnaryOp { op: op.op, expr, dtype })
        }

        // Unresolved Measure stub: not a real catalog measure, so it's a bare
        // [Name] reference to a virtual row-context column. Left as-is; the
        // evaluator resolves it dynamically via row context at eval time.
        BoundExprNode::Measure(_) => node,

        BoundExprNode::Summarize(s) => {
            let table = Box::new(annotate(*s.table));
            let group_by = s.group_by.into_iter().map(annotate).collect();
            let rollup_cols = s
                .rollup_cols
                .into_iter()
                .map(|(e, flag)| (annotate(e), flag))
                .collect();
            let extensions = s
                .extensions
                .into_iter()
                .map(|(n, e)| (n, annotate(e)))
                .collect();
            BoundExprNode::Summarize(BoundSummarize { table, group_by, rollup_cols, extensions })
        }

        BoundExprNode::TableConstructor(rows) => BoundExprNode::TableConstructor(
            rows.into_iter()
                .map(|row| row.into_iter().map(annotate).collect())
                .collect(),
        ),

        BoundExprNode::SummarizeColumns(sc) => {
            let group_by_cols = sc.group_by_cols.into_iter().map(annotate).collect();
            let rollup_groups = sc
                .rollup_groups
                .into_iter()
                .map(|axis| {
                    axis.into_iter()
                        .map(|(cols, flag)| (cols.into_iter().map(annotate).collect(), flag))
                        .collect()
                })
                .collect();
            let filters = sc.filters.into_iter().map(annotate).collect();
            let extensions = sc
                .extensions
                .into_iter()
                .map(|(n, e, ig)| (n, annotate(e), ig))
                .collect();
            BoundExprNode::SummarizeColumns(BoundSummarizeColumns {
                group_by_cols,
                rollup_groups,
                filters,
                extensions,
            })
        }

        BoundExprNode::Var(v) => {
            let bindings = v
                .bindings
                .into_iter()
                .map(|(n, e)| (n, annotate(e)))
                .collect();
            let result = Box::new(annotate(*v.result));
            BoundExprNode::Var(BoundVar { bindings, result })
        }

        // Resolved dynamically via row context at eval time — dtype unknown here.
        BoundExprNode::VarRef(_) => node,
    }
}
