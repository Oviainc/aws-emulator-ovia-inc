// SPDX-License-Identifier: CC0-1.0

//! `CreateTable` honours `StreamSpecification` WHATEVER ELSE the request carries.
//!
//! The rule pinned here is the one the refusal this suite was written for broke: an
//! operation must honour every modelled input member whose effect it can persist, and
//! may refuse a request only with an error its model declares. A `CreateTable` that
//! named a `StreamSpecification` used to be turned away whole — no table, and an error
//! code (`NotImplemented`) that `CreateTable` does not model, so no SDK branch could
//! classify it. The member has to be honoured independently of the REST of the
//! request: it is not conditional on the billing mode, on there being indexes, on
//! tags, on the table class or on deletion protection.
//!
//! So this suite varies the OTHER members present beside the specification — which is
//! the axis `table_streams.rs` does not vary (that suite varies the view type and the
//! key schema) — and asserts for every row that the create is accepted, that the
//! stream configuration is stored and reported back by a later `DescribeTable`, that
//! the stream is resolvable through the DynamoDB STREAMS surface, and that the other
//! member of the row is honoured too, so honouring one is not at the price of the
//! other. No row may answer an error code `CreateTable` does not model.
//!
//! Everything is driven over the wire against both services on one engine, so nothing
//! is asserted that a client could not observe.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;

use crate::emulator::{call, call_streams, emulator_with_streams, error_code};

/// One row: a create that enables a stream and also carries `extra`, plus what that
/// `extra` must still produce in the stored table's description.
struct Row {
    case: &'static str,
    /// The members this row adds to the base create body.
    extra: fn() -> Value,
    /// Asserted against `TableDescription` once the table exists, so the row proves
    /// the other member was honoured as well as the stream.
    check: fn(&Value) -> Option<String>,
}

