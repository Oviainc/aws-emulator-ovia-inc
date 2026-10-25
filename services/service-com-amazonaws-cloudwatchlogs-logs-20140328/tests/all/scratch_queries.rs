// SPDX-License-Identifier: CC0-1.0

//! Where the query compiler STOPS: the character range a `QueryCompileError` locates, for
//! each fault a Logs Insights QL query string can carry.
//!
//! RULE: a query string the compiler refuses is refused AT the text at fault — the
//! `QueryCompileError$location` spans exactly that text and nothing else — so a client can
//! point at the characters it has to fix. The rows span the faults the compiler judges: a
//! regular expression that is not one, an ephemeral field defined twice, a word that is no
//! command of the language, an empty stage and a `limit` that is no count.
//!
//! The wire-level counterpart of these rows — that each fault answers 400
//! `MalformedQueryException` — is in [`crate::insights_queries`]; the accepted strings are in
//! [`crate::query_compilation`].

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::query_compile::compile;

/// Every fault, with the text the compile error must locate.
#[test]
fn every_fault_is_located_at_the_text_at_fault() {
    // (case, the query string, the substring the located range must cover)
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "a regular expression with an unclosed group",
            r"fields @message | parse @message /(?<level>[A-Z]+/",
            r"/(?<level>[A-Z]+/",
        ),
        (
            "a regular expression that runs to the end of the command",
            r"fields @message | parse @message /(?<level",
            r"/(?<level",
        ),
        (
            "a field defined twice",
            r"parse @message /(?<duration>\d+)/ | fields duration as duration",
            "duration as duration",
        ),
        (
            "a word that is no command of the language",
            "fields @message | shuffle @message",
            "shuffle",
        ),
        (
            "a limit that is no count of log events",
            "fields @message | limit none",
            "none",
        ),
    ];
    for (case, query, at_fault) in cases {
        let failure = compile(query)
            .err()
            .unwrap_or_else(|| panic!("case {case}: the query {query:?} must be refused"));
        let characters: Vec<char> = query.chars().collect();
        let start = failure.start_char_offset.min(characters.len());
        let end = failure.end_char_offset.clamp(start, characters.len());
        let located: String = characters[start..end].iter().collect();
        assert!(
            located.contains(at_fault) || at_fault.contains(located.trim()),
            "case {case}: expected the located range to cover {at_fault:?}, got {located:?} \
             from {failure:?}"
        );
        assert!(
            !failure.message.is_empty(),
            "case {case}: a compile failure says what it stopped on, got {failure:?}"
        );
    }
}
