// SPDX-License-Identifier: CC0-1.0

//! What a `FilterLogEvents` search REPORTS about itself, and which requests it refuses,
//! end to end over the wire.
//!
//! One RULE per table below:
//!
//! * Every successful search carries the `searchStatistics` of the log streams it reports as
//!   searched: `approximateTotalLogStreamCount` is how many streams the search was spread
//!   over and `completedLogStreamCount` how many of those were searched to completion, which
//!   summarises the `searchedLogStreams` reporting the response carries beside it
//!   ("Indicates which log streams have been searched and whether each has been searched
//!   completely", which "As of May 15, 2020 [...] is no longer supported. This parameter
//!   returns an empty list.").
//! * A page that CARRIED events stopped at the last one it reported and is resumable from a
//!   `nextToken`; a page that carried none walked its scan to the end, where "pagination is
//!   finished" ("A returned page might only be partially full, or even empty. [...] Partially
//!   full or empty pages don't necessarily mean that pagination is finished. If the results
//!   include a nextToken, there might be more log events available").
//! * A term of a `filterPattern` is a bare word of the syntax's word characters, a `"…"`
//!   string or a `%…%` regular expression, and a condition's bare word additionally carries a
//!   number's sign and point and the `*` wildcard: a term carrying any other character is
//!   refused with `InvalidParameterException` naming that term, wherever the service RUNS a
//!   pattern.
//! * A descending scan (`startFromHead` false) is answered over whatever time range it names.
//! * `logStreamNames` NAMES log streams of the group, so a name the group does not hold is
//!   refused with `ResourceNotFoundException`, while a `logStreamNamePrefix` is a filter and
//!   one no stream carries selects nothing.
//!
//! The rows of each table differ along what its rule generalises over: which streams the
//! selection covers, how many events a page carries and how it is bounded, WHICH character
//! and which form of pattern a refused term carries, where the scan's range begins, and
//! whether a stream selector names or filters.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, Logs};

/// The log group the cases search.
const GROUP: &str = "/aws/emulator/search-reporting";
/// A second group, created and left empty, so a search can cover no stream at all.
const EMPTY_GROUP: &str = "/aws/emulator/search-reporting-empty";

/// The group's streams: two share a prefix the third does not.
const APP_ONE: &str = "app-one";
const APP_TWO: &str = "app-two";
const SYS_ONE: &str = "sys-one";

/// One stored event: its stream, its offset from the suite's base instant and its message.
struct Stored {
    stream: &'static str,
    offset: i64,
    message: &'static str,
}

/// The events every case searches over, in the chronological order they are stored in.
const STORED: [Stored; 5] = [
    Stored {
        stream: APP_ONE,
        offset: 0,
        message: "ERROR disk full",
    },
    Stored {
        stream: APP_TWO,
        offset: 10,
        message: "WARN disk filling",
    },
    Stored {
        stream: APP_ONE,
        offset: 20,
        message: "INFO disk fine",
    },
    Stored {
        stream: SYS_ONE,
        offset: 30,
        message: "ERROR kernel panic",
    },
    Stored {
        stream: APP_TWO,
        offset: 40,
        message: "ERROR 404 while handling orders",
    },
];

/// Both groups, the three streams and the events above.
async fn given_stored_events(logs: &Logs, at: i64) {
    for group in [GROUP, EMPTY_GROUP] {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    for stream in [APP_ONE, APP_TWO, SYS_ONE] {
        logs.ok(
            "CreateLogStream",
            &json!({"logGroupName": GROUP, "logStreamName": stream}),
        )
        .await;
        let events: Vec<(i64, &str)> = STORED
            .iter()
            .filter(|stored| stored.stream == stream)
            .map(|stored| (at + stored.offset, stored.message))
            .collect();
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": stream,
                "logEvents": batch(&events),
            }),
        )
        .await;
    }
}

