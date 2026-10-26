// SPDX-License-Identifier: CC0-1.0

//! The widget schema `PutDashboard` checks a `DashboardBody` against before storing it.
//!
//! "For more information about the syntax, see Dashboard Body Structure and Syntax."
//! (`PutDashboardInput$DashboardBody`) — a document that is well-formed JSON but does not
//! satisfy that syntax is refused with `DashboardInvalidInputError` enumerating every problem
//! found, each located by a JSON pointer into the SUBMITTED document, and nothing is stored;
//! a document that satisfies it is stored and round-trips through `GetDashboard`.
//!
//! The table below differs along what that rule generalises over: the widget's type, which
//! member its properties are missing, how the document's JSON types depart from the schema,
//! which widget position is at fault, and how many problems one document holds. The metric
//! widget missing its data source is one row among them rather than the specification.

use crate::harness::{build_router, call, error_code};
use serde_json::{json, Value};
// The emulator's own bounds on what one request can make this handler hold: the largest
// document it stores, and the most problems one refusal enumerates.
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::{
    ops::put_dashboard::MAX_VALIDATION_PROBLEMS, state::MAX_DASHBOARD_BODY_BYTES,
};

/// One row: the case, the document submitted, and the problems the schema must report for it
/// in order — an empty list meaning the document satisfies the schema.
type Case = (&'static str, Value, Vec<(&'static str, &'static str)>);

/// A metric widget's properties naming the series it graphs and the Region they are published
/// in: the form the schema is satisfied by.
fn graphed_metrics() -> Value {
    json!({
        "metrics": [["MyApp", "Latency"]],
        "period": 300,
        "stat": "Average",
        "region": "us-east-1",
    })
}

/// The refusal message CloudWatch words for `problems`: the count, then the problems as the
/// JSON array of `{"dataPath", "message"}` objects the schema produced, indented two spaces.
///
/// Rendered by hand rather than through a pretty-printer, so the layout the caller reads —
/// the newline after the colon, the two-space steps, the member order — is what is asserted.
fn expected_message(problems: &[(&str, &str)]) -> String {
    let mut rendered = String::from("[");
    for (index, (data_path, message)) in problems.iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        rendered.push_str("\n  {\n    \"dataPath\": ");
        rendered.push_str(&json!(data_path).to_string());
        rendered.push_str(",\n    \"message\": ");
        rendered.push_str(&json!(message).to_string());
        rendered.push_str("\n  }");
    }
    rendered.push_str("\n]");
    format!(
        "The dashboard body is invalid, there are {} validation errors:\n{rendered}",
        problems.len()
    )
}

