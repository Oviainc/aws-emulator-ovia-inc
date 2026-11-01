// SPDX-License-Identifier: CC0-1.0

//! How this service NAMES the class of a failure — the error envelope a service that migrated
//! off the query protocol answers with.
//!
//! The rule under test: **CloudWatch's model declares `awsJson1_0` beside `awsQuery` and
//! carries the `awsQueryCompatible` modifier, so every failure names its error class TWICE**
//! (`smithy_query(query="service_info")`):
//!
//! 1. the body's `__type` is the FULLY QUALIFIED shape id of the class — the
//!    request-validation class in the package of the validation layer in front of the service,
//!    and the service's own modelled faults in the service package;
//! 2. the `x-amzn-query-error` header carries that class's `awsQueryError` code and the fault
//!    side (`Sender` below 500), for a client that opted in with `x-amzn-query-mode: true`;
//!    a client that did not ask gets the unadorned JSON error.
//!
//! Both parts are fixed text of the CLASS, so the rows below differ along the class and along
//! the LAYER that refused — a handler's modelled error, a handler's refusal of a precondition
//! the model cannot express, the model's own constraint walk, and the value rules this service
//! judges ahead of a handler — because a client branching on the legacy code must get it
//! whichever layer answered. The recorded cases (a delete a composite alarm depends on, a log
//! alarm's name conflict, a dimension set over the cap, a bad pagination argument, a dataset
//! identifier of the wrong form) are five rows among them.
//!
//! Two renderings of the same envelope are tabled beside it, because the recordings show them
//! on these refusals: a `@pattern` requirement is quoted WITHOUT the anchors the model wraps it
//! in, and a value supplied EMPTY is reported as a parameter the request gave no value for.

use serde_json::{json, Value};

use crate::harness::{
    build_router, call, call_in_query_mode, call_with_headers, error_message, log_alarm_request,
    merged, metric_alarm_request, ok, AlarmKind,
};

/// The header the legacy query error code and the fault side are reported in.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The shape id the body names the request-validation class under: the class belongs to the
/// validation layer in front of the service, which is the package the id names.
const VALIDATION_SHAPE_ID: &str = "com.amazon.coral.validate#ValidationException";

/// The package this service's own modelled faults are named in.
const SERVICE_PACKAGE: &str = "com.amazon.coral.service";

/// The shape id of the modelled parameter-value fault.
fn parameter_value_shape_id() -> String {
    format!("{SERVICE_PACKAGE}#InvalidParameterValueException")
}

/// The shape id of the modelled missing-parameter fault.
fn missing_parameter_shape_id() -> String {
    format!("{SERVICE_PACKAGE}#MissingRequiredParameterException")
}

/// The `__type` a failure body carries, verbatim — the qualified id is the thing under test
/// here, so this reads it unreduced rather than through the harness's bare-name reader.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One refused call: what it varies, how to reach the refusal, and the two names the answer
/// owes for it.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The alarms the row needs in place before the refused call, as (operation, request).
    given: Vec<(&'static str, Value)>,
    operation: &'static str,
    request: Value,
    status: u16,
    /// The qualified shape id the body must name.
    shape_id: String,
    /// The report the header must carry for a query-compatible client.
    report: &'static str,
}

/// A `PutMetricData` request publishing one datum carrying `dimensions`.
fn publish_with_dimensions(dimensions: Value) -> Value {
    json!({
        "Namespace": "MyApp/Envelope",
        "MetricData": [{
            "MetricName": "Latency",
            "Value": 1.0,
            "Dimensions": dimensions,
        }],
    })
}

/// `count` well-formed dimensions, each named for its position.
fn dimensions(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Name": format!("d{index}"), "Value": format!("v{index}")}))
            .collect(),
    )
}