/// The rows span the member classes a create can carry beside a stream: both billing
/// modes, a global and a local secondary index, a tag set, the table class, and
/// deletion protection. Each `check` names what that member must leave in the stored
/// description.
const ROWS: &[Row] = &[
    Row {
        case: "on-demand billing",
        extra: || json!({"BillingMode": "PAY_PER_REQUEST"}),
        check: |description| {
            let reported = &description["BillingModeSummary"]["BillingMode"];
            (*reported != json!("PAY_PER_REQUEST"))
                .then(|| format!("expected BillingMode PAY_PER_REQUEST, got {reported}"))
        },
    },
    Row {
        case: "provisioned billing",
        extra: || {
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 4},
            })
        },
        check: |description| {
            let reported = &description["ProvisionedThroughput"]["WriteCapacityUnits"];
            (*reported != json!(4))
                .then(|| format!("expected the provisioned write capacity 4, got {reported}"))
        },
    },
    Row {
        case: "a global secondary index",
        extra: || {
            json!({
                "BillingMode": "PAY_PER_REQUEST",
                // The index keys on an attribute of its own, so the create declares
                // it: a definition no key schema uses is refused in its own right.
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Seq", "AttributeType": "N"},
                    {"AttributeName": "Owner", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexes": [{
                    "IndexName": "by-owner",
                    "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                }],
            })
        },
        check: |description| {
            let names: Vec<Value> = description["GlobalSecondaryIndexes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|index| index["IndexName"].clone())
                .collect();
            (!names.contains(&json!("by-owner")))
                .then(|| format!("expected the index by-owner, got {names:?}"))
        },
    },
    Row {
        case: "a local secondary index",
        extra: || {
            json!({
                "BillingMode": "PAY_PER_REQUEST",
                "LocalSecondaryIndexes": [{
                    "IndexName": "by-seq",
                    "KeySchema": [
                        {"AttributeName": "Part", "KeyType": "HASH"},
                        {"AttributeName": "Seq", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }],
            })
        },
        check: |description| {
            let names: Vec<Value> = description["LocalSecondaryIndexes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|index| index["IndexName"].clone())
                .collect();
            (!names.contains(&json!("by-seq")))
                .then(|| format!("expected the local index by-seq, got {names:?}"))
        },
    },
    Row {
        case: "a tag set",
        extra: || {
            json!({
                "BillingMode": "PAY_PER_REQUEST",
                "Tags": [{"Key": "team", "Value": "ledger"}],
            })
        },
        // Tags are read through the eventually consistent tagging surface, not the
        // table description, so this row asserts only that the create was accepted
        // whole — the stream assertions below are the rule under test.
        check: |_| None,
    },
    Row {
        case: "the infrequent-access table class",
        extra: || {
            json!({
                "BillingMode": "PAY_PER_REQUEST",
                "TableClass": "STANDARD_INFREQUENT_ACCESS",
            })
        },
        check: |description| {
            let reported = &description["TableClassSummary"]["TableClass"];
            (*reported != json!("STANDARD_INFREQUENT_ACCESS"))
                .then(|| format!("expected the table class echoed back, got {reported}"))
        },
    },
    Row {
        case: "deletion protection",
        extra: || {
            json!({
                "BillingMode": "PAY_PER_REQUEST",
                "DeletionProtectionEnabled": true,
            })
        },
        check: |description| {
            let reported = &description["DeletionProtectionEnabled"];
            (*reported != json!(true))
                .then(|| format!("expected deletion protection enabled, got {reported}"))
        },
    },
];

/// The base create body every row extends: a two-attribute key schema (so the local
/// index row has a sort key to use) plus the stream `specification`. A row that keys
/// an index on an attribute of its own replaces `AttributeDefinitions`, because a
/// definition no key schema uses is refused on its own.
fn create_body(name: &str, view_type: &str, extra: Value) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [
            {"AttributeName": "Part", "KeyType": "HASH"},
            {"AttributeName": "Seq", "KeyType": "RANGE"},
        ],
        "AttributeDefinitions": [
            {"AttributeName": "Part", "AttributeType": "S"},
            {"AttributeName": "Seq", "AttributeType": "N"},
        ],
        "StreamSpecification": {"StreamEnabled": true, "StreamViewType": view_type},
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

#[tokio::test]
async fn a_stream_is_honoured_whatever_else_the_create_carries() {
    for (position, row) in ROWS.iter().enumerate() {
        // The view type cycles through the model's enum, so no row depends on one
        // value of a member the rule is indifferent to.
        let view_type = STREAM_VIEW_TYPE_VALUES[position % STREAM_VIEW_TYPE_VALUES.len()];
        let case = format!("{} with StreamViewType {view_type}", row.case);
        let router = emulator_with_streams();
        let name = format!("combined-{position}");

        let (status, created) = call(
            &router,
            "CreateTable",
            create_body(&name, view_type, (row.extra)()),
        )
        .await;
        // The refusal this suite exists for: a member the handler can store must not
        // turn the request away, least of all with a code the operation does not model.
        assert_ne!(
            error_code(&created),
            "NotImplemented",
            "case {case:?}: CreateTable must not refuse a modelled member it stores, got {created}"
        );
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTable to answer HTTP 200, got {status} with {created}"
        );

        let description = &created["TableDescription"];
        assert_eq!(
            description["StreamSpecification"],
            json!({"StreamEnabled": true, "StreamViewType": view_type}),
            "case {case:?}: expected the stream specification echoed back, got {created}"
        );

        // Off the STORED table, not the create's echo: the configuration and the
        // stream's identity are persisted.
        let (status, described) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeTable to answer HTTP 200, got {status} with \
             {described}"
        );
        let stored = &described["Table"];
        for member in ["StreamSpecification", "LatestStreamArn", "LatestStreamLabel"] {
            assert_eq!(
                stored[member], description[member],
                "case {case:?}: expected DescribeTable to report the same {member}, got {described}"
            );
        }
        let stream_arn = stored["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let stream_label = stored["LatestStreamLabel"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stream_label.is_empty() && stream_arn.ends_with(&format!("/stream/{stream_label}")),
            "case {case:?}: expected the stream ARN to end in the stream's label, got \
             {stream_arn:?} and {stream_label:?}"
        );

        // The stream itself is the streams service's resource, and it is there.
        let (status, stream) =
            call_streams(&router, "DescribeStream", json!({"StreamArn": stream_arn})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStream to resolve the table's stream, got {status} \
             with {stream}"
        );
        let stream_description = &stream["StreamDescription"];
        assert_eq!(
            stream_description["StreamViewType"],
            json!(view_type),
            "case {case:?}: expected the stream to capture the requested view type, got {stream}"
        );
        assert_eq!(
            stream_description["TableName"],
            json!(name),
            "case {case:?}: expected the stream to name its own table, got {stream}"
        );

        // And the member the row carried beside the stream is honoured as well.
        if let Some(complaint) = (row.check)(stored) {
            panic!("case {case:?}: {complaint}, got {described}");
        }
    }
}

#[tokio::test]
async fn a_create_refused_for_another_reason_registers_no_stream() {
    // The member is honoured, which does not make it the last word: a create that is
    // refused for a reason of its own leaves no table and no stream, whatever its
    // StreamSpecification asked for. The rows are two refusals the model declares —
    // a name already taken (ResourceInUseException) and a key schema naming an
    // attribute with no definition (ValidationException).
    let rows: [(&str, Value, &str); 2] = [
        (
            "a table name already taken",
            create_body("taken", "NEW_IMAGE", json!({"BillingMode": "PAY_PER_REQUEST"})),
            "ResourceInUseException",
        ),
        (
            "a key schema with no attribute definition",
            json!({
                "TableName": "undefined-key",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [],
                "BillingMode": "PAY_PER_REQUEST",
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
            }),
            "ValidationException",
        ),
    ];

    for (case, body, expected_code) in rows {
        let router = emulator_with_streams();
        let table_name = body["TableName"].as_str().unwrap_or_default().to_string();
        if case == "a table name already taken" {
            let (status, first) = call(&router, "CreateTable", body.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the first create must succeed, got {status} with {first}"
            );
        }

        let (status, refused) = call(&router, "CreateTable", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the create refused with HTTP 400, got {status} with {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );

        // Exactly the streams of the tables that were created: one for the row whose
        // first create succeeded, none for the row where nothing was created.
        let (status, listed) = call_streams(&router, "ListStreams", json!({})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListStreams to answer HTTP 200, got {status} with {listed}"
        );
        let streams: Vec<String> = listed["Streams"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry["TableName"].as_str().map(str::to_string))
            .collect();
        let expected: Vec<String> = if case == "a table name already taken" {
            vec![table_name]
        } else {
            Vec::new()
        };
        assert_eq!(
            streams, expected,
            "case {case:?}: expected the refused create to register no stream, got {listed}"
        );
    }
}
