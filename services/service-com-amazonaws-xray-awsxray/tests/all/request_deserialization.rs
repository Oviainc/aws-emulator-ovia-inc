// SPDX-License-Identifier: CC0-1.0

//! The rule the request-DESERIALIZATION layer follows, over the wire.
//!
//! A body value whose JSON type cannot be read into the modelled type of the
//! member it is bound to never becomes a value at all, so it is refused at the
//! deserialization layer with the protocol-level `SerializationException` (HTTP
//! 400) whose message states the CLASS of value sent and the modelled type it
//! could not become — NOT with `ValidationException`, which is the answer to a
//! value that WAS read and then broke a rule the model declares (an absent
//! `@required` member, a `@length`/`@range`/`@pattern`/enum violation).
//!
//! The table below spans the class along what the rule generalises over: the
//! JSON type sent (string, boolean, number, object), the modelled type it is
//! bound to (`integer`, `list`, `timestamp`), the operation carrying it, and how
//! deep in the request it sits (a top-level member, a member of a structure
//! inside a list). The rows after it are the OTHER side of the rule: a refusal
//! that is not about a value's type keeps the error class it already had.

use serde_json::{json, Value};

use crate::xray_wire::{Answer, Xray};

/// One request whose body cannot be read, and the refusal it earns.
#[derive(Debug)]
struct Unreadable {
    /// What this row varies.
    case: &'static str,
    /// The operation's `@http` URI.
    path: &'static str,
    body: Value,
    /// The whole message, when the failing read is handed the schema of the
    /// member that could not be read.
    ///
    /// `None` where the modelled type the read is handed is the CONTAINER's: the
    /// generated codec passes a list member's own schema down to its elements
    /// (`vendor/smithy-macros/src/codegen.rs`, `emit_read`'s `List` arm — "the
    /// deserializer doesn't actually need a member schema for the element when
    /// the codec is JSON"), so an unreadable element names the list it is in.
    /// The error CODE, which is the requirement, is asserted for every row.
    message: Option<&'static str>,
}

/// The modelled types these rows reach, from the model
/// (`smithy_query(query="shape", shape="TelemetryRecord,TraceIdList,TraceSegmentDocumentList")`):
/// `TelemetryRecord$SegmentsReceivedCount` and its three sibling counters are
/// `integer`, `TelemetryRecord$Timestamp` is a `timestamp`, `TraceIds` targets
/// the `TraceIdList` list, and `TraceSegmentDocuments` the
/// `TraceSegmentDocumentList` list of strings.
fn unreadable_rows() -> Vec<Unreadable> {
    vec![
        Unreadable {
            case: "a string where a nested integer member is modelled",
            path: "/TelemetryRecords",
            body: json!({
                "TelemetryRecords": [{
                    "Timestamp": 1_700_000_000.0,
                    "SegmentsReceivedCount": "17",
                }],
            }),
            message: Some("STRING_VALUE can not be converted to an Integer"),
        },
        Unreadable {
            case: "a boolean where a nested integer member is modelled",
            path: "/TelemetryRecords",
            body: json!({
                "TelemetryRecords": [{
                    "Timestamp": 1_700_000_000.0,
                    "SegmentsSentCount": true,
                }],
            }),
            message: Some("BOOLEAN_VALUE can not be converted to an Integer"),
        },
        Unreadable {
            case: "a number past the modelled integer's width",
            path: "/TelemetryRecords",
            body: json!({
                "TelemetryRecords": [{
                    "Timestamp": 1_700_000_000.0,
                    // One past i32::MAX, which an `integer` cannot hold.
                    "SegmentsSpilloverCount": 2_147_483_648_i64,
                }],
            }),
            message: Some("NUMBER_VALUE can not be converted to an Integer"),
        },
        Unreadable {
            case: "an object where a nested integer member is modelled",
            path: "/TelemetryRecords",
            body: json!({
                "TelemetryRecords": [{
                    "Timestamp": 1_700_000_000.0,
                    "SegmentsRejectedCount": {"Count": 3},
                }],
            }),
            message: Some("OBJECT_VALUE can not be converted to an Integer"),
        },
        Unreadable {
            case: "a boolean where a nested timestamp member is modelled",
            path: "/TelemetryRecords",
            body: json!({ "TelemetryRecords": [{ "Timestamp": true }] }),
            message: Some("BOOLEAN_VALUE can not be converted to a Timestamp"),
        },
        Unreadable {
            case: "a string where a top-level list member is modelled",
            path: "/Traces",
            body: json!({ "TraceIds": "1-5759e988-bd862e3fe1be46a994272793" }),
            message: Some("STRING_VALUE can not be converted to a List"),
        },
        Unreadable {
            case: "an object where a top-level list member is modelled",
            path: "/Traces",
            body: json!({ "TraceIds": {"0": "1-5759e988-bd862e3fe1be46a994272793"} }),
            message: Some("OBJECT_VALUE can not be converted to a List"),
        },
        Unreadable {
            case: "a number where a list's string element is modelled",
            path: "/TraceSegments",
            body: json!({ "TraceSegmentDocuments": [7] }),
            message: None,
        },
        Unreadable {
            case: "a boolean where a list's string element is modelled",
            path: "/TraceSegments",
            body: json!({ "TraceSegmentDocuments": [false] }),
            message: None,
        },
        Unreadable {
            case: "a number where a top-level string member is modelled",
            path: "/CreateGroup",
            body: json!({ "GroupName": 12 }),
            message: Some("NUMBER_VALUE can not be converted to a String"),
        },
        Unreadable {
            case: "a string where a top-level boolean member is modelled",
            path: "/TraceSummaries",
            body: json!({ "StartTime": 1_000.0, "EndTime": 2_000.0, "Sampling": "yes" }),
            message: Some("STRING_VALUE can not be converted to a Boolean"),
        },
    ]
}

