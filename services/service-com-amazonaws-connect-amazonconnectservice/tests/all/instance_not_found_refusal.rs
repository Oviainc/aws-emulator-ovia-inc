// SPDX-License-Identifier: CC0-1.0

//! The refusal Connect gives for an `InstanceId` that names no instance, driven over
//! HTTP through the same `MultiServiceRouter` the emulator serves with.
//!
//! RULE: when a request carries an `InstanceId` that resolves to no instance — one that
//! never existed, or one `DeleteInstance` retired — the service answers
//! `ResourceNotFoundException` (HTTP 404, from the model's error binding) and its
//! `Message` is the sentence `Instance <InstanceId> not found`, whose only variable part
//! is the identifier, taken verbatim from the request.
//!
//! The suites below assert that rule over a TABLE: every operation of this service that
//! has to resolve an `InstanceId` crossed with identifiers of several forms and lengths,
//! including both ends of the modelled `InstanceId` length bound and an identifier whose
//! instance really existed until it was deleted. The last suite pins the other side of
//! the rule: when the instance IS there and the resource inside it is not, the refusal
//! names that resource instead.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_connect_amazonconnectservice::state::INSTANCE_ACTIVATION_DELAY;

use super::harness::{connect_managed, Connect};

/// A flow content of the documented form — the JSON string of a flow definition — so a
/// `CreateContactFlow` row reaches the instance lookup rather than content validation.
const FLOW_CONTENT: &str =
    r#"{"Version":"2019-10-30","StartAction":"a1","Actions":[{"Identifier":"a1","Type":"DisconnectParticipant","Parameters":{}}]}"#;

/// An identifier of the right modelled form that names no resource inside an instance.
/// A row that sends one only ever reaches the instance lookup, which is what the rule is
/// about; the suite that needs the child lookup itself sends it against a live instance.
const ABSENT_CHILD: &str = "11111111-2222-3333-4444-555555555555";

/// One request that has to resolve an `InstanceId`: the operation's name, and the HTTP
/// method, URI and body it is sent as. The method/URI pairs are the `http` bindings of
/// this service's model (smithy_query(query="operation",operation="<Name>")).
struct Call {
    operation: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
}

/// Every operation this emulator implements that resolves an `InstanceId`, as a request
/// against `instance_id`. Each body is a valid one for its operation, so the instance
/// lookup is what the request is refused on.
fn calls_resolving_an_instance(instance_id: &str) -> Vec<Call> {
    let call = |operation, method, uri: String, body| Call {
        operation,
        method,
        uri,
        body,
    };
    vec![
        call(
            "DescribeInstance",
            "GET",
            format!("/instance/{instance_id}"),
            None,
        ),
        call(
            "DeleteInstance",
            "DELETE",
            format!("/instance/{instance_id}"),
            None,
        ),
        call(
            "ListQueues",
            "GET",
            format!("/queues-summary/{instance_id}"),
            None,
        ),
        call(
            "CreateQueue",
            "PUT",
            format!("/queues/{instance_id}"),
            Some(json!({ "Name": "support", "HoursOfOperationId": ABSENT_CHILD })),
        ),
        call(
            "DescribeQueue",
            "GET",
            format!("/queues/{instance_id}/{ABSENT_CHILD}"),
            None,
        ),
        call(
            "DeleteQueue",
            "DELETE",
            format!("/queues/{instance_id}/{ABSENT_CHILD}"),
            None,
        ),
        call(
            "ListContactFlows",
            "GET",
            format!("/contact-flows-summary/{instance_id}"),
            None,
        ),
        call(
            "CreateContactFlow",
            "PUT",
            format!("/contact-flows/{instance_id}"),
            Some(json!({ "Name": "inbound", "Type": "CONTACT_FLOW", "Content": FLOW_CONTENT })),
        ),
        call(
            "DescribeContactFlow",
            "GET",
            format!("/contact-flows/{instance_id}/{ABSENT_CHILD}"),
            None,
        ),
        call(
            "ListHoursOfOperations",
            "GET",
            format!("/hours-of-operations-summary/{instance_id}"),
            None,
        ),
        call(
            "CreateHoursOfOperation",
            "PUT",
            format!("/hours-of-operations/{instance_id}"),
            Some(json!({
                "Name": "open",
                "TimeZone": "UTC",
                "Config": [{
                    "Day": "MONDAY",
                    "StartTime": { "Hours": 9, "Minutes": 0 },
                    "EndTime": { "Hours": 17, "Minutes": 30 },
                }],
            })),
        ),
        call(
            "DescribeHoursOfOperation",
            "GET",
            format!("/hours-of-operations/{instance_id}/{ABSENT_CHILD}"),
            None,
        ),
        call(
            "AssociateInstanceStorageConfig",
            "PUT",
            format!("/instance/{instance_id}/storage-config"),
            Some(json!({
                "ResourceType": "CALL_RECORDINGS",
                "StorageConfig": {
                    "StorageType": "S3",
                    "S3Config": { "BucketName": "connect-recordings", "BucketPrefix": "calls" },
                },
            })),
        ),
        call(
            "DescribeInstanceStorageConfig",
            "GET",
            format!(
                "/instance/{instance_id}/storage-config/{ABSENT_CHILD}?resourceType=CALL_RECORDINGS"
            ),
            None,
        ),
    ]
}

