// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what `StartQuery` ADMITS, per query language: which requests it
//! accepts and mints a `queryId` for, and which it refuses before any query exists.
//!
//! The rules under test:
//!
//! * a Logs Insights QL pipeline whose commands and functions the language defines COMPILES,
//!   however many stages it carries — listing a field an earlier stage produced selects that
//!   field rather than defining it a second time;
//! * a query that CALLS a function the query language does not define does not compile, and
//!   is refused `MalformedQueryException`;
//! * the log groups a query reads are named in exactly ONE place — the request's members for
//!   the pipelined languages, the statement's own `FROM` clause for the SQL language — so a
//!   SQL request naming them in neither, or in both, is refused `InvalidParameterException`;
//! * every log group one OpenSearch-language query reads is of one log class, so a query
//!   spanning two classes is refused `InvalidParameterException`;
//! * a SQL `HAVING` clause filters on the columns the statement selects, so one filtering on
//!   an aggregate function expression does not compile.
//!
//! Every case drives the service the way a client does, so a refusal a handler does not
//! actually compute cannot pass.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, now_ms, Logs};

/// The operations under test.
const START: &str = "StartQuery";

/// The modelled error codes the rules name.
const MALFORMED: &str = "MalformedQueryException";
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// A log group of the standard class and the window a query of it is started with, in the
/// epoch SECONDS `StartQuery` documents for its range.
async fn group_with_window(logs: &Logs, group: &str, class: Option<&str>) -> (i64, i64) {
    let mut body = json!({"logGroupName": group});
    if let Some(class) = class {
        body["logGroupClass"] = json!(class);
    }
    logs.ok("CreateLogGroup", &body).await;
    let now_secs = now_ms() / 1_000 + 1;
    (now_secs - 60 * 60, now_secs)
}

/// The `queryId` a successful `StartQuery` answered.
fn query_id(answer: &Value) -> String {
    answer["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {answer}"))
        .to_string()
}

/// Every pipeline the query language defines is admitted, whatever stages it carries.
///
/// The rows span what the rule generalises over: how many stages the pipeline carries, which
/// commands they are, whether a later stage LISTS a field an earlier one produced, and
/// whether a stage computes a value with arithmetic or with one of the language's functions.
#[tokio::test]
async fn a_pipeline_the_query_language_defines_is_admitted() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/pipelines";
    let (start_time, end_time) = group_with_window(&logs, GROUP, None).await;

    // (case, the query string)
    let cases: [(&str, &str); 16] = [
        ("one stage", "fields @timestamp, @message"),
        (
            "a parsed field listed by a later fields command",
            r"parse @message /lat=(?<lat>\d+)/ | fields @timestamp, lat",
        ),
        (
            "a parsed field displayed by a later display command",
            r#"fields @message | parse @message "svc=* lat=*" as svc, lat | display svc, lat"#,
        ),
        (
            "sort, limit, dedup and display over parsed fields",
            r#"fields @timestamp, @message
| parse @message "svc=* lat=* st=*" as svc, lat, st
| filter st = "ok"
| sort lat desc
| limit 5
| dedup svc
| display svc, lat"#,
        ),
        (
            "arithmetic over a parsed field, under a new name",
            r#"parse @message "lat=*" as lat | fields lat / 1000 as seconds | sort seconds desc"#,
        ),
        (
            // One `as` clause of a LIST names one field; the name after the comma is the next
            // item of the list, which the stage reads rather than defines.
            "a renamed field beside a field an earlier stage produced",
            r#"parse @message "svc=* lat=*" as svc, lat | fields lat / 1000 as sec, svc
| sort sec desc | limit 3 | dedup svc | display svc, sec"#,
        ),
        (
            "a renaming display stage beside an earlier field",
            r#"parse @message "svc=* lat=*" as svc, lat | display lat * 2 as doubled, svc
| sort doubled desc | limit 1"#,
        ),
        (
            // A `/` is the division operator wherever the language is not matching against a
            // pattern, including inside brackets.
            "divisions on both sides of a comparison",
            "fields @message | filter (duration/1000) > (other/1000) | limit 1",
        ),
        (
            "a division beside a pattern match",
            r"fields @message | filter @message like /ERROR/ and duration/1000 > 1",
        ),
        (
            "the or operator written as two pipes",
            "fields @message | filter duration > 1 || duration < 0 | limit 1",
        ),
        (
            "an aggregate by a field",
            "stats count(*) as events by bin(1m)",
        ),
        (
            "several aggregates, one of them over an expression",
            "fields @message | stats count(*) as n, avg(lat) as mean, max(lat) as worst by svc",
        ),
        (
            "a scalar function of the language",
            "fields @message | filter ispresent(lat) | stats sum(lat) as total by svc",
        ),
        (
            "a percentile aggregate",
            "fields @message | stats pct(lat, 95) as p95 by svc",
        ),
        (
            "a stage listing a reserved field twice over",
            "fields @timestamp, @message | sort @timestamp asc | limit 1",
        ),
        (
            "a glob whose text reads like a call",
            r#"fields @message | parse @message "frobnicate(*)" as called | limit 1"#,
        ),
    ];
    for (case, query_string) in cases {
        let (status, body) = logs
            .call(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": query_string,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a query the language defines, got {status} \
             with body {body}"
        );
        assert!(
            !query_id(&body).is_empty(),
            "case {case:?}: an accepted query is minted an id, got {body}"
        );
    }
}

