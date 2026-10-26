// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteDeliveryDestinationPolicy`: taking the delivery destination
//! policy off one of the account's delivery destinations.
//!
//! The rule under test: the policy a delivery destination holds belongs to THAT destination,
//! so deleting it leaves the destination it names holding no policy at all — which is what
//! every later read reports — while every other destination keeps the policy it holds and the
//! named destination keeps every other parameter it was created with; a destination holding no
//! policy is already in that state, so the deletion is accepted and changes nothing however
//! often it is repeated, and a later assignment takes effect on the same destination; and a
//! `deliveryDestinationName` the account holds no delivery destination under names no resource
//! to delete a policy from, so it is refused as a missing resource and nothing is persisted.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: destinations of
//! several `DeliveryDestinationType`s, names at both published bounds of
//! `DeliveryDestinationName`, policy documents of several forms and character sets, and
//! destinations holding no policy at all — all in one store, so a delete that reaches past the
//! destination it names cannot pass.
//!
//! `tests/all/delivery_destination_policies.rs` is the assignment side of the same state and
//! `tests/all/delivery_destination_policy_reads.rs` the reporting side.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operation under test, plus the operations a round trip and the namespace cases go
/// through.
const DELETE_POLICY: &str = "DeleteDeliveryDestinationPolicy";
const PUT_POLICY: &str = "PutDeliveryDestinationPolicy";
const GET_POLICY: &str = "GetDeliveryDestinationPolicy";
const PUT_DESTINATION: &str = "PutDeliveryDestination";
const GET_DESTINATION: &str = "GetDeliveryDestination";
const PUT_SOURCE: &str = "PutDeliverySource";
const PUT_LOGICAL_DESTINATION: &str = "PutDestination";
const CREATE_LOG_GROUP: &str = "CreateLogGroup";

/// The refusal this operation models for a name the account holds no delivery destination
/// under, and the sentence CloudWatch Logs words it with.
const NOT_FOUND: &str = "ResourceNotFoundException";
const NOT_FOUND_MESSAGE: &str = "Requested Delivery Destination does not exist in this account.";

/// Destination resources of three of the vendors a delivery destination can represent.
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-bucket";
const LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:123456789012:log-group:/aws/vendedlogs/one:*";
const FIREHOSE_ARN: &str = "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs";

/// The `DeliveryDestinationType` of a trace destination, the one kind created from its type
/// alone. Source: the model's `DeliveryDestinationType` enum.
const TRACE: &str = "XRAY";

/// The published maximum length of `DeliveryDestinationName`. Source: the model's
/// `DeliveryDestinationName` shape (`@length` 1..60, `@pattern ^[\w-]*$`), which
/// `DeleteDeliveryDestinationPolicyRequest$deliveryDestinationName` targets.
const NAME_MAX_LENGTH: usize = 60;

/// One delivery destination in the store: how it is created, and the policy (if any) assigned
/// to it before the deletes start.
#[derive(Debug)]
struct Row {
    /// What this row varies, named in every assertion.
    label: &'static str,
    /// `name` (`DeliveryDestinationName`, 1..60, `^[\w-]*$`).
    name: String,
    /// The `destinationResourceArn` this destination represents, or `None` for the trace
    /// destination, which is created from its type alone.
    resource_arn: Option<&'static str>,
    /// The `deliveryDestinationPolicy` assigned to it, or `None` for a destination none has
    /// been assigned to.
    policy: Option<String>,
}

impl Row {
    /// The `PutDeliveryDestination` request that creates this destination.
    fn put_body(&self) -> Value {
        match self.resource_arn {
            Some(resource_arn) => json!({
                "name": self.name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
            None => json!({"name": self.name, "deliveryDestinationType": TRACE}),
        }
    }
}

/// The two statements the assignment operation's documentation describes — "One statement
/// enables general logs delivery, and the other allows delivery to the chosen destination" —
/// as one IAM policy document.
fn two_statement_policy(destination_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Sid": "AllowLogDeliveryActions",
                "Effect": "Allow",
                "Principal": {"Service": "delivery.logs.amazonaws.com"},
                "Action": ["logs:PutLogEvents", "logs:CreateLogStream"],
                "Resource": destination_arn,
            },
            {
                "Sid": "AllowLogDeliveryForResource",
                "Effect": "Allow",
                "Principal": {"Service": "delivery.logs.amazonaws.com"},
                "Action": "logs:PutDeliverySource",
                "Resource": "*",
            },
        ],
    })
    .to_string()
}