/// The messages one answer reported, in the order it reported them.
fn reported(case: &str, answer: &Value) -> Vec<String> {
    answer["events"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: an answer carries events, got {answer}"))
        .iter()
        .map(|event| {
            event["message"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: an event carries a message, got {event}"))
                .to_string()
        })
        .collect()
}

/// The messages the stored events at `offsets` carry, in order.
fn messages_at(offsets: &[i64]) -> Vec<String> {
    offsets
        .iter()
        .map(|offset| {
            STORED
                .iter()
                .find(|stored| stored.offset == *offset)
                .unwrap_or_else(|| panic!("the tables only name stored offsets, not {offset}"))
                .message
                .to_string()
        })
        .collect()
}

/// Hold one answer to the search-reporting rule: the two `searchStatistics` counts are the
/// counts of the `searchedLogStreams` the same answer reports.
fn assert_reports_its_search(case: &str, answer: &Value) {
    let searched = answer["searchedLogStreams"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case}: an answer reports searchedLogStreams, got {answer}")
        })
        .clone();
    let statistics = answer
        .get("searchStatistics")
        .unwrap_or_else(|| panic!("case {case}: an answer reports searchStatistics, got {answer}"));
    let total = statistics["approximateTotalLogStreamCount"].as_i64();
    let completed = statistics["completedLogStreamCount"].as_i64();
    let searched_completely = searched
        .iter()
        .filter(|stream| stream["searchedCompletely"].as_bool() == Some(true))
        .count();
    assert_eq!(
        total,
        Some(i64::try_from(searched.len()).expect("a small count")),
        "case {case}: approximateTotalLogStreamCount counts the streams the search reports as \
         searched, got {answer}"
    );
    assert_eq!(
        completed,
        Some(i64::try_from(searched_completely).expect("a small count")),
        "case {case}: completedLogStreamCount counts the searched streams reported complete, \
         got {answer}"
    );
    assert!(
        completed.is_some_and(|completed| total.is_some_and(|total| completed <= total)),
        "case {case}: no more streams were searched to completion than were searched at all, \
         got {answer}"
    );
    // `searchedLogStreams` is the withdrawn reporting these counts count ("As of May 15,
    // 2020, this parameter is no longer supported. This parameter returns an empty list"), so
    // the observable answer is the empty list and the two zeroes beside it — whatever streams
    // the row's own selection covers.
    assert!(
        searched.is_empty(),
        "case {case}: searchedLogStreams is no longer supported and reports an empty list, \
         got {answer}"
    );
    assert_eq!(
        (total, completed),
        (Some(0), Some(0)),
        "case {case}: a search that reports no searched stream reports no count of one, got \
         {answer}"
    );
    let members: Vec<&str> = statistics
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: searchStatistics is an object, got {answer}"))
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        members,
        ["approximateTotalLogStreamCount", "completedLogStreamCount"],
        "case {case}: searchStatistics carries exactly the two counts, got {answer}"
    );
    for member in ["approximateTotalLogStreamCount", "completedLogStreamCount"] {
        assert!(
            statistics[member].is_i64(),
            "case {case}: {member} is a whole number on the wire, got {answer}"
        );
    }
}

/// One row of the search-reporting table: the request members added to a log group.
struct Reporting {
    case: &'static str,
    body: fn(i64) -> Value,
}

/// The search-reporting table: the rows differ in which streams the selection covers (every
/// one, one named, a prefixed pair, none at all, a group with no streams), in whether the
/// pattern and the time range admit anything, and in whether the page is bounded.
const REPORTING: [Reporting; 7] = [
    Reporting {
        case: "no selector, so every stream of the group",
        body: |_| json!({"logGroupName": GROUP}),
    },
    Reporting {
        case: "one named log stream",
        body: |_| json!({"logGroupName": GROUP, "logStreamNames": [APP_ONE]}),
    },
    Reporting {
        case: "a prefix two of the three streams carry",
        body: |_| json!({"logGroupName": GROUP, "logStreamNamePrefix": "app-"}),
    },
    Reporting {
        case: "a prefix no stream carries",
        body: |_| json!({"logGroupName": GROUP, "logStreamNamePrefix": "nothing-"}),
    },
    Reporting {
        case: "a group holding no log stream at all",
        body: |_| json!({"logGroupName": EMPTY_GROUP}),
    },
    Reporting {
        case: "a pattern no stored event carries",
        body: |_| json!({"logGroupName": GROUP, "filterPattern": "NOTHING_MATCHES_THIS"}),
    },
    Reporting {
        case: "a page bounded by a limit, over a range bounded at both ends",
        body: |at| {
            json!({
                "logGroupName": GROUP,
                "limit": 2,
                "startTime": at,
                "endTime": at + 30,
            })
        },
    },
];

