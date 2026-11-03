// SPDX-License-Identifier: CC0-1.0

//! The JSON class a modelled TIMESTAMP member of a request is read from.
//!
//! An `awsJson1_1` timestamp that declares no `@timestampFormat` travels as `epoch-seconds`
//! — a JSON NUMBER — so a value of any other class names no instant and the request is
//! refused at the deserialization layer, with `400 SerializationException`, before the
//! operation runs and before anything is mutated.
//!
//! Asserted over the class of the value (string, boolean, array, object) and over every
//! operation of this model whose input binds a timestamp
//! (`smithy_query(query="operation",operation="DeleteApplication,DeleteApplicationSnapshot")`),
//! not over the one request a recording happens to carry.

use serde_json::{json, Value};

use crate::harness::{call, create, error_code, error_message, router};

/// The sentence AWS refuses a timestamp of the wrong class with: the class the caller sent,
/// then the form it could not be read into.
fn unreadable(class: &str) -> String {
    format!("{class} can not be converted to milliseconds since epoch")
}

/// A value of each JSON class a timestamp member cannot be read from, paired with the class
/// name the refusal states. `epoch` is the instant the request is really about, so the
/// string row is the SAME instant merely quoted — the value a client's serializer gets
/// wrong, which is readable only if the layer is lenient.
fn unreadable_values(epoch: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("STRING_VALUE", json!(epoch.to_string())),
        ("STRING_VALUE", json!("2026-01-01T00:00:00Z")),
        ("BOOLEAN_VALUE", json!(true)),
        ("ARRAY_VALUE", json!([epoch])),
        ("OBJECT_VALUE", json!({"seconds": epoch})),
    ]
}

/// Every operation whose input binds a timestamp refuses a value of any other JSON class
/// with `400 SerializationException`, and serves the number form.
#[tokio::test]
async fn a_timestamp_member_is_read_only_from_a_json_number() {
    let router = router();
    let created = create(&router, "timestamp-classes", "FLINK-1_20").await;
    let epoch = created["CreateTimestamp"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected a stored create timestamp, got {created}"));

    // (operation, the member that binds the timestamp, the rest of the request)
    let cases: [(&str, &str, Value); 2] = [
        (
            "DeleteApplication",
            "CreateTimestamp",
            json!({"ApplicationName": "timestamp-classes"}),
        ),
        (
            "DeleteApplicationSnapshot",
            "SnapshotCreationTimestamp",
            json!({"ApplicationName": "timestamp-classes", "SnapshotName": "snap"}),
        ),
    ];

    for (operation, member, request) in cases {
        for (class, value) in unreadable_values(epoch) {
            let mut body = request.clone();
            body[member] = value.clone();
            let (status, refused) = call(&router, operation, &body).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, "SerializationException"),
                "case {operation}/{value}: expected 400 SerializationException, got \
                 {status} {refused}"
            );
            assert_eq!(
                error_message(&refused),
                unreadable(class),
                "case {operation}/{value}: expected the class and the form it could not \
                 be read into, got {refused}"
            );
        }

        // The number form is read, so the request reaches the operation and is answered by
        // it — whatever that answer is — rather than by this layer.
        let mut body = request.clone();
        body[member] = json!(epoch);
        let (status, answered) = call(&router, operation, &body).await;
        assert_ne!(
            error_code(&answered),
            "SerializationException",
            "case {operation}/number: the epoch-seconds form is readable, got \
             {status} {answered}"
        );
    }
}

/// A request refused at the deserialization layer mutates NOTHING: the application the
/// refused `DeleteApplication` named is still there to be described afterwards, and still
/// deletable with the timestamp in the form the protocol carries.
#[tokio::test]
async fn a_refused_timestamp_deletes_nothing() {
    let router = router();
    let created = create(&router, "survives-refusal", "FLINK-1_20").await;
    let epoch = created["CreateTimestamp"]
        .as_i64()
        .unwrap_or_else(|| panic!("expected a stored create timestamp, got {created}"));

    for (case, value) in [
        ("quoted-epoch", json!(epoch.to_string())),
        ("not-a-timestamp", json!("yesterday")),
    ] {
        let (status, refused) = call(
            &router,
            "DeleteApplication",
            &json!({"ApplicationName": "survives-refusal", "CreateTimestamp": value}),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "SerializationException"),
            "case {case}: expected 400 SerializationException, got {status} {refused}"
        );

        let (status, described) = call(
            &router,
            "DescribeApplication",
            &json!({"ApplicationName": "survives-refusal"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: the refused delete must leave the application stored, got \
             {status} {described}"
        );
    }

    let (status, deleted) = call(
        &router,
        "DeleteApplication",
        &json!({"ApplicationName": "survives-refusal", "CreateTimestamp": epoch}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case readable: the epoch-seconds form deletes the application, got \
         {status} {deleted}"
    );
}