/// Destinations spanning the vendors, the trace type, both published bounds of
/// `DeliveryDestinationName`, policies of several forms, and destinations holding none — all in
/// ONE store.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an S3 bucket destination holding the two documented statements",
            name: "vended-logs-bucket".to_string(),
            resource_arn: Some(BUCKET_ARN),
            policy: Some(two_statement_policy(BUCKET_ARN)),
        },
        Row {
            label: "a log group destination holding no policy",
            name: "vended_logs_group".to_string(),
            resource_arn: Some(LOG_GROUP_ARN),
            policy: None,
        },
        Row {
            label: "a Firehose destination holding the smallest JSON object",
            name: "vended-logs-stream".to_string(),
            resource_arn: Some(FIREHOSE_ARN),
            policy: Some("{}".to_string()),
        },
        Row {
            label: "a trace destination holding one statement",
            name: "vended-traces".to_string(),
            resource_arn: None,
            policy: Some(
                json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow"}]}).to_string(),
            ),
        },
        Row {
            label: "a name at the minimum length, holding a non-ASCII policy",
            name: "d".to_string(),
            resource_arn: Some(BUCKET_ARN),
            policy: Some(
                json!({"Sid": "lieferung-\u{00fc}ber-gr\u{00f6}\u{00df}e-\u{65e5}\u{672c}"})
                    .to_string(),
            ),
        },
        Row {
            label: "a name at the maximum length, holding a pretty-printed policy",
            name: "n".repeat(NAME_MAX_LENGTH),
            resource_arn: Some(FIREHOSE_ARN),
            policy: Some("{\n  \"Version\" : \"2012-10-17\",\n  \"Statement\" : [ ]\n}".to_string()),
        },
    ]
}

/// The `policy.deliveryDestinationPolicy` the service reports for `name`, or `None` when the
/// response carries no `policy` member at all.
async fn read_policy(logs: &Logs, name: &str) -> Option<String> {
    let body = json!({"deliveryDestinationName": name});
    let response = logs.ok(GET_POLICY, &body).await;
    let policy = response.get("policy")?;
    Some(
        policy
            .get("deliveryDestinationPolicy")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!(
                    "{GET_POLICY} {body}: a reported policy carries deliveryDestinationPolicy, \
                     got {response}"
                )
            })
            .to_string(),
    )
}

/// Assign one policy over the wire.
async fn assign_policy(logs: &Logs, name: &str, document: &str) {
    let body = json!({"deliveryDestinationName": name, "deliveryDestinationPolicy": document});
    logs.ok(PUT_POLICY, &body).await;
}

/// Delete one destination's policy over the wire, asserting the empty success body this
/// operation models (its output shape is `Unit`).
async fn delete_policy(logs: &Logs, case: &str, name: &str) {
    let body = json!({"deliveryDestinationName": name});
    let response = logs.ok(DELETE_POLICY, &body).await;
    assert!(
        response.is_null() || response.as_object().is_some_and(serde_json::Map::is_empty),
        "case {case:?}: {DELETE_POLICY} reports no members, got {response}"
    );
}

/// The destination record the service reports for `name` — the OTHER reader of this
/// destination's state, so a delete that moved anything else about it shows up here.
async fn destination_record(logs: &Logs, name: &str) -> Value {
    let body = json!({"name": name});
    logs.ok(GET_DESTINATION, &body)
        .await
        .get("deliveryDestination")
        .cloned()
        .unwrap_or_else(|| panic!("{GET_DESTINATION} {body}: the destination must be reported"))
}

/// Every destination in the store, created and given the policy its row names.
async fn store(logs: &Logs, rows: &[Row]) {
    for row in rows {
        logs.ok(PUT_DESTINATION, &row.put_body()).await;
        if let Some(document) = &row.policy {
            assign_policy(logs, &row.name, document).await;
        }
    }
}

/// Deleting one destination's policy leaves that destination holding none, leaves every other
/// destination's policy exactly where it was, and leaves every other parameter of the named
/// destination alone.
#[tokio::test]
async fn a_delete_takes_the_policy_off_only_the_destination_it_names() {
    let logs = Logs::new();
    let rows = rows();
    store(&logs, &rows).await;

    // The other reader's view of each destination before any delete, so a delete that moved
    // anything but the policy is caught.
    let mut before = Vec::new();
    for row in &rows {
        before.push(destination_record(&logs, &row.name).await);
    }

    // Delete the destinations' policies one at a time: after each delete the destination it
    // named holds none, and every destination not yet deleted from still holds its own.
    for (index, row) in rows.iter().enumerate() {
        delete_policy(&logs, row.label, &row.name).await;

        for (other_index, other) in rows.iter().enumerate() {
            let case = (row.label, other.label);
            let expected = if other_index <= index {
                None
            } else {
                other.policy.clone()
            };
            let reported = read_policy(&logs, &other.name).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the policy {expected:?} after deleting from \
                 {:?}, got {reported:?}",
                row.name
            );
        }
    }

    // Nothing but the policy moved: every other member the destination carries reports the
    // same as it did before the deletes.
    for (row, snapshot) in rows.iter().zip(&before) {
        let after = destination_record(&logs, &row.name).await;
        let unchanged = |record: &Value| {
            let mut members = record
                .as_object()
                .unwrap_or_else(|| panic!("case {:?}: the destination is an object", row.label))
                .clone();
            members.remove("policy");
            members
        };
        assert_eq!(
            unchanged(&after),
            unchanged(snapshot),
            "case {:?}: the delete left every other parameter of the destination alone, \
             got {after}",
            row.label
        );
    }
}

