// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the `@pattern` expression a constraint refusal of this service
//! ADVERTISES, over the members of several operations that declare one.
//!
//! The rule under test: a clause quotes the expression the WHOLE value must match. A Smithy
//! `@pattern` is a FIND rather than a total match, so the expression a member declares and
//! the expression its whole value has to satisfy are two different things, and the two ends
//! are decided independently — an end the model ANCHORS carries its `^`/`$` away with it,
//! because the anchor is how the model makes the match total there rather than part of the
//! character grammar, and an end the model leaves OPEN admits any run of characters, which
//! the clause spells with the unconstrained run `.*` unless the expression already leaves
//! that end open with one. A caller that lifts the advertised expression out of the message
//! and re-applies it locally then gets the grammar the service publishes.
//!
//! The table spans the rule rather than one member: its rows walk the three ways a modelled
//! pattern can meet the rule — anchored at neither end (the ARN shapes, `DestinationArn`
//! and `IamRoleArn`), anchored at both (`AppMonitorName`), anchored only at its start
//! (`DimensionName`) — across five operations and across the three places a member is read
//! from (a JSON body, a path label and the query string), and each row asserts the WHOLE
//! message, so the envelope, the value echo, the member path and the quoted expression are
//! all pinned.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so the modelled constraints are applied by the same boundary walk a
//! client's request goes through.

use serde_json::{json, Value};

use crate::support::{
    call, create, emulator, experiment_arn, message, metrics_destination_query, metrics_role_arn,
    put_metrics_destination,
};

/// The error code a constraint refusal carries, and the status the model binds to it.
/// Source: `smithy_query(query="operation", operation="PutRumMetricsDestination")` —
/// `ValidationException`, `httpError` 400.
const VALIDATION_CODE: &str = "ValidationException";
const VALIDATION_STATUS: u16 = 400;

/// The expression a refusal advertises for the ARN shapes of this service.
///
/// `DestinationArn`, `IamRoleArn` and `Arn` are each modelled
/// `@pattern("arn:[^:]*:[^:]*:[^:]*:[^:]*:.*")`, which anchors NEITHER end (the `^` inside
/// `[^:]` is an ordinary member of that character class), so the expression the whole value
/// must match opens with the unconstrained run and keeps the one the modelled expression
/// already ends with.
/// Source: `smithy_query(query="shape", shape="DestinationArn,IamRoleArn,Arn")`.
const ARN_EXPRESSION: &str = r".*arn:[^:]*:[^:]*:[^:]*:[^:]*:.*";

/// The expression a refusal advertises for `AppMonitorName`, modelled
/// `@pattern("^(?!\.)[\.\-_#A-Za-z0-9]+$")`: BOTH ends are anchored, so both anchors go and
/// nothing is added.
/// Source: `smithy_query(query="shape", shape="AppMonitorName")`.
const APP_MONITOR_NAME_EXPRESSION: &str = r"(?!\.)[\.\-_#A-Za-z0-9]+";

/// The expression a refusal advertises for `DimensionName`, the value shape of
/// `DimensionKeysMap`, modelled `@pattern("^(?!:).*[^\s].*")`: its START is anchored, so
/// that anchor goes, and its end is open but already spelled with the unconstrained run.
/// Source: `smithy_query(query="shape", shape="DimensionKeysMap")`, value.
const DIMENSION_NAME_EXPRESSION: &str = r"(?!:).*[^\s].*";

/// A value outside every one of those grammars: it is no ARN, and it opens with the colon
/// `DimensionName`'s guard forbids.
const NOT_AN_ARN: &str = "not-a-valid-arn";

/// A value outside `AppMonitorName`'s grammar: the colon is not one of the characters its
/// class admits.
const NOT_A_NAME: &str = "bad:name";

/// The app monitor the rows that need one are sent to, and the experiment its Evidently
/// destination names.
const MONITOR: &str = "advertised-pattern";

/// One row: the request to send and the clause it must be refused with.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The value the clause echoes.
    value: String,
    /// The dotted member path the clause reports it at.
    member: &'static str,
    /// The expression the clause advertises.
    expression: &'static str,
}

/// The whole message a row is refused with: AWS's envelope over one clause naming the value
/// submitted, the member it was read at and the requirement it failed.
fn refusal_message(row: &Row) -> String {
    format!(
        "1 validation error detected: Value '{}' at '{}' failed to satisfy constraint: \
         Member must satisfy regular expression pattern: {}",
        row.value, row.member, row.expression
    )
}