#[tokio::test]
async fn every_failure_names_its_class_in_the_body_and_in_the_legacy_header() {
    // The rows span the CLASS and the LAYER that refused, which is what the rule generalises
    // over; the operation and the member are incidental to it.
    let rows: Vec<Row> = vec![
        Row {
            case: "a delete a composite alarm depends on, refused by the handler",
            given: vec![
                ("PutMetricAlarm", metric_alarm_request("watched")),
                (
                    "PutCompositeAlarm",
                    json!({"AlarmName": "watcher", "AlarmRule": "ALARM(\"watched\")"}),
                ),
            ],
            operation: "DeleteAlarms",
            request: json!({"AlarmNames": ["watched"]}),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "a log alarm taking a name a metric alarm holds, refused by the handler",
            given: vec![("PutMetricAlarm", metric_alarm_request("taken"))],
            operation: "PutLogAlarm",
            request: log_alarm_request("taken"),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "an alarm rule naming no alarm, refused by the handler",
            given: vec![],
            operation: "PutCompositeAlarm",
            request: json!({"AlarmName": "orphan", "AlarmRule": "ALARM(\"nothing\")"}),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "a pagination argument below its modelled minimum, refused by the walk",
            given: vec![],
            operation: "ListAlarmMuteRules",
            request: json!({"MaxRecords": 0}),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "the same argument on another reader, refused by the walk",
            given: vec![],
            operation: "DescribeAlarms",
            request: json!({"MaxRecords": 0}),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "an identifier of a form no dataset has, refused by the walk",
            given: vec![],
            operation: "GetDataset",
            request: json!({"DatasetIdentifier": "not-a-dataset"}),
            status: 400,
            shape_id: VALIDATION_SHAPE_ID.to_string(),
            report: "ValidationError;Sender",
        },
        Row {
            case: "a dimension set one past the per-metric cap, refused by the walk",
            given: vec![],
            operation: "PutMetricData",
            request: publish_with_dimensions(dimensions(31)),
            status: 400,
            shape_id: parameter_value_shape_id(),
            report: "InvalidParameterValue;Sender",
        },
        Row {
            case: "a publish into the reserved namespace, refused by the handler",
            given: vec![],
            operation: "PutMetricData",
            request: json!({
                "Namespace": "AWS/ES",
                "MetricData": [{"MetricName": "SearchableDocuments", "Value": 1.0}],
            }),
            status: 400,
            shape_id: parameter_value_shape_id(),
            report: "InvalidParameterValue;Sender",
        },
        Row {
            case: "a stream name no stream can carry, refused ahead of the walk",
            given: vec![],
            operation: "PutMetricStream",
            request: merged(
                crate::metric_streams::put_request("a name with spaces", "json"),
                &json!({}),
            ),
            status: 400,
            shape_id: parameter_value_shape_id(),
            report: "InvalidParameterValue;Sender",
        },
        Row {
            case: "a dimension value supplied empty, refused by the walk",
            given: vec![],
            operation: "PutMetricData",
            request: publish_with_dimensions(json!([{"Name": "Probe", "Value": ""}])),
            status: 400,
            shape_id: missing_parameter_shape_id(),
            report: "MissingParameter;Sender",
        },
    ];

    for Row {
        case,
        given,
        operation,
        request,
        status,
        shape_id,
        report,
    } in rows
    {
        // A fresh emulator per row, so each row's own setup is the whole state it is refused
        // against.
        let router = build_router();
        for (setup, body) in &given {
            ok(&router, setup, body).await;
        }

        // 1. A query-compatible client reads both names.
        let (answered, headers, body) = call_in_query_mode(&router, operation, &request).await;
        assert_eq!(
            answered, status,
            "case {case:?}: expected HTTP {status}, got {answered} with {body}"
        );
        assert_eq!(
            error_type(&body),
            shape_id,
            "case {case:?}: expected the body to name {shape_id}, got {body}"
        );
        let reported = headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            reported, report,
            "case {case:?}: expected {QUERY_ERROR_HEADER} to read {report:?}, got \
             {reported:?}"
        );

        // 2. A native JSON client gets the same body and NO report: the legacy code is an
        // addition for a client that asked for it, never a change to the answer.
        let (plain_status, plain_headers, plain_body) =
            call_with_headers(&router, operation, &request).await;
        assert_eq!(
            (plain_status, error_type(&plain_body)),
            (status, shape_id.clone()),
            "case {case:?}: a native JSON client must get the same answer, got {plain_status} \
             with {plain_body}"
        );
        assert!(
            plain_headers.get(QUERY_ERROR_HEADER).is_none(),
            "case {case:?}: a client that did not opt in must get no report, got {:?}",
            plain_headers.get(QUERY_ERROR_HEADER)
        );
    }
}

