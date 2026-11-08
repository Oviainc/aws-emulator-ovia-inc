// SPDX-License-Identifier: CC0-1.0

//! The functions a CloudWatch Logs Insights QL query may CALL, and finding a call to one the
//! query language does not define.
//!
//! A call to a name the language defines no function for is a query that cannot be compiled —
//! there is nothing to compute — so it is a `MalformedQueryException` at `StartQuery` rather
//! than a query that runs and produces nothing. The fault is SEMANTIC: `stats bogusfn(ms) as
//! z` parses as a `stats` command over one call, and only the function's name is wrong.
//!
//! [`NAMED_FUNCTIONS`] is the closed catalogue, read off the operations-and-functions list of
//! the CloudWatch Logs Insights query syntax `StartQuery` refers to ("For more information,
//! see CloudWatch Logs Insights Query Syntax"), which groups them as the comments below do.
//! A function the language adds later is refused here until it is added to that one list —
//! the same deliberate trade the command vocabulary ([`super::COMMANDS`]) makes.
//!
//! Beside the catalogue, every function this emulator can actually EVALUATE is admitted
//! (`insights::expression`), so the compiler never refuses a call the execution would have
//! computed; the unit test at the foot of this module pins that.
//!
//! WHAT BOUNDS IT: one left-to-right pass over a command's arguments, which are part of a
//! query string the model constrains to `@length(0..10000)`.

use super::QueryCompileFailure;
use crate::state::insights::expression::{
    AggregateFunction, GROUPING_FUNCTIONS, SCALAR_FUNCTIONS,
};

/// The functions the query language defines, lowercased, as the query-syntax documentation
/// lists them:
///
/// * aggregation functions a `stats` command computes over a group — with the spellings that
///   documentation gives as synonyms (`avg`/`average`/`mean`, `count_distinct`/`dc`);
/// * the non-aggregation `stats` functions, which pick a value out of a group;
/// * arithmetic functions;
/// * string functions;
/// * datetime functions, including the `bin` a `by` clause buckets time with;
/// * IP-address functions;
/// * the general functions `ispresent`, `coalesce` and `unmask`.
const NAMED_FUNCTIONS: [&str; 52] = [
    // Aggregation.
    "avg",
    "average",
    "count",
    "count_distinct",
    "distinct_count",
    "dc",
    "max",
    "mean",
    "min",
    "pct",
    "percentile",
    "stddev",
    "sum",
    // Non-aggregation `stats` functions.
    "earliest",
    "latest",
    "sortsfirst",
    "sortslast",
    // Arithmetic.
    "abs",
    "ceil",
    "floor",
    "greatest",
    "least",
    "log",
    "sqrt",
    // Strings.
    "concat",
    "isblank",
    "isempty",
    "ltrim",
    "lower",
    "replace",
    "rtrim",
    "strcontains",
    "strlen",
    "substr",
    "tolower",
    "toupper",
    "trim",
    "upper",
    // Datetime.
    "bin",
    "dateceil",
    "datefloor",
    "frommillis",
    "tomillis",
    // IP addresses.
    "isipinsubnet",
    "isipv4insubnet",
    "isipv6insubnet",
    "isvalidip",
    "isvalidipv4",
    "isvalidipv6",
    // General.
    "coalesce",
    "ispresent",
    "unmask",
];

/// Whether `name` names a function of the query language — one of [`NAMED_FUNCTIONS`], or one
/// this emulator evaluates, which it must admit whatever that catalogue says.
fn is_named_function(name: &str) -> bool {
    let lowercased = name.to_lowercase();
    NAMED_FUNCTIONS.contains(&lowercased.as_str())
        || SCALAR_FUNCTIONS.contains(&lowercased.as_str())
        || GROUPING_FUNCTIONS.contains(&lowercased.as_str())
        || AggregateFunction::parse(&lowercased).is_some()
}

