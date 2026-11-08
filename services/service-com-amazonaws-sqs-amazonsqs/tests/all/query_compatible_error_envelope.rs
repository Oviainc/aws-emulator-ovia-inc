// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ERROR ENVELOPE this service answers in, driven over the wire
//! through the shared harness in [`super::support`].
//!
//! The rule under test: **this service's model carries `awsQueryCompatible`, so every
//! error it answers names its class TWICE — the body's `__type` names the error SHAPE that
//! was thrown, and the `x-amzn-query-error` response header reports that class's LEGACY
//! QUERY CODE and the fault side (`Sender` for a refusal the caller caused, `Receiver` for
//! a failure the service caused) to a client that asked for the report with
//! `x-amzn-query-mode: true`.** The code is the error shape's modelled `awsQueryError`
//! where the model states one and the shape's own error code where the model defaults that
//! binding; a parameter refusal raised by the request-validation framework in front of the
//! service names that framework's shape in the body and the service's own legacy code in
//! the header, which is why the two names cannot be read off each other.
//!
//! The table spans what the rule generalises over rather than one recorded request: the
//! LAYER the refusal comes from (a handler's modelled error, a refusal this service words
//! for a value the boundary walk rejected, and the framework parameter refusals), the
//! source of the CODE (stated by the model's `awsQueryError`, defaulted by the model so the
//! shape's own name is the code, and not modelled at all), the operation, and the STATUS
//! (400 and 403 — both a Sender fault, which is what the fault column pins). Every
//! expected shape id and legacy code is the constant the service itself declares, never a
//! string copied out of an answer.
//!
//! A MODELLED error's `__type` is its FULLY QUALIFIED shape id — `com.amazonaws.sqs#<Shape>`,
//! the id the service's own model declares for the class — which is what a modern client
//! resolves its typed exception from; a parameter refusal raised by the request-validation
//! framework names that framework's own shape id instead, because the class belongs to that
//! layer and not to this service's model.

use serde_json::{json, Value};
use service_com_amazonaws_sqs_amazonsqs::scaffolded::{
    INVALID_PARAMETER_VALUE, INVALID_PARAMETER_VALUE_SHAPE_ID, MISSING_PARAMETER,
    MISSING_PARAMETER_SHAPE_ID,
};
use smithy_server_runtime::MultiServiceRouter;

use super::support::{
    call_in_query_mode, call_with_headers, create_ok, emulator, error_shape, expected_url, ok,
    QUERY_ERROR_HEADER,
};

/// The legacy query code the model states for a queue that does not exist, and for a purge
/// already under way: from
/// `smithy_query(query="shape", shape="QueueDoesNotExist,PurgeQueueInProgress",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`,
/// whose `awsQueryError.code` is what a query-era client knows those classes by. Neither is
/// the shape's own name, which is what makes them the rows that pin "the model's code wins".
const NON_EXISTENT_QUEUE: &str = "AWS.SimpleQueueService.NonExistentQueue";
const PURGE_IN_PROGRESS: &str = "AWS.SimpleQueueService.PurgeQueueInProgress";

/// The namespace this service's model declares its shapes in
/// (`smithy_query(query="service_info",
/// model_path="services/service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`:
/// service shape `com.amazonaws.sqs#AmazonSQS`), which is the package a body names one of its
/// error classes under.
const NAMESPACE: &str = "com.amazonaws.sqs";

/// The shape id a body names the modelled class `local` under.
fn modelled(local: &str) -> String {
    format!("{NAMESPACE}#{local}")
}

/// One row: the case, the operation, the request, the status it owes, the error SHAPE its
/// `__type` names and the LEGACY CODE its header reports.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: Value,
    status: u16,
    shape: String,
    code: String,
}

/// The fault side the report names for `status` — the caller's for a refusal (4xx), the
/// service's for a failure of its own (5xx).
fn fault_for(status: u16) -> &'static str {
    if status >= 500 {
        "Receiver"
    } else {
        "Sender"
    }
}

/// The report `status` and `code` owe: `<code>;<fault>`.
fn expected_report(code: &str, status: u16) -> String {
    format!("{code};{}", fault_for(status))
}

/// The value of one response header, or `None` when the answer carries none.
fn header(headers: &http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToString::to_string)
}