/// A query calling a function the query language does not define does not compile.
#[tokio::test]
async fn a_call_to_a_function_the_language_does_not_define_is_malformed() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/functions";
    let (start_time, end_time) = group_with_window(&logs, GROUP, None).await;

    // (case, the query string, the name the compile error must blame)
    let cases: [(&str, &str, &str); 4] = [
        ("an unknown aggregate", "fields ms | stats bogusfn(ms) as z", "bogusfn"),
        (
            "a misspelled aggregate",
            "fields @message | stats avgg(lat) as mean by svc",
            "avgg",
        ),
        (
            "an unknown function in a filter",
            "fields @message | filter frobnicate(lat) > 1",
            "frobnicate",
        ),
        (
            "an unknown function computing a field",
            "fields upperrr(@message) as shouted",
            "upperrr",
        ),
    ];
    for (case, query_string, blamed) in cases {
        let (code, message) = logs
            .refusal(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": query_string,
                }),
            )
            .await;
        assert_eq!(
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with message {message:?}"
        );
        assert!(
            message.contains(blamed),
            "case {case:?}: the refusal names the function it stopped on ({blamed:?}), got \
             {message:?}"
        );
    }
}

/// Defining one ephemeral field name TWICE is still the malformed query it was: the rule is
/// about definitions, which is what listing a field is not.
#[tokio::test]
async fn a_field_defined_twice_over_is_malformed() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/definitions";
    let (start_time, end_time) = group_with_window(&logs, GROUP, None).await;

    // (case, the query string) — each row defines one name in two stages, by a different
    // pair of the three ways a stage defines a field.
    let cases: [(&str, &str); 4] = [
        (
            "two glob parse commands naming one field",
            r#"fields @message | parse @message "lat=*" as lat | parse @message "lon=*" as lat"#,
        ),
        (
            "a named capture and a later rename",
            r"parse @message /lat=(?<lat>\d+)/ | fields lat / 1000 as lat",
        ),
        (
            "two named captures of one name",
            r"parse @message /lat=(?<lat>\d+)/ | parse @message /lon=(?<lat>\d+)/",
        ),
        (
            "a parse and a stats aggregate under one name",
            r#"parse @message "lat=*" as lat | stats count(*) as lat by svc"#,
        ),
    ];
    for (case, query_string) in cases {
        let (code, message) = logs
            .refusal(
                START,
                &json!({
                    "logGroupName": GROUP,
                    "startTime": start_time,
                    "endTime": end_time,
                    "queryString": query_string,
                }),
            )
            .await;
        assert_eq!(
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with message {message:?}"
        );
    }
}