/// The first call in `arguments` to a function the query language does not define, as the
/// failure the query is refused with — `None` for a stage that calls only functions it
/// defines, which is every stage that calls none.
///
/// `command` says which command's arguments these are, because that decides what text inside
/// them is a LITERAL rather than something to read as a call: the pattern a `parse` or
/// `filter` command matches with is the caller's own text and names no function
/// ([`masked_literals`]).
pub(super) fn unknown_function_call(
    command: &str,
    arguments: &str,
    arguments_offset: usize,
) -> Option<QueryCompileFailure> {
    let characters: Vec<char> = masked_literals(command, arguments).chars().collect();
    let mut index = 0usize;
    while index < characters.len() {
        if !is_name_start(characters[index]) {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < characters.len() && is_name_character(characters[end]) {
            end += 1;
        }
        // A name is a function CALL only where the arguments it is given open right after it,
        // which is how every one of the language's functions is written.
        if characters.get(end) == Some(&'(') {
            let name: String = characters[index..end].iter().collect();
            if !is_named_function(&name) {
                let (start_char_offset, end_char_offset) =
                    (arguments_offset + index, arguments_offset + end);
                return Some(QueryCompileFailure {
                    message: format!("The function {name} is not a valid query function."),
                    // The sentence states the rule the query broke and where in the query
                    // string the call stands: the name parses fine, so a diagnostic naming a
                    // symbol the compiler stopped on would misreport a query that is wrong
                    // about what the language defines.
                    refusal: Some(format!(
                        "{UNKNOWN_FUNCTION_PREFIX}{name} ([{start_char_offset},{end_char_offset}])"
                    )),
                    start_char_offset,
                    end_char_offset,
                });
            }
        }
        index = end.max(index + 1);
    }
    None
}

/// The fixed text such a refusal opens with, the name the call used following it.
const UNKNOWN_FUNCTION_PREFIX: &str = "Unknown function: ";

/// `arguments` with every LITERAL in it blanked out, so a scan over the result reads only
/// text the query means as an expression.
///
/// Two kinds of literal carry text of the caller's own: a quoted string — the glob a `parse`
/// command extracts with, a value a `filter` compares against — and the regular expression a
/// `parse` or `filter` command matches with, which is read where that command's own check
/// reads it ([`super::regex_literal`]). Blanking keeps every offset, so a failure found after
/// one still points at the query's own characters.
fn masked_literals(command: &str, arguments: &str) -> String {
    let mut masked: Vec<char> = arguments.chars().collect();
    let mut index = 0usize;
    while index < masked.len() {
        let quote = masked[index];
        if quote != '\'' && quote != '"' {
            index += 1;
            continue;
        }
        masked[index] = ' ';
        let mut at = index + 1;
        while at < masked.len() {
            let inside = masked[at];
            masked[at] = ' ';
            at += 1;
            // A quote the string escapes is part of it rather than its end.
            if inside == '\\' {
                if at < masked.len() {
                    masked[at] = ' ';
                    at += 1;
                }
                continue;
            }
            if inside == quote {
                break;
            }
        }
        index = at;
    }
    if command == super::PARSE || command == super::FILTER {
        let text: String = masked.iter().collect();
        for literal in super::command_regex_literals(command, &text, 0) {
            let slashes = if literal.closed { 2 } else { 1 };
            let end = literal
                .offset
                .saturating_add(literal.pattern.chars().count() + slashes)
                .min(masked.len());
            for blanked in masked.iter_mut().take(end).skip(literal.offset) {
                *blanked = ' ';
            }
        }
    }
    masked.iter().collect()
}

/// Whether `character` can open the name of a function.
fn is_name_start(character: char) -> bool {
    character.is_alphabetic() || character == '_'
}

/// Whether `character` can stand inside one.
fn is_name_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

#[cfg(test)]
mod tests {
    use super::{is_named_function, masked_literals, unknown_function_call, NAMED_FUNCTIONS};
    use crate::state::insights::expression::{GROUPING_FUNCTIONS, SCALAR_FUNCTIONS};

    /// Every function this emulator EVALUATES compiles: the compiler may not refuse a call the
    /// execution would have computed, whatever the catalogue lists.
    #[test]
    fn every_function_the_execution_evaluates_compiles() {
        let evaluated = SCALAR_FUNCTIONS
            .iter()
            .chain(GROUPING_FUNCTIONS.iter())
            // The aggregate spellings `AggregateFunction::parse` reads.
            .chain(
                [
                    "count",
                    "count_distinct",
                    "distinct_count",
                    "dc",
                    "sum",
                    "avg",
                    "average",
                    "mean",
                    "min",
                    "max",
                ]
                .iter(),
            );
        for name in evaluated {
            assert!(
                is_named_function(name),
                "case {name:?}: a function the execution evaluates must compile"
            );
        }
        for name in NAMED_FUNCTIONS {
            assert_eq!(
                name,
                name.to_lowercase(),
                "case {name:?}: the catalogue is lowercased, so a call is matched case-insensitively"
            );
        }
    }

    /// A call is read out of the expression text alone: a name inside a quoted glob or inside
    /// the regular expression a command matches with is the caller's own text.
    #[test]
    fn a_name_inside_a_literal_is_not_a_call() {
        // (case, command, arguments, whether a call to something unknown is found)
        let cases: [(&str, &str, &str, bool); 6] = [
            ("a glob naming no function", "parse", r#" @message "frobnicate(*)" as x"#, false),
            (
                "a regular expression with a group",
                "parse",
                r" @message /frobnicate(?<x>\d+)/",
                false,
            ),
            (
                "a regular expression in a filter",
                "filter",
                " @message like /frobnicate(a|b)/",
                false,
            ),
            ("a known call beside a literal", "filter", r#" svc = "a(b)" and ispresent(lat)"#, false),
            ("an unknown call", "stats", " bogusfn(ms) as z", true),
            ("a known call", "stats", " count(*) as n by bin(1m)", false),
        ];
        for (case, command, arguments, expected) in cases {
            let found = unknown_function_call(command, arguments, 0);
            assert_eq!(
                found.is_some(),
                expected,
                "case {case:?}: expected unknown-call {expected}, got {found:?}"
            );
        }
        // Blanking keeps the length, so every offset a later failure reports still points at
        // the query's own characters.
        let arguments = r#" @message "svc=*" as svc"#;
        let masked = masked_literals("parse", arguments);
        assert_eq!(
            masked.chars().count(),
            arguments.chars().count(),
            "masking a literal keeps the arguments' length, got {masked:?}"
        );
    }
}
