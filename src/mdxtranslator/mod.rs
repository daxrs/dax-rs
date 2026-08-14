//! A from-scratch MDX translator, built independently of `crate::mdx`.
//!
//! `crate::mdx` remains the production parser/translator and is untouched by
//! this module. `mdxtranslator` starts from a general MDX grammar (see
//! `mdx.pest`) rather than a fixed set of recognized query shapes.

pub mod ast;
pub mod daxgen;
pub mod eval;

use pest::iterators::Pairs;
use pest::Parser;
use pest_derive::Parser as PestParser;

#[derive(PestParser)]
#[grammar = "mdxtranslator/mdx.pest"]
pub struct MdxParser;

/// Parses a raw MDX statement against the general grammar, returning the
/// pest parse tree. No AST is built yet — this only validates that the
/// statement is syntactically valid MDX.
pub fn parse_mdx(input: &str) -> Result<Pairs<'_, Rule>, Box<pest::error::Error<Rule>>> {
    MdxParser::parse(Rule::mdx_query, input).map_err(Box::new)
}

/// One MDX axis's evaluated shape, kept separate from every other axis —
/// the un-flattened counterpart to what `translate()` folds into one
/// combined tuple space for DAX generation. A response serializer that needs
/// to know "which hierarchies came from ROWS vs COLUMNS" (e.g. to lay out
/// Tabular rowset columns or a Multidimensional cellset's per-axis member
/// lists) needs this; `daxgen` itself does not.
pub struct AxisTranslation {
    pub id: ast::AxisId,
    pub dim_props: Vec<String>,
    pub set: eval::EvaluatedSet,
}

pub struct TranslatedQuery {
    pub axes: Vec<AxisTranslation>,
    pub slicer: Vec<eval::Member>,
    pub dax: String,
}

/// Parses `mdx`, evaluates every axis and the WHERE slicer, and generates the
/// DAX query text for the result — same as `translate()`, but also returns
/// each axis's evaluated shape individually (before `combine_sets` folds them
/// together for `daxgen`), plus the DIMENSION PROPERTIES requested per axis.
pub fn translate_with_axes(
    mdx: &str,
    engine: &crate::engine::Engine,
) -> Result<TranslatedQuery, String> {
    let query = ast::parse(mdx).map_err(|e| e.to_string())?;
    let ctx = eval::EvalCtx::from_query(engine, &query).map_err(|e| e.to_string())?;
    let cube = match query.body {
        ast::QueryBody::Cube(cube) => cube,
        ast::QueryBody::System(_) => {
            return Err(
                "$system discovery queries are not supported by the DAX translator".to_string(),
            )
        }
    };

    let mut axes = Vec::with_capacity(cube.axes.len());
    let mut combined: Option<eval::EvaluatedSet> = None;
    for axis in &cube.axes {
        let set = eval::eval_set(&axis.expr, &ctx)?;
        combined = Some(match combined {
            None => set.clone(),
            Some(acc) => eval::combine_sets(acc, set.clone()),
        });
        axes.push(AxisTranslation { id: axis.id.clone(), dim_props: axis.dim_props.clone(), set });
    }

    let slicer = match &cube.where_clause {
        Some(w) => eval::eval_slicer(w, &ctx)?,
        None => Vec::new(),
    };

    let dax = daxgen::generate_dax(combined.as_ref(), &slicer, &ctx)?;

    Ok(TranslatedQuery { axes, slicer, dax })
}

/// Parses `mdx`, evaluates every axis and the WHERE slicer, and generates the
/// DAX query text for the result. Multiple axes are folded into one combined
/// tuple space (the same shape-concat + Cartesian product `CrossJoin`
/// performs) before generation, since `daxgen` only cares about the final
/// combined shape, not which MDX axis each hierarchy came from.
pub fn translate(mdx: &str, engine: &crate::engine::Engine) -> Result<String, String> {
    translate_with_axes(mdx, engine).map(|t| t.dax)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_rule(pairs: Pairs<Rule>, target: Rule) -> bool {
        for pair in pairs {
            if pair.as_rule() == target {
                return true;
            }
            if contains_rule(pair.into_inner(), target) {
                return true;
            }
        }
        false
    }

    #[test]
    fn not_is_distinguishable_from_absence_of_not() {
        let with_not = parse_mdx(
            "WITH MEMBER [Measures].[X] AS NOT [Measures].[Y] = 0 SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        )
        .expect("should parse");
        assert!(
            contains_rule(with_not, Rule::negated_expr),
            "expected NOT to surface as Rule::negated_expr"
        );

        let without_not = parse_mdx(
            "WITH MEMBER [Measures].[X] AS [Measures].[Y] = 0 SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        )
        .expect("should parse");
        assert!(
            !contains_rule(without_not, Rule::negated_expr),
            "did not expect Rule::negated_expr without NOT"
        );
    }

    #[test]
    fn is_not_is_distinguishable_from_is() {
        let is_not_null = parse_mdx(
            "WITH MEMBER [Measures].[X] AS [Measures].[Y] IS NOT NULL SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        )
        .expect("should parse");
        assert!(
            contains_rule(is_not_null, Rule::negated_is_tail),
            "expected IS NOT to surface as Rule::negated_is_tail"
        );

        let is_null = parse_mdx(
            "WITH MEMBER [Measures].[X] AS [Measures].[Y] IS NULL SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        )
        .expect("should parse");
        assert!(
            !contains_rule(is_null, Rule::negated_is_tail),
            "did not expect Rule::negated_is_tail without NOT"
        );

        let is_not_member = parse_mdx(
            "WITH MEMBER [Measures].[X] AS [Measures].[Y].CurrentMember IS NOT [Measures].[Z].CurrentMember SELECT {[Measures].[X]} ON COLUMNS FROM [Model]",
        )
        .expect("should parse");
        assert!(
            contains_rule(is_not_member, Rule::negated_is_tail),
            "expected IS NOT <member> to surface as Rule::negated_is_tail"
        );
    }
}