/// The `Message` member of an answered error — the member name the model gives it —
/// so a row's assertion reports the text rather than the whole envelope.
fn message_of(body: &Value) -> String {
    body["Message"].as_str().unwrap_or_default().to_string()
}

/// Drive every instance-resolving call against `instance_id` and assert the rule on each
/// answer: 404, `ResourceNotFoundException`, and AWS's sentence carrying that identifier.
async fn assert_every_call_reports_the_instance_missing(
    connect: &Connect,
    case: &str,
    instance_id: &str,
) {
    let expected = format!("Instance {instance_id} not found");
    for call in calls_resolving_an_instance(instance_id) {
        let answer = connect
            .send_answer(call.method, &call.uri, call.body.clone())
            .await;
        let actual = message_of(&answer.body);
        assert_eq!(
            answer.status, 404,
            "case {case:?}/{}: expected HTTP 404, got {} {}",
            call.operation, answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}/{}: expected error code ResourceNotFoundException, got {:?}",
            call.operation, answer.error_code
        );
        assert_eq!(
            actual, expected,
            "case {case:?}/{}: expected message {expected:?}, got {actual:?}",
            call.operation
        );
    }
}

/// RULE: an `InstanceId` that names no instance is refused with
/// `ResourceNotFoundException` (404) whose message is `Instance <InstanceId> not found`,
/// for every operation that resolves an `InstanceId`.
///
/// The table spans the identifier's form and length: the UUID shape Connect mints, both
/// ends of the modelled `InstanceId` length bound (1..100), a mixed-case identifier, one
/// that is a prefix of a UUID, and one carrying the characters an identifier may hold —
/// crossed with all fourteen instance-resolving operations.
#[tokio::test]
async fn an_unknown_instance_id_is_refused_in_awss_sentence() {
    let connect = Connect::new();
    // A live instance stands beside the absent ones, so a row cannot pass merely because
    // the store is empty.
    let (live_id, _) = connect.create_instance(connect_managed("neighbour")).await;

    let longest = "i".repeat(99) + "z"; // InstanceId length bound: 1..100.
    let cases: Vec<(&str, String)> = vec![
        (
            "the UUID shape Connect mints",
            "7f3c9b21-0d4e-4a86-9c15-2b6f8e5d1a30".to_string(),
        ),
        ("one character, the length lower bound", "a".to_string()),
        ("100 characters, the length upper bound", longest),
        ("mixed case", "AbCdEf-0123".to_string()),
        (
            "a prefix of a UUID-shaped identifier",
            "7f3c9b21".to_string(),
        ),
        (
            "a prefix of the live instance's identifier",
            live_id[..live_id.len() - 1].to_string(),
        ),
        ("digits only", "9876543210".to_string()),
    ];

    for (case, instance_id) in cases {
        assert_ne!(
            instance_id, live_id,
            "case {case:?}: the row has to name an instance the store does not hold"
        );
        assert_every_call_reports_the_instance_missing(&connect, case, &instance_id).await;
    }
}