/// The problems a refusal carried in its modelled `DashboardValidationMessages`, as
/// (`DataPath`, `Message`) pairs.
fn reported_problems(body: &Value) -> Vec<(String, String)> {
    body.get("dashboardValidationMessages")
        .and_then(Value::as_array)
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    (
                        message
                            .get("DataPath")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        message
                            .get("Message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every row of the table, built here so the suite's two tests walk the same inputs.
fn cases() -> Vec<Case> {
    // The member each widget type's properties must carry comes from the dashboard body
    // syntax AWS documents for that type: a text widget renders `markdown`, an alarm widget
    // names the `alarms` it watches, a log widget runs a `query`, and a custom widget calls an
    // `endpoint`. A metric widget instead names a data source: the `metrics` it graphs with
    // the `region` they come from, or an alarm `annotations` block.
    vec![
        (
            "a metric widget whose properties name no data source",
            json!({"widgets": [{
                "type": "metric", "x": 0, "y": 0, "width": 24, "height": 6,
                "properties": {"title": "End-to-End Latency"},
            }]}),
            vec![
                (
                    "/widgets/0/properties",
                    "Should have required property 'metrics'",
                ),
                (
                    "/widgets/0/properties",
                    "The metric widget should have specified a region and a data source or an \
                     alarm annotation",
                ),
                (
                    "/widgets/0/properties",
                    "Should have required property 'annotations'",
                ),
            ],
        ),
        (
            "a metric widget graphing metrics from no Region",
            json!({"widgets": [{
                "type": "metric",
                "properties": {"metrics": [["MyApp", "Latency"]], "stat": "Average"},
            }]}),
            vec![
                (
                    "/widgets/0/properties",
                    "The metric widget should have specified a region and a data source or an \
                     alarm annotation",
                ),
                (
                    "/widgets/0/properties",
                    "Should have required property 'annotations'",
                ),
            ],
        ),
        (
            "a metric widget graphing metrics from a Region",
            json!({"widgets": [{
                "type": "metric", "x": 0, "y": 0, "width": 12, "height": 6,
                "properties": graphed_metrics(),
            }]}),
            vec![],
        ),
        (
            "a metric widget annotating an alarm instead",
            json!({"widgets": [{
                "type": "metric",
                "properties": {"annotations": {"alarms": [
                    "arn:aws:cloudwatch:us-east-1:000000000000:alarm:latency",
                ]}},
            }]}),
            vec![],
        ),
        (
            "a text widget carrying no markdown",
            json!({"widgets": [{"type": "text", "properties": {"background": "transparent"}}]}),
            vec![(
                "/widgets/0/properties",
                "Should have required property 'markdown'",
            )],
        ),
        (
            "a text widget carrying markdown",
            json!({"widgets": [{"type": "text", "properties": {"markdown": "# Overview"}}]}),
            vec![],
        ),
        (
            "an alarm widget naming no alarms",
            json!({"widgets": [{"type": "alarm", "properties": {"title": "Health"}}]}),
            vec![(
                "/widgets/0/properties",
                "Should have required property 'alarms'",
            )],
        ),
        (
            "an alarm widget naming the alarms it watches",
            json!({"widgets": [{"type": "alarm", "properties": {"alarms": [
                "arn:aws:cloudwatch:us-east-1:000000000000:alarm:latency",
            ]}}]}),
            vec![],
        ),
        (
            "a log widget carrying no query",
            json!({"widgets": [{"type": "log", "properties": {"region": "us-east-1"}}]}),
            vec![(
                "/widgets/0/properties",
                "Should have required property 'query'",
            )],
        ),
        (
            "a log widget carrying a query",
            json!({"widgets": [{"type": "log", "properties": {
                "query": "SOURCE 'app' | fields @message",
                "region": "us-east-1",
            }}]}),
            vec![],
        ),
        (
            "a custom widget calling no endpoint",
            json!({"widgets": [{"type": "custom", "properties": {"title": "Cost"}}]}),
            vec![(
                "/widgets/0/properties",
                "Should have required property 'endpoint'",
            )],
        ),
        (
            "a custom widget calling an endpoint",
            json!({"widgets": [{"type": "custom", "properties": {
                "endpoint": "arn:aws:lambda:us-east-1:000000000000:function:cost",
            }}]}),
            vec![],
        ),
        (
            "a widget declaring no type",
            json!({"widgets": [{"properties": {"markdown": "# Overview"}}]}),
            vec![("/widgets/0", "Should have required property 'type'")],
        ),
        (
            "a widget whose type is not a string",
            json!({"widgets": [{"type": 7, "properties": {"markdown": "# Overview"}}]}),
            vec![("/widgets/0/type", "Should be string")],
        ),
        (
            "a widget declaring no properties",
            json!({"widgets": [{"type": "text", "x": 0, "y": 0}]}),
            vec![("/widgets/0", "Should have required property 'properties'")],
        ),
        (
            "a widget whose properties are not an object",
            json!({"widgets": [{"type": "text", "properties": "# Overview"}]}),
            vec![("/widgets/0/properties", "Should be object")],
        ),
        (
            "a widget that is not an object",
            json!({"widgets": ["metric"]}),
            vec![("/widgets/0", "Should be object")],
        ),
        (
            "widgets that are not an array",
            json!({"widgets": {"type": "text"}}),
            vec![("/widgets", "Should be array")],
        ),
        (
            "a later widget at fault, the earlier ones sound",
            json!({"widgets": [
                {"type": "text", "properties": {"markdown": "# Overview"}},
                {"type": "metric", "properties": graphed_metrics()},
                {"type": "text", "properties": {}},
            ]}),
            vec![(
                "/widgets/2/properties",
                "Should have required property 'markdown'",
            )],
        ),
        (
            "two widgets at fault, each reported at its own position",
            json!({"widgets": [
                {"type": "alarm", "properties": {}},
                {"type": "text", "properties": {"markdown": "# Overview"}},
                {"type": "log", "properties": {}},
            ]}),
            vec![
                (
                    "/widgets/0/properties",
                    "Should have required property 'alarms'",
                ),
                (
                    "/widgets/2/properties",
                    "Should have required property 'query'",
                ),
            ],
        ),
        (
            "a widget of a kind the schema states no rule for",
            json!({"widgets": [{"type": "explorer", "properties": {"metrics": []}}]}),
            vec![],
        ),
        (
            "a document declaring no widgets at all",
            json!({"start": "-PT6H", "periodOverride": "inherit"}),
            vec![],
        ),
        (
            "a document declaring an empty widget list",
            json!({"widgets": []}),
            vec![],
        ),
    ]
}

/// The name each row's dashboard is written under: distinct per row, so one row's refusal
/// cannot be read as another row's dashboard.
fn name_for(index: usize) -> String {
    format!("body-rule-{index}")
}

#[tokio::test]
async fn a_dashboard_body_is_checked_against_the_widget_schema_before_it_is_stored() {
    for (index, (case, document, problems)) in cases().into_iter().enumerate() {
        let router = build_router();
        let name = name_for(index);
        let body = serde_json::to_string(&document).expect("the document serializes");
        let (status, response) = call(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": body}),
        )
        .await;

        if problems.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a document the schema allows must be stored, got {status} \
                 {response}"
            );
            // "If the input for PutDashboard was correct and the dashboard was successfully
            // created or modified, this result is empty."
            // (`PutDashboardOutput$DashboardValidationMessages`)
            assert_eq!(
                response.get("DashboardValidationMessages"),
                Some(&json!([])),
                "case {case:?}: a stored dashboard reports no validation message, got {response}"
            );
            let got = call(&router, "GetDashboard", &json!({"DashboardName": name})).await;
            assert_eq!(
                (got.0, got.1.get("DashboardBody").and_then(Value::as_str)),
                (200, Some(body.as_str())),
                "case {case:?}: the stored document must round-trip verbatim, got {:?}",
                got.1
            );
            continue;
        }

        // "Some part of the dashboard data is invalid." (`DashboardInvalidInputError`, HTTP
        // 400, `awsQueryError` code `InvalidParameterInput`)
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "DashboardInvalidInputError"),
            "case {case:?}: expected a 400 DashboardInvalidInputError, got {status} {response}"
        );
        let expected = expected_message(&problems);
        assert_eq!(
            response.get("message").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected the enumerated refusal {expected:?}, got {response}"
        );
        let expected_pairs: Vec<(String, String)> = problems
            .iter()
            .map(|(data_path, message)| ((*data_path).to_string(), (*message).to_string()))
            .collect();
        assert_eq!(
            reported_problems(&response),
            expected_pairs,
            "case {case:?}: expected the same problems in DashboardValidationMessages, \
             got {response}"
        );
    }
}