/// A SQL request names the log groups it reads in its statement, and in nothing else.
#[tokio::test]
async fn a_sql_request_names_its_log_groups_in_the_statement_alone() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/sql";
    let (start_time, end_time) = group_with_window(&logs, GROUP, None).await;

    // (case, the request body members beyond the window)
    let refused: [(&str, Value); 4] = [
        (
            "a pipeline query string submitted as SQL",
            json!({
                "queryLanguage": "SQL",
                "queryString": "fields @timestamp, @message | sort @timestamp asc",
            }),
        ),
        (
            "a SQL statement with no FROM clause",
            json!({"queryLanguage": "SQL", "queryString": "SELECT 1"}),
        ),
        (
            "a SQL statement beside the legacy logGroupName member",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT svc FROM `{GROUP}`"),
                "logGroupName": GROUP,
            }),
        ),
        (
            "a SQL statement beside the legacy logGroupNames member",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT svc FROM `{GROUP}`"),
                "logGroupNames": [GROUP],
            }),
        ),
    ];
    for (case, members) in refused {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        object.insert("startTime".to_string(), json!(start_time));
        object.insert("endTime".to_string(), json!(end_time));
        let (code, message) = logs.refusal(START, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with message {message:?}"
        );
    }

    // A query string that IS a statement of the language and does not compile is a MALFORMED
    // query, not a request that named no log groups: the fault is in the statement.
    let malformed: [(&str, &str); 4] = [
        ("a statement truncated at its FROM clause", "SELECT * FROM"),
        (
            "a statement truncated at its condition",
            "SELECT * FROM `/admission/sql` WHERE",
        ),
        (
            "a statement truncated at its grouping",
            "SELECT svc, COUNT(*) AS n FROM `/admission/sql` GROUP BY",
        ),
        (
            "a statement whose string literal nothing closes",
            "SELECT svc FROM `/admission/sql` WHERE svc = 'unclosed",
        ),
    ];
    for (case, query_string) in malformed {
        let (code, message) = logs
            .refusal(
                START,
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": start_time,
                    "endTime": end_time,
                }),
            )
            .await;
        assert_eq!(
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with message {message:?}"
        );
    }

    // A statement whose own FROM clause names the log groups is admitted, including the form
    // that selects them through `logGroups(...)`, and `logGroupIdentifiers` beside it is
    // overridden by the clause rather than refused.
    let accepted: [(&str, Value); 3] = [
        (
            "a FROM clause naming one log group",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc"),
            }),
        ),
        (
            "a FROM clause selecting log groups by identifier",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!(
                    "SELECT COUNT(*) AS n FROM `logGroups(logGroupIdentifier: ['{GROUP}'])`"
                ),
            }),
        ),
        (
            "a FROM clause beside logGroupIdentifiers",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT svc FROM `{GROUP}`"),
                "logGroupIdentifiers": [GROUP],
            }),
        ),
    ];
    for (case, members) in accepted {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        object.insert("startTime".to_string(), json!(start_time));
        object.insert("endTime".to_string(), json!(end_time));
        let (status, answer) = logs.call(START, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {answer}"
        );
        assert!(
            !query_id(&answer).is_empty(),
            "case {case:?}: an accepted query is minted an id, got {answer}"
        );
    }
}