#[tokio::test]
async fn a_served_answer_carries_no_class_report() {
    // Rows: a write, a read of what it wrote, and a read of an empty collection — a success
    // names no error class, so none of them carries the report, opt-in or not.
    let router = build_router();
    AlarmKind::Metric.create(&router, "served", None).await;
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a write",
            "PutMetricAlarm",
            metric_alarm_request("served-again"),
        ),
        (
            "a read of one alarm",
            "DescribeAlarms",
            json!({"AlarmNames": ["served"]}),
        ),
        (
            "a read that reports nothing",
            "DescribeAlarms",
            json!({"AlarmNames": ["never-created"]}),
        ),
    ];
    for (case, operation, request) in rows {
        let (status, headers, body) = call_in_query_mode(&router, operation, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the call to be served, got {status} with {body}"
        );
        assert!(
            headers.get(QUERY_ERROR_HEADER).is_none(),
            "case {case:?}: a served answer names no error class, got {:?}",
            headers.get(QUERY_ERROR_HEADER)
        );
        assert!(
            body.get("__type").is_none(),
            "case {case:?}: a served answer carries no error type, got {body}"
        );
    }
}

/// The clause a constraint refusal states a `@pattern` requirement in.
const PATTERN_CLAUSE: &str = "Member must satisfy regular expression pattern: ";

/// The members whose `@pattern` a refusal of these operations quotes, as the model declares
/// them — anchors included, which is what the refusal must NOT state
/// (`smithy_query(query="operation",operation="GetDataset,AssociateDatasetKmsKey,DisassociateDatasetKmsKey")`).
const MODELLED_PATTERNS: [(&str, &str); 2] = [
    (
        "datasetIdentifier",
        r"^(default|arn:[a-zA-Z0-9-]+:cloudwatch:[a-zA-Z0-9-]*:\d{12}:dataset/default)$",
    ),
    (
        "kmsKeyArn",
        r"^arn:[a-zA-Z0-9-]+:kms:[a-zA-Z0-9-]+:\d{12}:key/[a-f0-9-]+$",
    ),
];

/// That expression without the whole-value anchors the model wraps it in: the leading `^` and
/// the trailing `$`, which are how the model makes the match total rather than part of the
/// expression the service advertises.
fn unanchored(pattern: &str) -> &str {
    pattern
        .strip_prefix('^')
        .and_then(|body| body.strip_suffix('$'))
        .unwrap_or(pattern)
}

