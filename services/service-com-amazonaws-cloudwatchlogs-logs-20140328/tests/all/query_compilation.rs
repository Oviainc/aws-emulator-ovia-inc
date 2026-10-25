// SPDX-License-Identifier: CC0-1.0

//! The Logs Insights QL query strings the compiler must ACCEPT: the counterpart of the
//! refusals in [`crate::insights_queries`], asserted against the compiler itself so one row
//! per pipeline form costs nothing.
//!
//! RULE: a query string the query language defines compiles — so `StartQuery` accepts it —
//! and the only thing that refuses one is a fault in the string itself. The rows span what
//! the rule generalises over: which commands the pipeline is built from, whether a `|`
//! appears inside a regular expression or a quoted string, which form a `parse` pattern takes
//! (regular expression or glob), whether a stage aliases fields with `as`, and whether a
//! metric is aggregated, sorted or limited.

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::query_compile::compile;

/// Every query string here compiles, and the compiled pipeline names one command per stage.
#[test]
fn every_query_the_language_defines_compiles() {    // Each row is one realistic query of the language, differing along the commands it uses
    // and the literals they carry.
    let queries: Vec<(&str, usize)> = vec![
        ("fields @timestamp, @message | sort @timestamp desc | limit 10", 3),
        ("fields @timestamp, @message | limit 1", 2),
        ("stats count(*) as events by bin(1m)", 1),
        ("fields @message | filter @message like /ERROR|WARN/", 2),
        (
            r"fields @message | parse @message /(?<level>[A-Z]+)/ | stats count(*) by level",
            3,
        ),
        ("filter @message like 'ERROR' | stats count(*) as n", 2),
        ("fields @timestamp, @message | filter ispresent(@message) | limit 5", 3),
        (r#"parse @message "* *" as a, b | display a, b"#, 2),
        (
            "fields @timestamp, @message, @logStream, @log | sort @timestamp desc | limit 20",
            3,
        ),
        ("stats count() by bin(5m)", 1),
        ("fields @timestamp, @message | filter @message =~ /x/", 2),
        (
            r#"SOURCE logGroups(namePrefix:["/aws/"]) | fields @message | limit 1"#,
            3,
        ),
        (
            r"fields @timestamp, @message | parse @message /(?<a>\S+)\s+(?<b>\S+)/ | display a, b",
            3,
        ),
        (
            "fields @timestamp, @message | stats count(*) as c by @logStream | sort c desc | limit 5",
            4,
        ),
        ("fields @timestamp, @message | filter level = \"ERROR\" | limit 1", 3),
        ("fields @message | unmask @message | limit 1", 3),
        ("fields @message | dedup @message | limit 1", 3),
        ("fields @message | pattern @message | limit 1", 3),
        (
            r"parse @message /(?<srcaddr>[\d.]+)/ | stats sum(bytes) as totalBytes by srcaddr",
            2,
        ),
        // A quoted string carrying a word that looks like a command is part of the string.
        (r#"fields @message | filter @message = "limit 5" | limit 1"#, 3),
        // A division inside an expression is not the start of a regular expression.
        ("fields @message | stats sum(bytes) / count(*) as mean", 2),
    ];
    for (query, stages) in queries {
        match compile(query) {
            Ok(compiled) => assert_eq!(
                compiled.commands.len(),
                stages,
                "case {query:?}: expected {stages} pipeline stages, got {:?}",
                compiled.commands
            ),
            Err(failure) => panic!("case {query:?}: the query must compile, got {failure:?}"),
        }
    }
}

/// The compiler's only input is the query string, bounded by the model's own
/// `QueryString` `@length(0..10000)`: at every extreme of that bound, and for a string
/// truncated mid-literal, it answers — it neither accepts a query that is not one nor runs
/// away on one.
///
/// The rows differ along which extreme they sit at: the empty string, a string of nothing but
/// stage separators at the modelled maximum length, a maximum-length string of one command,
/// and strings truncated inside each kind of literal.
#[test]
fn the_compiler_answers_at_every_extreme_of_the_modelled_query_length() {
    /// The modelled maximum of `QueryString`
    /// (`smithy_query(query="operation",operation="StartQuery")` -> `queryString` len
    /// 0..10000).
    const MAX_QUERY_CHARS: usize = 10_000;

    // (case, the query string, whether it must compile)
    let longest_pipeline = "|".repeat(MAX_QUERY_CHARS);
    let longest_filter = format!(
        "filter @message like \"{}\"",
        "a".repeat(MAX_QUERY_CHARS - "filter @message like \"\"".len())
    );
    let cases: Vec<(&str, &str, bool)> = vec![
        ("the empty query string", "", false),
        (
            "the modelled maximum length of stage separators only",
            &longest_pipeline,
            false,
        ),
        (
            "the modelled maximum length of one command",
            &longest_filter,
            true,
        ),
        ("a string truncated inside a quoted string", "filter @message like \"abc", true),
        (
            "a string truncated inside a parse expression",
            r"parse @message /(?<level>[A-Z]+",
            false,
        ),
        ("a string of one blank character", " ", false),
    ];
    for (case, query, must_compile) in cases {
        assert!(
            query.chars().count() <= MAX_QUERY_CHARS,
            "case {case}: the row must stay inside the modelled maximum, got {} characters",
            query.chars().count()
        );
        let compiled = compile(query);
        assert_eq!(
            compiled.is_ok(),
            must_compile,
            "case {case}: expected the query to compile: {must_compile}, got {compiled:?}"
        );
    }
}