/// Every row, over an emulator already holding the queues they name.
///
/// The queues are made through this service's own `CreateQueue`, and the purge row's queue
/// has already been purged once, so the refusal it drives is the state the service really
/// holds rather than one asserted into place.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let queue = create_ok(router, "envelope-queue", &json!({})).await;
    let purged = create_ok(router, "envelope-purged", &json!({})).await;
    ok(router, "PurgeQueue", json!({"QueueUrl": purged})).await;
    let absent = expected_url("envelope-never-created");

    vec![
        // A handler's refusal of a parameter VALUE: not modelled on the service at all, so
        // the body names the validation framework's shape and the header the legacy code.
        Row {
            case: "a VisibilityTimeout outside the range the parameter documents",
            operation: "ChangeMessageVisibility",
            request: json!({
                "QueueUrl": queue,
                "ReceiptHandle": "any-handle",
                "VisibilityTimeout": -1,
            }),
            status: 400,
            shape: INVALID_PARAMETER_VALUE_SHAPE_ID.to_string(),
            code: INVALID_PARAMETER_VALUE.to_string(),
        },
        Row {
            case: "a MaxNumberOfMessages outside the range the parameter documents",
            operation: "ReceiveMessage",
            request: json!({"QueueUrl": queue, "MaxNumberOfMessages": 11}),
            status: 400,
            shape: INVALID_PARAMETER_VALUE_SHAPE_ID.to_string(),
            code: INVALID_PARAMETER_VALUE.to_string(),
        },
        // A required parameter supplied with nothing in it: the other framework class.
        Row {
            case: "a MessageBody holding no character",
            operation: "SendMessage",
            request: json!({"QueueUrl": queue, "MessageBody": ""}),
            status: 400,
            shape: MISSING_PARAMETER_SHAPE_ID.to_string(),
            code: MISSING_PARAMETER.to_string(),
        },
        // A modelled class whose `awsQueryError` binding the model DEFAULTS: the code is
        // the shape's own name, and the framework's own renderer states none, so the
        // report is what this service completes.
        Row {
            case: "an attribute value outside what the attribute accepts, on a create",
            operation: "CreateQueue",
            request: json!({
                "QueueName": "envelope-bad-attribute",
                "Attributes": {"VisibilityTimeout": "-1"},
            }),
            status: 400,
            shape: modelled("InvalidAttributeValue"),
            code: "InvalidAttributeValue".to_string(),
        },
        Row {
            case: "an attribute value outside what the attribute accepts, on an update",
            operation: "SetQueueAttributes",
            request: json!({"QueueUrl": queue, "Attributes": {"DelaySeconds": "1000000"}}),
            status: 400,
            shape: modelled("InvalidAttributeValue"),
            code: "InvalidAttributeValue".to_string(),
        },
        Row {
            case: "a message body outside the allowed character set",
            operation: "SendMessage",
            request: json!({"QueueUrl": queue, "MessageBody": "ring\u{7}ring"}),
            status: 400,
            shape: modelled("InvalidMessageContents"),
            code: "InvalidMessageContents".to_string(),
        },
        // The BOUNDARY walk's refusal, worded by this service: rendered outside the
        // per-operation sequence, so it is the row that pins the report reaching that
        // layer too.
        Row {
            case: "an attribute name no queue has",
            operation: "GetQueueAttributes",
            request: json!({"QueueUrl": queue, "AttributeNames": ["NotAnAttribute"]}),
            status: 400,
            shape: modelled("InvalidAttributeName"),
            code: "InvalidAttributeName".to_string(),
        },
        // A modelled class whose code the model STATES: the report carries that code, not
        // the shape's name.
        Row {
            case: "a url naming a queue this account does not have",
            operation: "GetQueueAttributes",
            request: json!({"QueueUrl": absent, "AttributeNames": ["All"]}),
            status: 400,
            shape: modelled("QueueDoesNotExist"),
            code: NON_EXISTENT_QUEUE.to_string(),
        },
        // A refusal that is a 4xx but not a 400: still the caller's fault.
        Row {
            case: "a purge of a queue whose purge is still running",
            operation: "PurgeQueue",
            request: json!({"QueueUrl": purged}),
            status: 403,
            shape: modelled("PurgeQueueInProgress"),
            code: PURGE_IN_PROGRESS.to_string(),
        },
    ]
}

/// A client that asked for the legacy report gets it on every refusal, beside the error
/// shape in the body.
#[tokio::test]
async fn every_refusal_reports_its_legacy_query_code_and_fault() {
    let router = emulator();
    for row in rows(&router).await {
        let (status, headers, body) = call_in_query_mode(&router, row.operation, &row.request).await;
        let report = header(&headers, QUERY_ERROR_HEADER);
        assert_eq!(
            (status, error_shape(&body), report.clone()),
            (
                row.status,
                row.shape.clone(),
                Some(expected_report(&row.code, row.status)),
            ),
            "case {:?}: expected HTTP {} with __type {:?} and {QUERY_ERROR_HEADER} {:?}, got \
             {status} with {body} and {report:?}",
            row.case,
            row.status,
            row.shape,
            expected_report(&row.code, row.status),
        );
    }
}

/// A native JSON client asks for no legacy report and is sent none — and the error shape
/// its SDK resolves the typed exception from is the same one either way.
#[tokio::test]
async fn a_client_that_asked_for_no_report_is_sent_none() {
    let router = emulator();
    for row in rows(&router).await {
        let (status, headers, body) = call_with_headers(&router, row.operation, &row.request).await;
        let report = header(&headers, QUERY_ERROR_HEADER);
        assert_eq!(
            (status, error_shape(&body), report.clone()),
            (row.status, row.shape.clone(), None),
            "case {:?}: expected HTTP {} with __type {:?} and no {QUERY_ERROR_HEADER}, got \
             {status} with {body} and {report:?}",
            row.case,
            row.status,
            row.shape,
        );
    }
}

/// An ACCEPTED call names no error class, so it carries no report — even for the client
/// that asked for one.
#[tokio::test]
async fn an_accepted_call_reports_no_error_class() {
    let router = emulator();
    let url = create_ok(&router, "envelope-accepted", &json!({})).await;
    for (case, operation, request) in [
        ("a create", "CreateQueue", json!({"QueueName": "envelope-created"})),
        (
            "a send",
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": "hello"}),
        ),
        (
            "a read",
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["All"]}),
        ),
    ] {
        let (status, headers, body) = call_in_query_mode(&router, operation, &request).await;
        let report = header(&headers, QUERY_ERROR_HEADER);
        assert_eq!(
            (status, report.clone()),
            (200, None),
            "case {case:?}: expected HTTP 200 with no {QUERY_ERROR_HEADER}, got {status} with \
             {body} and {report:?}"
        );
    }
}