/// Every successful search reports the streams it searched and the counts of that reporting.
#[tokio::test]
async fn every_successful_search_reports_the_statistics_of_its_own_stream_reporting() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &REPORTING {
        let answer = logs.ok("FilterLogEvents", &(row.body)(at)).await;
        assert_reports_its_search(row.case, &answer);
    }
}

/// One row of the pagination table.
struct Walk {
    case: &'static str,
    body: fn(i64) -> Value,
    /// The offsets of the stored events the walk must cover, in the order it covers them.
    covered: &'static [i64],
}

/// The pagination table: the rows differ in how many events a page carries, how the page is
/// bounded, which direction the scan runs and whether the scan admits anything at all.
const WALKS: [Walk; 6] = [
    Walk {
        case: "an unbounded page over every stored event",
        body: |_| json!({"logGroupName": GROUP}),
        covered: &[0, 10, 20, 30, 40],
    },
    Walk {
        case: "pages of at most one event",
        body: |_| json!({"logGroupName": GROUP, "limit": 1}),
        covered: &[0, 10, 20, 30, 40],
    },
    Walk {
        case: "pages of at most two events",
        body: |_| json!({"logGroupName": GROUP, "limit": 2}),
        covered: &[0, 10, 20, 30, 40],
    },
    Walk {
        case: "a descending scan in pages of at most two events",
        body: |_| json!({"logGroupName": GROUP, "limit": 2, "startFromHead": false}),
        covered: &[40, 30, 20, 10, 0],
    },
    Walk {
        case: "a pattern no stored event carries, so the first page is already empty",
        body: |_| json!({"logGroupName": GROUP, "filterPattern": "NOTHING_MATCHES_THIS"}),
        covered: &[],
    },
    Walk {
        case: "a range that admits two of the five",
        body: |at| json!({"logGroupName": GROUP, "startTime": at + 10, "endTime": at + 20}),
        covered: &[10, 20],
    },
];

/// A page that carried events is resumable and a page that carried none finishes pagination,
/// so the walk covers every matched event exactly once and then stops.
#[tokio::test]
async fn a_page_with_events_is_resumable_and_an_empty_page_finishes_pagination() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &WALKS {
        let case = row.case;
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut body = (row.body)(at);
            if let Some(token) = &token {
                body["nextToken"] = json!(token);
            }
            let answer = logs.ok("FilterLogEvents", &body).await;
            let page = reported(case, &answer);
            let resumable = answer.get("nextToken").and_then(Value::as_str).is_some();
            assert_eq!(
                resumable,
                !page.is_empty(),
                "case {case}: page {pages} carried {} event(s), so resumable must be {}, got \
                 {answer}",
                page.len(),
                !page.is_empty()
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= STORED.len() + 1,
                "case {case}: the walk terminates, got {pages} pages after {walked:?}"
            );
            match answer["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked,
            messages_at(row.covered),
            "case {case}: the pages cover every matched event exactly once, in the scan's order"
        );
    }
}

/// One row of the term table: a `filterPattern` and the refusal message it earns, or `None`
/// where the syntax admits it.
struct Term {
    case: &'static str,
    pattern: &'static str,
    refusal: Option<&'static str>,
}