/// One OpenSearch-language query reads log groups of ONE log class.
#[tokio::test]
async fn an_opensearch_query_spanning_two_log_classes_is_refused() {
    let logs = Logs::new();
    const STANDARD: &str = "/admission/class.standard";
    const INFREQUENT: &str = "/admission/class.infrequent";
    let (start_time, end_time) = group_with_window(&logs, STANDARD, Some("STANDARD")).await;
    group_with_window(&logs, INFREQUENT, Some("INFREQUENT_ACCESS")).await;

    // Each class ALONE is admitted, in both languages that name their groups per class.
    let accepted: [(&str, Value); 3] = [
        (
            "SQL over the standard group",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT COUNT(*) AS n FROM `{STANDARD}`"),
            }),
        ),
        (
            "SQL over the infrequent-access group",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!("SELECT COUNT(*) AS n FROM `{INFREQUENT}`"),
            }),
        ),
        (
            "PPL over the infrequent-access group",
            json!({
                "queryLanguage": "PPL",
                "logGroupNames": [INFREQUENT],
                "queryString": "stats count() as n",
            }),
        ),
    ];
    for (case, members) in accepted {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        object.insert("startTime".to_string(), json!(start_time));
        object.insert("endTime".to_string(), json!(end_time));
        let (status, answer) = logs.call(START, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {answer}"
        );
    }

    // The two classes TOGETHER are refused, however the query names them.
    let refused: [(&str, Value); 3] = [
        (
            "SQL naming both classes in its FROM clause",
            json!({
                "queryLanguage": "SQL",
                "queryString": format!(
                    "SELECT COUNT(*) AS n FROM `logGroups(logGroupIdentifier: \
                     ['{STANDARD}', '{INFREQUENT}'])`"
                ),
            }),
        ),
        (
            "PPL naming both classes in logGroupNames",
            json!({
                "queryLanguage": "PPL",
                "logGroupNames": [STANDARD, INFREQUENT],
                "queryString": "stats count() as n",
            }),
        ),
        (
            "PPL naming both classes in logGroupIdentifiers",
            json!({
                "queryLanguage": "PPL",
                "logGroupIdentifiers": [INFREQUENT, STANDARD],
                "queryString": "stats count() as n",
            }),
        ),
    ];
    for (case, members) in refused {
        let mut body = members.clone();
        let object = body.as_object_mut().expect("the request body is an object");
        object.insert("startTime".to_string(), json!(start_time));
        object.insert("endTime".to_string(), json!(end_time));
        let (code, message) = logs.refusal(START, &body).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case:?}: expected {INVALID_PARAMETER}, got {code} with message {message:?}"
        );
    }
}

/// A SQL `HAVING` clause filters on the columns the statement selects, never on an aggregate
/// function expression of its own.
#[tokio::test]
async fn a_having_clause_on_an_aggregate_expression_is_malformed() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/having";
    let (start_time, end_time) = group_with_window(&logs, GROUP, None).await;

    let refused: [(&str, String); 3] = [
        (
            "HAVING on a count",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc HAVING COUNT(*) > 1"
            ),
        ),
        (
            "HAVING on a sum",
            format!(
                "SELECT svc, SUM(ms) AS total FROM `{GROUP}` GROUP BY svc HAVING SUM(ms) > 50"
            ),
        ),
        (
            "HAVING on an aggregate beside a column",
            format!(
                "SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc \
                 HAVING svc = 'a' AND MAX(ms) > 1"
            ),
        ),
    ];
    for (case, query_string) in refused {
        let (code, message) = logs
            .refusal(
                START,
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": start_time,
                    "endTime": end_time,
                }),
            )
            .await;
        assert_eq!(
            code, MALFORMED,
            "case {case:?}: expected {MALFORMED}, got {code} with message {message:?}"
        );
    }

    // The alias forms are the ones the language accepts.
    let accepted: [(&str, String); 2] = [
        (
            "HAVING on a count's alias",
            format!("SELECT svc, COUNT(*) AS n FROM `{GROUP}` GROUP BY svc HAVING n > 1"),
        ),
        (
            "HAVING on a sum's alias",
            format!("SELECT svc, SUM(ms) AS total FROM `{GROUP}` GROUP BY svc HAVING total > 50"),
        ),
    ];
    for (case, query_string) in accepted {
        let (status, answer) = logs
            .call(
                START,
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": start_time,
                    "endTime": end_time,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {answer}"
        );
    }
}

/// One row of the `HAVING` table: what the case is, the clause it adds to the statement, and
/// the `(svc, n, total)` groups the results must carry, in order.
type HavingCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str, &'a str)>);