/// The table: one row per (member, operation, place the member is read from).
fn rows() -> Vec<Row> {
    let experiment = experiment_arn("advertised");
    let role = metrics_role_arn("rum-writer");
    vec![
        // `DestinationArn` in a JSON body, on the put that configures a destination.
        Row {
            case: "PutRumMetricsDestination with a DestinationArn that is no ARN",
            method: "POST",
            path: format!("/rummetrics/{MONITOR}/metricsdestination"),
            body: Some(json!({
                "Destination": "Evidently",
                "DestinationArn": NOT_AN_ARN,
                "IamRoleArn": role.clone(),
            })),
            value: NOT_AN_ARN.to_string(),
            member: "destinationArn",
            expression: ARN_EXPRESSION,
        },
        // `IamRoleArn`, the other ARN member of the same request: one rule, every member
        // that targets a shape it holds for.
        Row {
            case: "PutRumMetricsDestination with an IamRoleArn that is no ARN",
            method: "POST",
            path: format!("/rummetrics/{MONITOR}/metricsdestination"),
            body: Some(json!({
                "Destination": "Evidently",
                "DestinationArn": experiment.clone(),
                "IamRoleArn": NOT_AN_ARN,
            })),
            value: NOT_AN_ARN.to_string(),
            member: "iamRoleArn",
            expression: ARN_EXPRESSION,
        },
        // The same member read from the QUERY STRING by the paginated read.
        Row {
            case: "BatchGetRumMetricDefinitions with a destinationArn that is no ARN",
            method: "GET",
            path: format!(
                "/rummetrics/{MONITOR}/metrics?{}",
                metrics_destination_query("Evidently", Some(NOT_AN_ARN))
            ),
            body: None,
            value: NOT_AN_ARN.to_string(),
            member: "destinationArn",
            expression: ARN_EXPRESSION,
        },
        // And from the query string of the delete, whose other members are bound there too.
        Row {
            case: "BatchDeleteRumMetricDefinitions with a destinationArn that is no ARN",
            method: "DELETE",
            path: format!(
                "/rummetrics/{MONITOR}/metrics?{}",
                metrics_destination_query("Evidently", Some(NOT_AN_ARN))
            ),
            body: None,
            value: NOT_AN_ARN.to_string(),
            member: "destinationArn",
            expression: ARN_EXPRESSION,
        },
        // An EMPTY value is inside `DestinationArn`'s `@length(min: 0)` and outside its
        // pattern, so it reaches the same clause with the empty value echoed.
        Row {
            case: "BatchCreateRumMetricDefinitions with an empty DestinationArn",
            method: "POST",
            path: format!("/rummetrics/{MONITOR}/metrics"),
            body: Some(json!({
                "Destination": "Evidently",
                "DestinationArn": "",
                "MetricDefinitions": [{"Name": "Checkouts", "Namespace": "MyApp/Funnel"}],
            })),
            value: String::new(),
            member: "destinationArn",
            expression: ARN_EXPRESSION,
        },
        // A pattern anchored at BOTH ends: the contrast row, which the rule leaves without
        // either anchor and without an added run.
        Row {
            case: "CreateAppMonitor with a Name outside AppMonitorName's pattern",
            method: "POST",
            path: "/appmonitor".to_string(),
            body: Some(json!({"Name": NOT_A_NAME, "Domain": "example.com"})),
            value: NOT_A_NAME.to_string(),
            member: "name",
            expression: APP_MONITOR_NAME_EXPRESSION,
        },
        // A pattern anchored at its START only, reached as the VALUE of a map entry.
        Row {
            case: "BatchCreateRumMetricDefinitions with a dimension name outside its pattern",
            method: "POST",
            path: format!("/rummetrics/{MONITOR}/metrics"),
            body: Some(json!({
                "Destination": "CloudWatch",
                "MetricDefinitions": [{
                    "Name": "Checkouts",
                    "Namespace": "MyApp/Funnel",
                    "EventPattern": r#"{"event_type":["com.example.checkout"],"metadata":{"pageId":["home"]}}"#,
                    "DimensionKeys": {"metadata.pageId": ":PageId"},
                }],
            })),
            value: ":PageId".to_string(),
            member: "metricDefinitions.1.member.dimensionKeys.metadata.pageId",
            expression: DIMENSION_NAME_EXPRESSION,
        },
    ]
}

#[tokio::test]
async fn every_pattern_refusal_advertises_the_expression_the_whole_value_must_match() {
    let (router, _engine) = emulator();
    create(
        &router,
        "advertised",
        json!({"Name": MONITOR, "Domain": "example.com"}),
    )
    .await;
    // A CloudWatch destination, so the rows whose request is otherwise valid are refused on
    // the member under test rather than on a destination that is not configured.
    put_metrics_destination(&router, "advertised", MONITOR, "CloudWatch", None).await;

    for row in rows() {
        let case = row.case;
        let reply = call(&router, row.method, &row.path, row.body.clone()).await;
        assert_eq!(
            reply.status,
            VALIDATION_STATUS,
            "case {case:?}: expected HTTP {VALIDATION_STATUS}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(case),
            VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(case);
        let expected = refusal_message(&row);
        assert_eq!(
            message(&body),
            Some(expected.as_str()),
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
    }
}
