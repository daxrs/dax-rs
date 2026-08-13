//! A from-scratch MDX translator, built independently of `crate::mdx`.
//!
//! `crate::mdx` remains the production parser/translator and is untouched by
//! this module. `mdxtranslator` starts from a general MDX grammar (see
//! `mdx.pest`) rather than a fixed set of recognized query shapes.

pub mod ast;
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