/// A `HAVING` clause over the columns the projection names RUNS: it keeps the groups the
/// condition selects and drops the rest, which is read back through `GetQueryResults`.
///
/// The rows span what the rule generalises over: which aggregate the alias stands for, which
/// comparison the condition makes, and whether the condition keeps every group, some of them
/// or none — with the no-`HAVING` control showing the groups the statement folds before any
/// filtering, so a condition that did nothing could not pass.
#[tokio::test]
async fn a_having_clause_on_the_projections_columns_filters_the_groups() {
    let logs = Logs::new();
    const GROUP: &str = "/admission/having-results";
    const STREAM: &str = "stream-one";
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
    )
    .await;
    // Two events for `a` (10 + 20 ms) and one for `b` (5 ms), so a condition over a count and
    // one over a sum select different groups.
    let base = base_ms() / 1_000 * 1_000;
    let events: Vec<(i64, String)> = [
        (1_000, r#"{"svc":"a","ms":10}"#),
        (2_000, r#"{"svc":"a","ms":20}"#),
        (3_000, r#"{"svc":"b","ms":5}"#),
    ]
    .iter()
    .map(|(offset, message)| (base + offset, (*message).to_string()))
    .collect();
    let batched: Vec<(i64, &str)> = events
        .iter()
        .map(|(timestamp, message)| (*timestamp, message.as_str()))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&batched),
        }),
    )
    .await;
    let (start_time, end_time) = (base / 1_000 - 60, now_ms() / 1_000 + 60);

    // (case, the HAVING clause, the `(svc, n, total)` rows the results must carry)
    let cases: [HavingCase; 5] = [
        (
            "no HAVING clause, so every group",
            "",
            vec![("a", "2", "30"), ("b", "1", "5")],
        ),
        (
            "HAVING over the count's alias keeps the group with two records",
            " HAVING n > 1",
            vec![("a", "2", "30")],
        ),
        (
            "HAVING over the sum's alias keeps the group whose total is over the bound",
            " HAVING total > 10",
            vec![("a", "2", "30")],
        ),
        (
            "HAVING over the count's alias the other way keeps the other group",
            " HAVING n < 2",
            vec![("b", "1", "5")],
        ),
        (
            "HAVING no group satisfies keeps none",
            " HAVING n > 5",
            vec![],
        ),
    ];
    for (case, having, expected) in cases {
        let query_string = format!(
            "SELECT svc, COUNT(*) AS n, SUM(ms) AS total FROM `{GROUP}` GROUP BY svc{having} \
             ORDER BY svc"
        );
        let answer = logs
            .ok(
                START,
                &json!({
                    "queryLanguage": "SQL",
                    "queryString": query_string,
                    "startTime": start_time,
                    "endTime": end_time,
                }),
            )
            .await;
        let id = query_id(&answer);
        // Past both spans the start armed, so the query has run.
        tokio::time::sleep(std::time::Duration::from_millis(900)).await;
        let results = logs.ok("GetQueryResults", &json!({"queryId": id})).await;
        assert_eq!(
            results["status"].as_str(),
            Some("Complete"),
            "case {case:?}: a query over stored events completes, got {results}"
        );
        let rows: Vec<Vec<(String, String)>> = results["results"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: results is a list, got {results}"))
            .iter()
            .map(|row| {
                row.as_array()
                    .unwrap_or_else(|| panic!("case {case:?}: a row is a list, got {row}"))
                    .iter()
                    .map(|field| {
                        (
                            field["field"].as_str().unwrap_or_default().to_string(),
                            field["value"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .collect()
            })
            .collect();
        let actual: Vec<Vec<(String, String)>> = expected
            .iter()
            .map(|(svc, n, total)| {
                vec![
                    ("svc".to_string(), (*svc).to_string()),
                    ("n".to_string(), (*n).to_string()),
                    ("total".to_string(), (*total).to_string()),
                ]
            })
            .collect();
        assert_eq!(
            rows, actual,
            "case {case:?}: expected the groups the condition selects, got {rows:?} from \
             {results}"
        );
    }
}