/// The term table. The refused rows differ in WHICH character the term carries (a quote, a
/// colon, a slash, a two-character run) and in which form of pattern carries it (an
/// unstructured term, a JSON condition, a space-delimited condition); the admitted rows
/// cover every form a term may take — a word with an underscore, a `"…"` string carrying
/// spaces, a `%…%` expression, the `?` and `-` prefixes, a number, a dotted word, a `*`
/// wildcard and the `...` of a field list.
const TERMS: [Term; 19] = [
    Term {
        case: "a pattern that is nothing but a quote",
        pattern: "\"",
        refusal: Some("Invalid character(s) in term '\"'"),
    },
    Term {
        case: "a quote at the end of the pattern, closing nothing",
        pattern: "skips \"",
        refusal: Some("Invalid character(s) in term '\"'"),
    },
    Term {
        case: "a quote whose string's content does not follow it",
        pattern: "\" skips \"",
        refusal: Some("Invalid character(s) in term '\"'"),
    },
    Term {
        case: "a quoted string that never closes",
        pattern: "\"ConditionalCheck",
        refusal: Some("Invalid character(s) in term '\"ConditionalCheck'"),
    },
    Term {
        case: "a colon at the end of a bare term",
        pattern: "ConditionalCheckFailedException:",
        refusal: Some("Invalid character(s) in term ':'"),
    },
    Term {
        case: "a slash opening a path-like term",
        pattern: "GET /health",
        refusal: Some("Invalid character(s) in term '/'"),
    },
    Term {
        case: "a run of two characters no term admits",
        pattern: "handler->skips",
        refusal: Some("Invalid character(s) in term '->'"),
    },
    Term {
        case: "a semicolon between two words",
        pattern: "skips;retries",
        refusal: Some("Invalid character(s) in term ';'"),
    },
    Term {
        case: "a slash in an unquoted JSON condition value",
        pattern: "{ $.requestPath = /health }",
        refusal: Some("Invalid character(s) in term '/'"),
    },
    Term {
        case: "a colon in an unquoted JSON condition value",
        pattern: "{ $.principal = arn:aws:iam }",
        refusal: Some("Invalid character(s) in term ':'"),
    },
    Term {
        case: "a slash in an unquoted space-delimited condition value",
        pattern: "[method, path = /health, status]",
        refusal: Some("Invalid character(s) in term '/'"),
    },
    Term {
        case: "a bare word of alphanumerics and an underscore",
        pattern: "NOTHING_MATCHES_THIS",
        refusal: None,
    },
    Term {
        case: "a quoted string carrying spaces",
        pattern: "\"Internal Server Error\"",
        refusal: None,
    },
    Term {
        case: "a regular expression literal",
        pattern: "%ERR[0-9]+%",
        refusal: None,
    },
    Term {
        case: "the alternative and exclusion prefixes",
        pattern: "?ERROR ?WARN -Exception",
        refusal: None,
    },
    Term {
        case: "a number in a JSON condition",
        pattern: "{ $.code = 404 }",
        refusal: None,
    },
    Term {
        case: "a dotted word in a JSON condition",
        pattern: "{ $.version = v1.2 }",
        refusal: None,
    },
    Term {
        case: "a wildcard literal in a space-delimited condition",
        pattern: "[ip, status_code = 4*, bytes]",
        refusal: None,
    },
    Term {
        case: "the field-list wildcard of a space-delimited pattern",
        pattern: "[level = ERROR, ...]",
        refusal: None,
    },
];

/// A term the pattern syntax does not admit is refused, in the same words and with the same
/// error, by every operation that RUNS a filter pattern; a term it admits is run.
#[tokio::test]
async fn a_term_the_pattern_syntax_does_not_admit_is_refused_wherever_a_pattern_runs() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &TERMS {
        let case = row.case;
        let searches = json!({"logGroupName": GROUP, "filterPattern": row.pattern});
        let tests = json!({"filterPattern": row.pattern, "logEventMessages": ["ERROR disk full"]});
        match row.refusal {
            Some(expected) => {
                for (operation, body) in [("FilterLogEvents", &searches), ("TestMetricFilter", &tests)] {
                    let (code, message) = logs.refusal(operation, body).await;
                    assert_eq!(
                        code, "InvalidParameterException",
                        "case {case} on {operation}: expected InvalidParameterException, got \
                         {code} with {message:?}"
                    );
                    assert_eq!(
                        message, expected,
                        "case {case} on {operation}: expected {expected:?}, got {message:?}"
                    );
                }
            }
            None => {
                let answer = logs.ok("FilterLogEvents", &searches).await;
                assert!(
                    answer["events"].is_array(),
                    "case {case}: an admitted pattern is run and answers events, got {answer}"
                );
                let tested = logs.ok("TestMetricFilter", &tests).await;
                assert!(
                    tested["matches"].is_array(),
                    "case {case}: an admitted pattern is run over the sample, got {tested}"
                );
            }
        }
    }
}