/// RULE (the same one, on an identifier that really named an instance): deleting an
/// instance retires its identifier, so every later call carrying it is refused with
/// `Instance <InstanceId> not found` — the identifier minted for that instance, not one
/// the test wrote.
#[tokio::test]
async fn a_deleted_instances_id_is_refused_in_awss_sentence() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("retired")).await;

    // The instance really exists first: it reaches ACTIVE once the activation span the
    // store records has elapsed.
    tokio::time::sleep(INSTANCE_ACTIVATION_DELAY + Duration::from_millis(100)).await;
    let described = connect
        .send_answer("GET", &format!("/instance/{instance_id}"), None)
        .await;
    assert_eq!(
        described.body["Instance"]["InstanceStatus"],
        json!("ACTIVE"),
        "the instance has to be ACTIVE before it is deleted, got {}",
        described.body
    );

    let deleted = connect
        .send_answer("DELETE", &format!("/instance/{instance_id}"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "DeleteInstance of a live instance: got {} {}",
        deleted.status, deleted.body
    );

    assert_every_call_reports_the_instance_missing(
        &connect,
        "a deleted instance's identifier",
        &instance_id,
    )
    .await;

    // The identifier stays retired: the same refusal answers a second read.
    assert_every_call_reports_the_instance_missing(
        &connect,
        "a deleted instance's identifier, read twice",
        &instance_id,
    )
    .await;
}

/// RULE (the boundary of the one above): the instance-missing sentence is reserved for a
/// missing INSTANCE. When the instance is there and the resource inside it is not, the
/// refusal is still `ResourceNotFoundException` (404) but names that resource.
#[tokio::test]
async fn a_present_instance_with_an_absent_child_names_the_child() {
    let connect = Connect::new();
    let (instance_id, _) = connect.create_instance(connect_managed("children")).await;
    let instance_sentence = format!("Instance {instance_id} not found");

    let cases: Vec<(&str, &str, String)> = vec![
        (
            "DescribeQueue",
            "GET",
            format!("/queues/{instance_id}/{ABSENT_CHILD}"),
        ),
        (
            "DeleteQueue",
            "DELETE",
            format!("/queues/{instance_id}/{ABSENT_CHILD}"),
        ),
        (
            "DescribeContactFlow",
            "GET",
            format!("/contact-flows/{instance_id}/{ABSENT_CHILD}"),
        ),
        (
            "DescribeHoursOfOperation",
            "GET",
            format!("/hours-of-operations/{instance_id}/{ABSENT_CHILD}"),
        ),
        (
            "DescribeInstanceStorageConfig",
            "GET",
            format!(
                "/instance/{instance_id}/storage-config/{ABSENT_CHILD}?resourceType=CALL_RECORDINGS"
            ),
        ),
    ];

    for (case, method, uri) in cases {
        let answer = connect.send_answer(method, &uri, None).await;
        let actual = message_of(&answer.body);
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code, "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {:?}",
            answer.error_code
        );
        assert_ne!(
            actual, instance_sentence,
            "case {case:?}: the instance is present, so its message must name the \
             resource inside it, got {actual:?}"
        );
        assert!(
            actual.contains(ABSENT_CHILD),
            "case {case:?}: expected the message to name {ABSENT_CHILD}, got {actual:?}"
        );
    }
}
