// SPDX-License-Identifier: CC0-1.0

//! What the query compiler reads a `/` and a `|` AS, probed over the compiler itself
//! (`query_compile::compile`) rather than over the wire.
//!
//! RULE: a character the query language gives two meanings is read as the one the command it
//! stands in gives it. A `/` opens a regular-expression literal only where a command writes a
//! pattern — the second operand of `parse`, the right operand of a match operator in
//! `filter` — and is the division operator everywhere else; a `|` separates the stages of the
//! pipeline, and `||` is the or operator of an expression. So arithmetic is not read as a
//! pattern, and a disjunction is not read as a stage.
//!
//! The rows span what that rule generalises over: which command carries the characters, how
//! many of them it carries, whether they stand inside brackets, and whether a real pattern
//! stands beside them. The wire-level counterpart — that such a query is ACCEPTED by
//! `StartQuery` — is in [`crate::query_language_admission`].

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::query_compile::compile;

/// Every query string whose `/` or `|` the language means as an operator compiles.
#[test]
fn an_operator_is_not_read_as_a_pattern_or_a_stage() {
    // (case, the query string)
    let compiles: [(&str, &str); 8] = [
        (
            "one division in a filter",
            "fields @message | filter duration/1000 > 1",
        ),
        (
            "two divisions in a filter",
            "fields @message | filter duration/1000 > other/1000",
        ),
        (
            "divisions inside brackets",
            "fields @message | filter (duration/1000) > (other/1000)",
        ),
        (
            "a division beside a pattern match",
            r"filter @message like /ERROR/ and duration/1000 > 1",
        ),
        (
            "a division in a projection",
            "fields duration/1000 as seconds | sort seconds desc",
        ),
        (
            "a division in an aggregation",
            "stats sum(duration)/1000 as seconds by svc",
        ),
        (
            "the or operator written as two pipes",
            "fields @message | filter duration > 1 || duration < 0",
        ),
        (
            "two disjunctions and a stage separator",
            "fields @message | filter a > 1 || b > 2 || c > 3 | limit 5",
        ),
    ];
    for (case, query) in compiles {
        let compiled = compile(query);
        assert!(
            compiled.is_ok(),
            "case {case}: the query {query:?} compiles, got {compiled:?}"
        );
    }

    // A pattern that IS a pattern is still judged as one, and a `|` that opens a stage still
    // separates stages: the rule narrows what the delimiters mean, it removes nothing.
    let refused: [(&str, &str); 2] = [
        (
            "a pattern a match operator carries that is no expression",
            r"fields @message | filter @message like /+/",
        ),
        (
            "a stage separator followed by a word that is no command",
            "fields @message | frobnicate @message",
        ),
    ];
    for (case, query) in refused {
        let failure = compile(query);
        assert!(
            failure.is_err(),
            "case {case}: the query {query:?} does not compile, got {failure:?}"
        );
    }
}