/// One row of the descending-scan table: where the scan's range begins.
struct Descending {
    case: &'static str,
    /// The `startTime` the request names, as an absolute instant, or `None` for a request
    /// that names none.
    start_time: Option<i64>,
    /// The offsets the scan must report, newest first.
    reported: &'static [i64],
}

/// The descending-scan table. `FilterLogEventsRequest$startFromHead` documents a restriction
/// to a `startTime` "on or after Jan 1, 2024 00:00:00 UTC", which the service does not hold
/// the request to: every row is answered. They differ in where the range begins — at the
/// epoch, well before that instant, exactly at it, and nowhere at all.
const DESCENDING: [Descending; 4] = [
    Descending {
        case: "a startTime at the Unix epoch",
        start_time: Some(0),
        reported: &[40, 30, 20, 10, 0],
    },
    Descending {
        case: "a startTime in 2023, before the documented instant",
        start_time: Some(1_700_000_000_000),
        reported: &[40, 30, 20, 10, 0],
    },
    Descending {
        // Jan 1, 2024 00:00:00 UTC in the milliseconds every timestamp member carries.
        case: "a startTime exactly at the documented instant",
        start_time: Some(1_704_067_200_000),
        reported: &[40, 30, 20, 10, 0],
    },
    Descending {
        case: "no startTime at all",
        start_time: None,
        reported: &[40, 30, 20, 10, 0],
    },
];

/// A descending scan is answered over whatever time range it names, newest first.
#[tokio::test]
async fn a_descending_scan_is_answered_whatever_range_it_names() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &DESCENDING {
        let case = row.case;
        let mut body = json!({"logGroupName": GROUP, "startFromHead": false});
        if let Some(start) = row.start_time {
            body["startTime"] = json!(start);
        }
        let answer = logs.ok("FilterLogEvents", &body).await;
        assert_eq!(
            reported(case, &answer),
            messages_at(row.reported),
            "case {case}: a descending scan reports the admitted events newest first, got \
             {answer}"
        );
        assert_reports_its_search(case, &answer);
    }
}

/// One row of the stream-selector table.
struct Selector {
    case: &'static str,
    body: fn() -> Value,
    /// The offsets the request must report, or the `(code, message)` it must be refused
    /// with.
    answer: Result<&'static [i64], (&'static str, &'static str)>,
}

/// The sentence the service refuses a request naming both log-stream selectors with: "If you
/// specify a value for both logStreamNames and logStreamNamePrefix, the action returns an
/// InvalidParameterException error", worded as the call it cannot make.
const BOTH_SELECTORS: (&str, &str) = (
    "InvalidParameterException",
    "Cannot call FilterLogEvents with both logStreamNames and logStreamNamePrefix",
);

/// The sentence a request naming a log stream the group does not hold is refused with.
const NO_SUCH_STREAM: (&str, &str) = (
    "ResourceNotFoundException",
    "The specified log stream does not exist.",
);

/// The stream-selector table: the rows differ in whether the selector NAMES streams or
/// FILTERS by prefix, in whether what it names or filters for is in the group, and in
/// whether the request names both selectors at once.
const SELECTORS: [Selector; 6] = [
    Selector {
        case: "a named log stream the group holds",
        body: || json!({"logGroupName": GROUP, "logStreamNames": [APP_ONE]}),
        answer: Ok(&[0, 20]),
    },
    Selector {
        case: "a named log stream the group does not hold",
        body: || json!({"logGroupName": GROUP, "logStreamNames": ["absent-stream"]}),
        answer: Err(NO_SUCH_STREAM),
    },
    Selector {
        case: "an absent name beside a name the group holds",
        body: || json!({"logGroupName": GROUP, "logStreamNames": [APP_ONE, "absent-stream"]}),
        answer: Err(NO_SUCH_STREAM),
    },
    Selector {
        case: "a prefix two of the three streams carry",
        body: || json!({"logGroupName": GROUP, "logStreamNamePrefix": "app-"}),
        answer: Ok(&[0, 10, 20, 40]),
    },
    Selector {
        case: "a prefix no stream of the group carries",
        body: || json!({"logGroupName": GROUP, "logStreamNamePrefix": "absent-"}),
        answer: Ok(&[]),
    },
    Selector {
        case: "both log-stream selectors at once",
        body: || {
            json!({
                "logGroupName": GROUP,
                "logStreamNames": [APP_ONE],
                "logStreamNamePrefix": "app-",
            })
        },
        answer: Err(BOTH_SELECTORS),
    },
];

