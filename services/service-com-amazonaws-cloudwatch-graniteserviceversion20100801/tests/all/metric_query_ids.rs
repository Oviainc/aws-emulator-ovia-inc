// SPDX-License-Identifier: CC0-1.0

//! A `MetricDataQuery` array is judged by its own SHAPE before any series it names is read,
//! and every result it is answered with is tied back to the query that asked for it.
//!
//! Three rules:
//!
//! 1. **Every `Id` in a query array has to match the grammar its member documents, and the ids
//!    of one array have to be unique**: "A short name used to tie this object to the results in
//!    the response. This name must be unique within a single call to `GetMetricData`. […] The
//!    valid characters are letters, numbers, and underscore. The first character must be a
//!    lowercase letter." (`smithy_query(query="shape",shape="MetricDataQuery")`, member `Id`) —
//!    the member carries no modelled constraint, so nothing refuses such an id before a handler
//!    runs.
//! 2. **A query states the series it answers in exactly one of two members**: "Within one
//!    MetricDataQuery object, you must specify either Expression or MetricStat but not both."
//!    (same shape, member `MetricStat`)
//! 3. **A result carries the label its query names, and the default for an expression result is
//!    the query's own `Id`** — never the expression's source text — while the default for a
//!    retrieval is the metric it names (same shape, member `Label`: "If Label is omitted,
//!    CloudWatch generates a default").
//!
//! An array that breaks rule 1 or rule 2 is refused as the request-validation failure it is,
//! naming the array member it arrived on. The SAME array is what a read answers from
//! (`GetMetricDataInput$MetricDataQueries`) and what a metric-math alarm's configuration IS
//! (`PutMetricAlarmInput$Metrics`), so both operations are driven here: the refusal names the
//! member the array arrived on, and nothing else about it differs.

use serde_json::{json, Value};

use crate::harness::{build_router, call, call_in_query_mode, error_code, error_message, ok};

/// The namespace the queries of this suite retrieve from.
const NAMESPACE: &str = "Probe/QueryIds";

/// The grammar an id has to match, as the refusal publishes it (`MetricDataQuery$Id`).
const GRAMMAR: &str = "^[a-z][a-zA-Z0-9_]*$";

/// A `MetricStat` query under `id` retrieving one series; `returns` is its `ReturnData`.
fn stat_query(id: &str, returns: bool) -> Value {
    json!({
        "Id": id,
        "ReturnData": returns,
        "MetricStat": {
            "Metric": {"Namespace": NAMESPACE, "MetricName": "Latency"},
            "Period": 60,
            "Stat": "Sum",
        },
    })
}

/// A `GetMetricData` request over `queries`, covering the last ten minutes.
fn read(queries: Value) -> Value {
    json!({
        "MetricDataQueries": queries,
        "StartTime": crate::harness::now_secs() - 600.0,
        "EndTime": crate::harness::now_secs(),
    })
}