#[tokio::test]
async fn a_pattern_requirement_is_quoted_without_the_models_anchors() {
    // (case, operation, request, the member the clause names, the submitted value it echoes).
    // The rows span both patterned members and every operation that takes one, and the values
    // differ in form (a bare name, a near-miss ARN of the wrong service, an ARN of the right
    // service naming another resource) — the requirement is a property of the MEMBER, so every
    // row quotes the same expression for it.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a read naming a bare dataset name",
            "GetDataset",
            json!({"DatasetIdentifier": "my-dataset"}),
            "datasetIdentifier",
            "my-dataset",
        ),
        (
            "a read naming an ARN of another service",
            "GetDataset",
            json!({"DatasetIdentifier": "arn:aws:logs:us-east-1:000000000000:dataset/default"}),
            "datasetIdentifier",
            "arn:aws:logs:us-east-1:000000000000:dataset/default",
        ),
        (
            "an association naming a dataset that is not the default one",
            "AssociateDatasetKmsKey",
            json!({
                "DatasetIdentifier": "arn:aws:cloudwatch:us-east-1:000000000000:dataset/other",
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:key/\
                              1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d",
            }),
            "datasetIdentifier",
            "arn:aws:cloudwatch:us-east-1:000000000000:dataset/other",
        ),
        (
            "a removal naming a bare dataset name",
            "DisassociateDatasetKmsKey",
            json!({"DatasetIdentifier": "my-dataset"}),
            "datasetIdentifier",
            "my-dataset",
        ),
        (
            "an association naming a key ARN of the wrong form",
            "AssociateDatasetKmsKey",
            json!({
                "DatasetIdentifier": "default",
                "KmsKeyArn": "arn:aws:kms:us-east-1:000000000000:alias/not-a-key-id",
            }),
            "kmsKeyArn",
            "arn:aws:kms:us-east-1:000000000000:alias/not-a-key-id",
        ),
    ];

    let router = build_router();
    for (case, operation, request, member, submitted) in rows {
        let modelled = MODELLED_PATTERNS
            .iter()
            .find(|(name, _)| *name == member)
            .map(|(_, pattern)| *pattern)
            .unwrap_or_else(|| panic!("case {case:?}: {member} has a modelled pattern"));
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        let message = error_message(&body);
        let expected = format!(
            "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
             constraint: {PATTERN_CLAUSE}{}",
            unanchored(modelled)
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
        );
        // Stated separately, because the anchors are the whole point: whatever else the clause
        // carries, the expression it quotes is not the anchored one.
        for quoted in message.split(PATTERN_CLAUSE).skip(1) {
            assert!(
                !quoted.starts_with('^') && !quoted.ends_with('$'),
                "case {case:?}: the quoted expression must carry no anchors, got {quoted:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_value_supplied_empty_is_reported_as_a_parameter_with_no_value() {
    // (case, operation, request, the request member path the refusal names). A parameter
    // carrying no characters is a parameter the request gave no value for, so the rows span
    // where such a value can sit: a top-level member, a member inside a list element, and the
    // same member at another position in that list — the path is the request's, not a fixed
    // one.
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "the namespace of a publish",
            "PutMetricData",
            json!({
                "Namespace": "",
                "MetricData": [{"MetricName": "Latency", "Value": 1.0}],
            }),
            "Namespace",
        ),
        (
            "a dimension's value inside the first datum",
            "PutMetricData",
            publish_with_dimensions(json!([{"Name": "Probe", "Value": ""}])),
            "MetricData.member.1.Dimensions.member.1.Value",
        ),
        (
            "a dimension's name inside the first datum",
            "PutMetricData",
            publish_with_dimensions(json!([{"Name": "", "Value": "v"}])),
            "MetricData.member.1.Dimensions.member.1.Name",
        ),
        (
            "a dimension's value third in its collection",
            "PutMetricData",
            publish_with_dimensions(json!([
                {"Name": "a", "Value": "1"},
                {"Name": "b", "Value": "2"},
                {"Name": "c", "Value": ""},
            ])),
            "MetricData.member.1.Dimensions.member.3.Value",
        ),
        (
            "the name of a metric-stream read",
            "GetMetricStream",
            json!({"Name": ""}),
            "Name",
        ),
        (
            "an alarm name a reader filters by",
            "DescribeAlarms",
            json!({"AlarmNamePrefix": ""}),
            "AlarmNamePrefix",
        ),
    ];

    let router = build_router();
    for (case, operation, request, parameter) in rows {
        let (status, headers, body) = call_in_query_mode(&router, operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            missing_parameter_shape_id(),
            "case {case:?}: expected the missing-parameter class, got {body}"
        );
        let message = error_message(&body);
        let expected = format!("The parameter {parameter} is required.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
        );
        let reported = headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            reported, "MissingParameter;Sender",
            "case {case:?}: expected the legacy missing-parameter code, got {reported:?}"
        );
    }
}