/// A named log stream the group does not hold is refused as a missing resource and both
/// selectors at once in the service's own words, while a prefix nothing carries selects
/// nothing.
#[tokio::test]
async fn a_named_log_stream_the_group_does_not_hold_is_refused() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;

    for row in &SELECTORS {
        let case = row.case;
        let body = (row.body)();
        match row.answer {
            Ok(offsets) => {
                let answer = logs.ok("FilterLogEvents", &body).await;
                assert_eq!(
                    reported(case, &answer),
                    messages_at(offsets),
                    "case {case}: expected the selected events, got {answer}"
                );
            }
            Err((expected_code, expected_message)) => {
                let (code, message) = logs.refusal("FilterLogEvents", &body).await;
                assert_eq!(
                    code, expected_code,
                    "case {case}: expected {expected_code}, got {code} with {message:?}"
                );
                assert_eq!(
                    message, expected_message,
                    "case {case}: expected {expected_message:?}, got {message:?}"
                );
            }
        }
    }
}

/// A `filterPattern` is held to its bracket structure where a filter is CONFIGURED and to the
/// whole syntax where one is RUN, so a pattern carrying a term only the running side refuses
/// is stored, echoed back and left matching nothing — it never fails an ingestion, and it is
/// the search naming the same pattern that is refused.
#[tokio::test]
async fn a_configured_pattern_the_running_side_refuses_is_stored_and_matches_nothing() {
    let logs = Logs::new();
    let at = base_ms();
    given_stored_events(&logs, at).await;
    let case = "a stored metric filter whose pattern carries a path-like term";
    // A term the run-time rule refuses ([`is_term_character`] admits no `/`), whose BRACKET
    // structure is sound, which is what a configuration write checks.
    let pattern = "GET /health";

    logs.ok(
        "PutMetricFilter",
        &json!({
            "logGroupName": GROUP,
            "filterName": "path-term",
            "filterPattern": pattern,
            "metricTransformations": [{
                "metricName": "Requests",
                "metricNamespace": "Emulator/Logs",
                "metricValue": "1",
            }],
        }),
    )
    .await;
    let described = logs
        .ok(
            "DescribeMetricFilters",
            &json!({"logGroupName": GROUP, "filterNamePrefix": "path-term"}),
        )
        .await;
    assert_eq!(
        described["metricFilters"][0]["filterPattern"].as_str(),
        Some(pattern),
        "case {case}: the configured pattern is stored as written, got {described}"
    );

    // Ingestion is not refused for a stored filter's pattern.
    let ingested = logs
        .ok(
            "PutLogEvents",
            &json!({
                "logGroupName": GROUP,
                "logStreamName": APP_ONE,
                "logEvents": batch(&[(at + 50, "GET /health 200")]),
            }),
        )
        .await;
    assert!(
        ingested.get("rejectedLogEventsInfo").is_none(),
        "case {case}: an ingested batch is not rejected by a stored filter's pattern, got \
         {ingested}"
    );

    // The same pattern named by a SEARCH is refused, which is the side that runs it.
    let (code, message) = logs
        .refusal(
            "FilterLogEvents",
            &json!({"logGroupName": GROUP, "filterPattern": pattern}),
        )
        .await;
    assert_eq!(
        (code.as_str(), message.as_str()),
        (
            "InvalidParameterException",
            "Invalid character(s) in term '/'"
        ),
        "case {case}: the search naming the same pattern is refused, got {code} with {message:?}"
    );
}