/// A dashboard document of exactly `bytes` bytes: an empty widget list plus padding.
fn padded_body(bytes: usize) -> String {
    let prefix = r#"{"widgets":[],"note":""#;
    let suffix = r#""}"#;
    let fill = bytes - prefix.len() - suffix.len();
    format!("{prefix}{}{suffix}", "p".repeat(fill))
}

#[tokio::test]
async fn the_document_a_request_can_make_the_handler_hold_is_bounded() {
    // Rows span the body-size bound: at it, and one byte past it.
    for (case, bytes, expected_ok) in [
        ("a document of exactly the largest size held", MAX_DASHBOARD_BODY_BYTES, true),
        ("a document one byte past it", MAX_DASHBOARD_BODY_BYTES + 1, false),
    ] {
        let router = build_router();
        let body = padded_body(bytes);
        assert_eq!(
            body.len(),
            bytes,
            "case {case:?}: the row's document must be {bytes} bytes, got {}",
            body.len()
        );
        let (status, response) = call(
            &router,
            "PutDashboard",
            &json!({"DashboardName": "sized", "DashboardBody": body}),
        )
        .await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a document within the bound must be stored, got {status} \
                 {response}"
            );
            continue;
        }
        // "The operation exceeded one or more limits." (`LimitExceededException`, HTTP 400)
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "LimitExceededException"),
            "case {case:?}: expected a 400 LimitExceededException, got {status} {response}"
        );
        let (get_status, got) = call(&router, "GetDashboard", &json!({"DashboardName": "sized"})).await;
        assert_eq!(
            (get_status, error_code(&got).as_str()),
            (404, "DashboardNotFoundError"),
            "case {case:?}: a refused body must store no dashboard, got {get_status} {got}"
        );
    }
}