#[tokio::test]
async fn a_body_value_the_modelled_type_cannot_be_read_from_is_a_serialization_failure() {
    let xray = Xray::new();

    for row in unreadable_rows() {
        let answer = xray.call(row.path, &row.body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("SerializationException")),
            "case {:?}: a body value the modelled type cannot be read from must be refused \
             400 SerializationException, got {answer:?}",
            row.case
        );
        let stated = answer.message().unwrap_or_default();
        match row.message {
            Some(expected) => assert_eq!(
                stated, expected,
                "case {:?}: expected the message {expected:?}, got {stated:?}",
                row.case
            ),
            // The class of value sent is stated even where the type named is the
            // container's: it is the fact the caller has to act on.
            None => assert!(
                stated.contains("_VALUE can not be converted to "),
                "case {:?}: expected the refusal to name the class of value sent, got {stated:?}",
                row.case
            ),
        }
    }
}

/// A refusal that is NOT about a value's JSON type, and the error class it keeps.
///
/// Each row's value READS cleanly (or is simply absent) and is then refused by a
/// rule the model declares, so it stays the request layer's `ValidationException`
/// — the separation this change is about, asserted from the other side.
fn still_validation_rows() -> Vec<(&'static str, &'static str, Value)> {
    vec![
        (
            "an absent nested @required member",
            "/TelemetryRecords",
            json!({ "TelemetryRecords": [{ "SegmentsReceivedCount": 1 }] }),
        ),
        (
            "an absent top-level @required member",
            "/TelemetryRecords",
            json!({}),
        ),
        (
            "a string below a modelled @length minimum",
            "/CreateGroup",
            json!({ "GroupName": "" }),
        ),
        (
            "a string above a modelled @length maximum",
            "/CreateGroup",
            json!({ "GroupName": "g".repeat(33) }),
        ),
        // Text that is a string but not a time is NOT one of these rows: the
        // value is of the member's JSON form and what is wrong with it is what
        // it MEANS, which this service refuses with its own modelled error
        // naming the value (see `tests/all/unconvertible_time_member.rs`).
    ]
}

#[tokio::test]
async fn a_refusal_that_is_not_about_a_json_type_keeps_its_own_error_class() {
    let xray = Xray::new();

    for (case, path, body) in still_validation_rows() {
        let answer = xray.call(path, &body).await;
        assert_eq!(
            (answer.status, answer.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a refusal that is not about a value's JSON type must keep \
             400 ValidationException, got {answer:?}"
        );
    }
}

/// And a body the codec CAN read is untouched by the layer: it reaches the
/// handler and is answered by it.
#[tokio::test]
async fn a_readable_body_still_reaches_its_handler() {
    let xray = Xray::new();

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "every nested counter sent as the modelled integer",
            "/TelemetryRecords",
            json!({
                "TelemetryRecords": [{
                    "Timestamp": 1_700_000_000.0,
                    "SegmentsReceivedCount": 1,
                    "SegmentsSentCount": 2,
                    "SegmentsSpilloverCount": 3,
                    "SegmentsRejectedCount": 4,
                    "BackendConnectionErrors": { "TimeoutCount": 5 },
                }],
                "EC2InstanceId": "i-0123456789abcdef0",
            }),
        ),
        (
            "an empty list where a list is modelled",
            "/Traces",
            json!({ "TraceIds": [] }),
        ),
        (
            "a timestamp sent as the epoch seconds the protocol declares",
            "/TelemetryRecords",
            json!({ "TelemetryRecords": [{ "Timestamp": 1_700_000_001.5 }] }),
        ),
    ];

    for (case, path, body) in rows {
        let answer: Answer = xray.call(path, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a body the codec can read must reach its handler, got {answer:?}"
        );
    }
}