/// A metric-math `PutMetricAlarm` request for `name` watching `queries`.
fn math_alarm(name: &str, queries: Value) -> Value {
    json!({
        "AlarmName": name,
        "Metrics": queries,
        "EvaluationPeriods": 1,
        "Threshold": 10.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// The class every refusal of this rule carries: the request-validation class, which the
/// `awsQueryCompatible` envelope names `com.amazon.coral.validate#ValidationException`.
const VALIDATION_CLASS: &str = "ValidationException";

#[tokio::test]
async fn a_query_id_must_match_the_grammar_its_member_documents() {
    let router = build_router();

    // The rows differ along what the grammar generalises over: the case of the first
    // character, whether it is a letter at all, and which characters follow it — including an
    // id no report of this behaviour cites (`metric_total_99`) and a non-ASCII letter.
    //
    // (case, the id, whether the array is accepted)
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a lowercase letter and a digit", "e1", true),
        ("a single lowercase letter", "e", true),
        ("letters, digits and an underscore", "m_1A9", true),
        ("a long name no report of this cites", "metric_total_99", true),
        ("an uppercase first character", "E1", false),
        ("a digit first", "1e", false),
        ("an underscore first", "_e", false),
        ("a hyphen inside", "e-1", false),
        ("a space inside", "e 1", false),
        ("a dot inside", "e.1", false),
        ("a non-ASCII letter", "é1", false),
    ];

    for (case, id, accepted) in rows {
        // The offending id rides in the SECOND query, so the refusal has a position to name
        // that is not the first.
        let queries = json!([stat_query("m1", false), {"Id": id, "Expression": "m1 * 2"}]);
        let (status, body) = call(&router, "GetMetricData", &read(queries)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the array to be accepted, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CLASS,
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let expected = format!(
            "The value for parameter MetricDataQueries.member.2.Id is not matching the expected \
             pattern {GRAMMAR}.\n"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn the_ids_of_one_query_array_are_unique() {
    let router = build_router();

    // The rows differ along how the repeat arises: two retrievals under one id, a retrieval
    // and an expression under one id, a repeat at the end of a longer array — and, accepted,
    // the same queries under ids that differ.
    //
    // (case, the array, whether it is accepted)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "two retrievals sharing one id",
            json!([stat_query("m1", true), stat_query("m1", true)]),
            false,
        ),
        (
            "a retrieval and an expression sharing one id",
            json!([stat_query("m1", false), {"Id": "m1", "Expression": "m1 * 2"}]),
            false,
        ),
        (
            "a repeat at the end of a longer array",
            json!([
                stat_query("m1", false),
                stat_query("m2", false),
                {"Id": "m1", "Expression": "m1 + m2"},
            ]),
            false,
        ),
        (
            "the same queries under ids that differ",
            json!([stat_query("m1", true), stat_query("m2", true)]),
            true,
        ),
    ];

    for (case, queries, accepted) in rows {
        let (status, body) = call(&router, "GetMetricData", &read(queries)).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the array to be accepted, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CLASS,
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let expected = "The values for parameter id in MetricDataQueries are not unique.\n";
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn an_alarms_query_array_is_judged_by_the_same_id_rule() {
    let router = build_router();

    // (case, the array, the refusal message, or none for an array that configures an alarm)
    let rows: Vec<(&str, Value, Option<String>)> = vec![
        (
            "an id outside the grammar",
            json!([{"Id": "E1", "Expression": "100"}]),
            Some(format!(
                "The value for parameter Metrics.member.1.Id is not matching the expected \
                 pattern {GRAMMAR}.\n"
            )),
        ),
        (
            "an id outside the grammar in the second query",
            json!([stat_query("m1", false), {"Id": "e 1", "Expression": "m1 * 2"}]),
            Some(format!(
                "The value for parameter Metrics.member.2.Id is not matching the expected \
                 pattern {GRAMMAR}.\n"
            )),
        ),
        (
            "two queries sharing one id",
            json!([
                stat_query("m1", false),
                {"Id": "m1", "Expression": "m1 * 2"},
            ]),
            Some("The values for parameter id in Metrics are not unique.\n".to_string()),
        ),
        (
            "an array whose ids are a set the member admits",
            json!([stat_query("m1", false), {"Id": "e1", "Expression": "m1 * 2"}]),
            None,
        ),
    ];

    for (index, (case, queries, refusal)) in rows.into_iter().enumerate() {
        let name = format!("query-ids-{index}");
        let (status, body) = call(&router, "PutMetricAlarm", &math_alarm(&name, queries)).await;
        let Some(expected) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the alarm to be created, got {status} with {body}"
            );
            // Round trip: an accepted configuration is stored and reads back.
            let described = ok(&router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
            let ids: Vec<&str> = described["MetricAlarms"][0]["Metrics"]
                .as_array()
                .map(|queries| {
                    queries
                        .iter()
                        .filter_map(|query| query["Id"].as_str())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                ids,
                vec!["m1", "e1"],
                "case {case:?}: expected the stored alarm to carry the ids it was created \
                 with, got {described}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CLASS,
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// One row of the query-shape table: the array submitted, and the sentence the refusal has to
/// carry.
///
/// A row rather than a tuple so the table states what each column is.
struct ShapeRow {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The `MetricDataQueries` array the row sends.
    queries: Value,
    /// The message the refusal carries, or `None` for an array this service answers.
    refusal: Option<String>,
}

/// A `MetricStat` naming the series the queries of this suite retrieve.
fn metric_stat() -> Value {
    json!({
        "Metric": {"Namespace": NAMESPACE, "MetricName": "Latency"},
        "Period": 60,
        "Stat": "Sum",
    })
}

#[tokio::test]
async fn a_query_states_the_series_it_answers_in_exactly_one_of_two_members() {
    let router = build_router();

    // "Within one MetricDataQuery object, you must specify either Expression or MetricStat but
    // not both." (`MetricDataQuery$MetricStat`) — the rows differ along which half of that
    // either/or the query breaks, and WHERE the offending query sits, which is what the
    // refusal's member path names; the accepted rows carry one of the two members each.
    let rows: Vec<ShapeRow> = vec![
        ShapeRow {
            case: "the first query names both",
            queries: json!([{"Id": "m1", "Expression": "100", "MetricStat": metric_stat()}]),
            refusal: Some(mutually_exclusive("MetricDataQueries.member.1")),
        },
        ShapeRow {
            case: "the second query names both",
            queries: json!([
                stat_query("m1", false),
                {"Id": "e1", "Expression": "m1 * 2", "MetricStat": metric_stat()},
            ]),
            refusal: Some(mutually_exclusive("MetricDataQueries.member.2")),
        },
        ShapeRow {
            case: "the only query names neither",
            queries: json!([{"Id": "m1"}]),
            refusal: Some(required("MetricDataQueries.member.1.MetricStat")),
        },
        ShapeRow {
            case: "the second query names neither",
            queries: json!([stat_query("m1", true), {"Id": "m2"}]),
            refusal: Some(required("MetricDataQueries.member.2.MetricStat")),
        },
        ShapeRow {
            case: "a retrieval alone",
            queries: json!([stat_query("m1", true)]),
            refusal: None,
        },
        ShapeRow {
            case: "an expression over a retrieval",
            queries: json!([stat_query("m1", false), {"Id": "e1", "Expression": "m1 * 2"}]),
            refusal: None,
        },
    ];

    for ShapeRow { case, queries, refusal } in rows {
        let (status, body) = call(&router, "GetMetricData", &read(queries)).await;
        let Some(expected) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the array to be accepted, got {status} with {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CLASS,
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// The sentence a query naming BOTH members is refused with.
fn mutually_exclusive(path: &str) -> String {
    format!(
        "The parameters {path}.Expression and {path}.MetricStat are mutually exclusive and you \
         have specified both.\n"
    )
}

/// The sentence a query naming NEITHER is refused with: the member the request-validation
/// layer reports as missing.
fn required(parameter: &str) -> String {
    format!("The parameter {parameter} is required.\n")
}

#[tokio::test]
async fn a_result_carries_the_label_the_query_it_answers_names() {
    let router = build_router();
    // One datum, so every row's result carries a point as well as a label.
    ok(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{
                "MetricName": "Latency",
                "Timestamp": crate::harness::now_secs() - 120.0,
                "Value": 4.0,
            }],
        }),
    )
    .await;

    // "A human-readable label for this metric or expression. […] If the metric or expression
    // is shown in a CloudWatch dashboard widget, the label is shown. If Label is omitted,
    // CloudWatch generates a default." (`smithy_query(query="shape",shape="MetricDataQuery"`,
    // member `Label`) — the rows differ along which kind of query answers the result and
    // whether it names a label of its own: the default for an EXPRESSION result is the query's
    // own `Id`, never the expression's source text, and for a retrieval it is the metric the
    // query names.
    //
    // (case, the query, the label its result carries)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "an expression with no label of its own",
            json!({"Id": "e1", "Expression": "m1*2"}),
            "e1",
        ),
        (
            "another expression with no label, under another id",
            json!({"Id": "doubled_latency", "Expression": "m1 + m1"}),
            "doubled_latency",
        ),
        (
            "an expression that names its own label",
            json!({"Id": "e1", "Expression": "m1*2", "Label": "twice the latency"}),
            "twice the latency",
        ),
        (
            "a retrieval with no label of its own",
            json!({"Id": "m2", "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": "Latency"},
                "Period": 60,
                "Stat": "Sum",
            }}),
            "Latency",
        ),
        (
            "a retrieval that names its own label",
            json!({"Id": "m2", "Label": "the latency", "MetricStat": {
                "Metric": {"Namespace": NAMESPACE, "MetricName": "Latency"},
                "Period": 60,
                "Stat": "Sum",
            }}),
            "the latency",
        ),
    ];

    for (case, query, expected) in rows {
        let body = ok(
            &router,
            "GetMetricData",
            &read(json!([stat_query("m1", false), query])),
        )
        .await;
        let labels: Vec<&str> = body["MetricDataResults"]
            .as_array()
            .map(|results| {
                results
                    .iter()
                    .filter_map(|result| result["Label"].as_str())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            labels,
            vec![expected],
            "case {case:?}: expected the result to be labelled {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn an_expression_calling_a_function_nothing_computes_names_the_function() {
    let router = build_router();

    // The rows differ along the case the call was written in and the shape of the name: the
    // refusal quotes the name as the EXPRESSION wrote it, since that is what a caller edits,
    // and the known names are resolved without regard to case
    // (`crate::metric_math` tables the functions this service computes).
    //
    // (case, the expression, the function the refusal names)
    let rows: Vec<(&str, &str, &str)> = vec![
        ("an upper-case name", "FOO(m1)", "FOO"),
        ("the same name in lower case", "foo(m1)", "foo"),
        ("a name of mixed case", "Rate_Of(m1)", "Rate_Of"),
    ];

    for (case, expression, named) in rows {
        let queries = json!([
            stat_query("m1", false),
            {"Id": "e1", "Expression": expression},
        ]);
        let (status, body) = call(&router, "GetMetricData", &read(queries)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            VALIDATION_CLASS,
            "case {case:?}: expected the request-validation class, got {body}"
        );
        let expected = format!("Error in expression 'e1': Function '{named}' not found");
        let actual = error_message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_refused_query_array_names_its_class_twice_for_a_query_compatible_client() {
    let router = build_router();
    let queries = json!([stat_query("m1", true), stat_query("m1", true)]);
    let (status, headers, body) =
        call_in_query_mode(&router, "GetMetricData", &read(queries)).await;

    assert_eq!(status, 400, "expected HTTP 400, got {status} with {body}");
    assert_eq!(
        body["__type"].as_str(),
        Some("com.amazon.coral.validate#ValidationException"),
        "the body names the request-validation class in full, got {body}"
    );
    let reported = headers
        .get("x-amzn-query-error")
        .and_then(|value| value.to_str().ok());
    assert_eq!(
        reported,
        Some("ValidationError;Sender"),
        "the header reports the class's legacy code and the fault side, got {reported:?}"
    );
}