/// A destination holding no policy is already in the state the delete asks for, so the delete
/// is accepted and changes nothing however often it is repeated — on a destination whose policy
/// was just deleted, and on one none was ever assigned to — and a later assignment on the same
/// destination takes effect and can be deleted again.
#[tokio::test]
async fn a_delete_is_idempotent_and_a_later_assignment_takes_effect() {
    let logs = Logs::new();
    let rows = rows();
    store(&logs, &rows).await;

    for row in &rows {
        // Four deletes in a row: the first takes off whatever the row holds, the rest find
        // nothing to take off, and all four are accepted.
        for round in 1..=4 {
            let case = (row.label, round);
            delete_policy(&logs, row.label, &row.name).await;
            let reported = read_policy(&logs, &row.name).await;
            assert_eq!(
                reported, None,
                "case {case:?}: expected no policy after the delete, got {reported:?}"
            );
        }

        // The destination is still there to assign to, and the assignment is reported back.
        let document = two_statement_policy(&format!("arn:aws:s3:::{}", row.name));
        assign_policy(&logs, &row.name, &document).await;
        let reported = read_policy(&logs, &row.name).await;
        assert_eq!(
            reported.as_deref(),
            Some(document.as_str()),
            "case {:?}: expected the newly assigned policy, got {reported:?}",
            row.label
        );

        // And the cycle closes: the new policy is deleted the same way.
        delete_policy(&logs, row.label, &row.name).await;
        let reported = read_policy(&logs, &row.name).await;
        assert_eq!(
            reported, None,
            "case {:?}: expected no policy after deleting the reassigned one, got {reported:?}",
            row.label
        );
    }
}

/// The policy is deleted off a delivery DESTINATION, so a name the account holds no delivery
/// destination under — a near-miss spelling of one it holds, or a name held only by another kind
/// of resource of this service — names no resource to delete a policy from and is refused;
/// the refused calls persist nothing.
#[tokio::test]
async fn a_name_the_account_holds_no_destination_under_is_refused() {
    let logs = Logs::new();
    let held = "vended-logs-bucket";
    logs.ok(
        PUT_DESTINATION,
        &json!({
            "name": held,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;
    let document = two_statement_policy(BUCKET_ARN);
    assign_policy(&logs, held, &document).await;

    // One namesake per other namespace of this service, so a name held THERE is still a name
    // held by no delivery destination.
    let namesake = "shared_name";
    logs.ok(
        PUT_SOURCE,
        &json!({"name": namesake, "resourceArn": BUCKET_ARN, "logType": "ACCESS_LOGS"}),
    )
    .await;
    logs.ok(CREATE_LOG_GROUP, &json!({"logGroupName": namesake}))
        .await;
    logs.ok(
        PUT_LOGICAL_DESTINATION,
        &json!({
            "destinationName": namesake,
            "targetArn": "arn:aws:kinesis:eu-west-2:123456789012:stream/vended",
            "roleArn": "arn:aws:iam::123456789012:role/CWLtoKinesisRole",
        }),
    )
    .await;

    // Near misses that all satisfy `DeliveryDestinationName` (1..60, `^[\w-]*$`): case,
    // separator, a missing character, an extra character, a leading separator, a name at each
    // published bound, and the name held only in the other namespaces.
    let refused = [
        ("the held name upper-cased", "VENDED-LOGS-BUCKET".to_string()),
        ("dashes written as underscores", "vended_logs_bucket".to_string()),
        ("one character short", held[..held.len() - 1].to_string()),
        ("one character too many", format!("{held}x")),
        ("a leading separator", format!("-{held}")),
        ("a name at the minimum length", "x".to_string()),
        ("a name at the maximum length", "z".repeat(NAME_MAX_LENGTH)),
        ("a name held only by other kinds of resource", namesake.to_string()),
    ];

    for (case, name) in &refused {
        let (code, message) = logs
            .refusal(DELETE_POLICY, &json!({"deliveryDestinationName": name}))
            .await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: a name the account holds no destination under is refused with \
             {NOT_FOUND}, got {code}"
        );
        assert_eq!(
            message, NOT_FOUND_MESSAGE,
            "case {case:?}: expected the refusal {NOT_FOUND_MESSAGE:?}, got {message:?}"
        );
    }

    // The held destination kept its policy through every refused delete.
    let reported = read_policy(&logs, held).await;
    assert_eq!(
        reported.as_deref(),
        Some(document.as_str()),
        "case \"the held spelling\": expected the assigned policy, got {reported:?}"
    );

    // The refused deletes persisted nothing: a destination created under one of those names
    // afterwards holds no policy, and one created with a policy keeps it.
    logs.ok(
        PUT_DESTINATION,
        &json!({"name": namesake, "deliveryDestinationType": TRACE}),
    )
    .await;
    let reported = read_policy(&logs, namesake).await;
    assert_eq!(
        reported, None,
        "case \"a destination created after the refused deletes\": expected no policy, \
         got {reported:?}"
    );
}