#[tokio::test]
async fn the_diagnosis_one_document_can_drive_is_bounded() {
    let router = build_router();
    // Each widget of this document is missing its markdown, so the problems the schema finds
    // grow one per widget: twice the bound's worth of widgets must still report the bound's
    // worth of problems, counted by the message and listed in the error.
    let widgets: Vec<Value> = (0..MAX_VALIDATION_PROBLEMS * 2)
        .map(|_| json!({"type": "text", "properties": {}}))
        .collect();
    let body = serde_json::to_string(&json!({"widgets": widgets})).expect("the document serializes");
    let (status, response) = call(
        &router,
        "PutDashboard",
        &json!({"DashboardName": "many-problems", "DashboardBody": body}),
    )
    .await;

    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "DashboardInvalidInputError"),
        "case \"every widget at fault\": expected a 400 DashboardInvalidInputError, got {status} \
         {response}"
    );
    let reported = reported_problems(&response);
    assert_eq!(
        reported.len(),
        MAX_VALIDATION_PROBLEMS,
        "case \"every widget at fault\": expected the enumeration capped at \
         {MAX_VALIDATION_PROBLEMS}, got {}",
        reported.len()
    );
    let expected: Vec<(&str, &str)> = reported
        .iter()
        .map(|(data_path, _)| {
            (
                data_path.as_str(),
                "Should have required property 'markdown'",
            )
        })
        .collect();
    assert_eq!(
        response.get("message").and_then(Value::as_str),
        Some(expected_message(&expected).as_str()),
        "case \"every widget at fault\": the message must count and render the capped \
         enumeration, got {response}"
    );
    // The first widgets are the ones diagnosed, each at its own position.
    assert_eq!(
        reported.first().map(|(data_path, _)| data_path.as_str()),
        Some("/widgets/0/properties"),
        "case \"every widget at fault\": expected the first problem at the first widget, \
         got {reported:?}"
    );
    assert_eq!(
        reported.last().map(|(data_path, _)| data_path.as_str()),
        Some(format!("/widgets/{}/properties", MAX_VALIDATION_PROBLEMS - 1).as_str()),
        "case \"every widget at fault\": expected the last problem at the last widget walked, \
         got {reported:?}"
    );
    let (get_status, got) = call(
        &router,
        "GetDashboard",
        &json!({"DashboardName": "many-problems"}),
    )
    .await;
    assert_eq!(
        (get_status, error_code(&got).as_str()),
        (404, "DashboardNotFoundError"),
        "case \"every widget at fault\": a refused body must store no dashboard, got \
         {get_status} {got}"
    );
}

#[tokio::test]
async fn a_body_the_widget_schema_refuses_stores_no_dashboard() {
    let router = build_router();
    let mut refused: Vec<String> = Vec::new();
    let mut stored: Vec<String> = Vec::new();
    for (index, (case, document, problems)) in cases().into_iter().enumerate() {
        let name = name_for(index);
        let body = serde_json::to_string(&document).expect("the document serializes");
        let (status, response) = call(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": body}),
        )
        .await;
        if problems.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: a document the schema allows must be stored, got {status} \
                 {response}"
            );
            stored.push(name);
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a document the schema refuses must not be stored, got {status} \
                 {response}"
            );
            refused.push(name);
        }
    }

    for name in &refused {
        // "The specified dashboard does not exist." (`DashboardNotFoundError`, HTTP 404,
        // `awsQueryError` code `ResourceNotFound`)
        let (status, response) = call(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (404, "DashboardNotFoundError"),
            "case {name:?}: a refused body must leave no dashboard, got {status} {response}"
        );
        assert_eq!(
            response.get("message").and_then(Value::as_str),
            Some(format!("Dashboard {name} does not exist").as_str()),
            "case {name:?}: expected the refusal to name the dashboard asked for, got {response}"
        );
    }

    // Only the documents the schema allowed are held, so a refusal left nothing behind and
    // took nothing away.
    let listed = call(&router, "ListDashboards", &json!({})).await;
    let reported: Vec<String> = listed
        .1
        .get("DashboardEntries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("DashboardName").and_then(Value::as_str))
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default();
    let mut expected = stored;
    expected.sort();
    assert_eq!(
        reported, expected,
        "case \"after every row\": expected only the allowed documents held, got {:?}",
        listed.1
    );
}
