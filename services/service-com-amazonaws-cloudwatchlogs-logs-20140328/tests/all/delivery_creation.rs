// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `CreateDelivery` and `GetDelivery`: pairing a delivery source with a
//! delivery destination, and reading the delivery back.
//!
//! The rule under test: `CreateDelivery` connects one delivery source and one delivery
//! destination the account ALREADY holds — a request naming a source, or a destination, this
//! account does not hold is refused `ResourceNotFoundException` and adds nothing — mints the
//! delivery's id, ARN and destination type itself, stores the optional configuration the
//! request carries, and reports the delivery `GetDelivery` and `DescribeDeliveries` then
//! report. A second create of a pair the account already holds creates nothing and answers
//! that stored delivery — the idempotence `tests/all/delivery_idempotence.rs` is about —
//! while the same source paired with another destination is another delivery.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response a handler does not actually compute cannot pass.
//!
//! The table spans what the rule generalises over rather than one request: which vendor the
//! destination belongs to (each `DeliveryDestinationType` the model enumerates that this
//! service can hold a destination for), record fields absent / empty / several, a delimiter
//! at both published length bounds, an S3 configuration present and absent, and tag maps of
//! zero and several entries. The recorded request an issue happens to mention is one row.

use serde_json::{json, Value};

use crate::harness::Logs;

/// The operations under test.
const CREATE: &str = "CreateDelivery";
const GET: &str = "GetDelivery";
const DELETE: &str = "DeleteDelivery";
const DESCRIBE: &str = "DescribeDeliveries";

/// The modelled error codes the rule names.
const NOT_FOUND: &str = "ResourceNotFoundException";
const CONFLICT: &str = "ConflictException";

/// The whole message a request naming a delivery source the account does not hold is refused
/// with, and the one for a destination it does not hold: each states that the requested
/// resource is not in this account and interpolates no name.
const SOURCE_NOT_FOUND: &str = "Requested Delivery Source does not exist in this account.";
const DESTINATION_NOT_FOUND: &str =
    "Requested Delivery Destination does not exist in this account.";

/// A delivery source over one log group of this account, as `PutDeliverySource` creates one.
async fn delivery_source(logs: &Logs, name: &str, group: &str) {
    logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
        .await;
    let (_, group_arn) = logs.group_arns(group).await;
    logs.ok(
        "PutDeliverySource",
        &json!({"name": name, "resourceArn": group_arn, "logType": "INSIGHTS_QUERY_LOGS"}),
    )
    .await;
}

/// A delivery destination over `resource_arn`, answering the destination's own ARN.
async fn delivery_destination(logs: &Logs, name: &str, resource_arn: &str) -> String {
    let answer = logs
        .ok(
            "PutDeliveryDestination",
            &json!({
                "name": name,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource_arn},
            }),
        )
        .await;
    answer["deliveryDestination"]["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("a put answers the destination's ARN, got {answer}"))
        .to_string()
}

/// The `delivery` member of a response that carries one.
fn delivery(answer: &Value, case: &str) -> Value {
    answer
        .get("delivery")
        .filter(|delivery| !delivery.is_null())
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected a delivery, got {answer}"))
}

/// Every delivery the account holds, as `DescribeDeliveries` reports them.
async fn described(logs: &Logs) -> Vec<Value> {
    let page = logs.ok(DESCRIBE, &json!({})).await;
    page["deliveries"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeDeliveries reports a deliveries list, got {page}"))
        .clone()
}

/// A create that pairs two resources the account holds stores the delivery, and the delivery
/// it reports is the one both readers then report.
#[tokio::test]
async fn a_created_delivery_is_the_one_both_readers_report() {
    let logs = Logs::new();
    delivery_source(&logs, "vpc-flow-logs", "/vended/source-one").await;
    delivery_source(&logs, "waf_logs", "/vended/source-two").await;
    let cwl_destination = {
        logs.ok("CreateLogGroup", &json!({"logGroupName": "/vended/destination"}))
            .await;
        let (_, group_arn) = logs.group_arns("/vended/destination").await;
        delivery_destination(&logs, "to-logs", &group_arn).await
    };
    let s3_destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-bucket").await;
    let firehose_destination = delivery_destination(
        &logs,
        "to-firehose",
        "arn:aws:firehose:eu-west-2:123456789012:deliverystream/vended-logs",
    )
    .await;

    // (case, the source, the destination ARN, the optional members the request carries, the
    // `deliveryDestinationType` the destination's own resource implies)
    let cases: Vec<(&str, &str, &str, Value, &str)> = vec![
        (
            "a CloudWatch Logs destination and no optional member",
            "vpc-flow-logs",
            &cwl_destination,
            json!({}),
            "CWL",
        ),
        (
            "an S3 destination with record fields, the longest delimiter and a configuration",
            "vpc-flow-logs",
            &s3_destination,
            json!({
                "recordFields": ["event_timestamp", "log_stream", "message"],
                // `FieldDelimiter` is `len 0..5`: the maximum.
                "fieldDelimiter": "|||||",
                "s3DeliveryConfiguration": {
                    "suffixPath": "AWSLogs/{account-id}/{source-type}",
                    "enableHiveCompatiblePath": true,
                },
                "tags": {"env": "prod", "team": "logs"},
            }),
            "S3",
        ),
        (
            "a Firehose destination with the empty record-field list and the shortest delimiter",
            "waf_logs",
            &firehose_destination,
            json!({
                // `RecordFields` is `@length(min: 0)`: the empty list is a list the caller
                // named, which is not the same as naming none.
                "recordFields": [],
                "fieldDelimiter": "",
            }),
            "FH",
        ),
    ];
    let mut created_ids: Vec<String> = Vec::new();
    for (case, source, destination_arn, optional, destination_type) in cases {
        let mut body = json!({
            "deliverySourceName": source,
            "deliveryDestinationArn": destination_arn,
        });
        if let Some(members) = optional.as_object() {
            for (member, value) in members {
                body[member] = value.clone();
            }
        }
        let created = delivery(&logs.ok(CREATE, &body).await, case);
        let id = created["id"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: a created delivery carries an id, got {created}"))
            .to_string();
        assert!(
            !created_ids.contains(&id),
            "case {case}: every create mints an id no earlier one used, got {id}"
        );
        assert_eq!(
            created["deliverySourceName"].as_str(),
            Some(source),
            "case {case}: the delivery names the source it was created from, got {created}"
        );
        assert_eq!(
            created["deliveryDestinationArn"].as_str(),
            Some(destination_arn),
            "case {case}: the delivery names the destination it was created from, got {created}"
        );
        assert_eq!(
            created["deliveryDestinationType"].as_str(),
            Some(destination_type),
            "case {case}: the destination type is read off the destination's own resource, got \
             {created}"
        );
        assert!(
            created["arn"]
                .as_str()
                .is_some_and(|arn| arn.contains(":logs:") && arn.ends_with(&format!("delivery:{id}"))),
            "case {case}: the delivery's ARN identifies the delivery this call created, got \
             {created}"
        );
        // Every optional member the request carried is stored as given.
        if let Some(members) = optional.as_object() {
            for (member, value) in members {
                assert_eq!(
                    &created[member], value,
                    "case {case}: the delivery carries the {member} the request named, got \
                     {created}"
                );
            }
        }

        // The round trip: both readers report the delivery this call reported.
        let fetched = delivery(&logs.ok(GET, &json!({"id": id})).await, case);
        assert_eq!(
            fetched, created,
            "case {case}: GetDelivery reports the delivery the create reported"
        );
        assert!(
            described(&logs).await.contains(&created),
            "case {case}: DescribeDeliveries reports the delivery the create reported"
        );
        created_ids.push(id);
    }
    assert_eq!(
        described(&logs).await.len(),
        created_ids.len(),
        "the account holds exactly the deliveries that were created"
    );

    // The round trip closes with the delete: it removes the delivery it names and nothing
    // else — "it does not delete the delivery destination or the delivery source" — and the
    // same delete a second time names a delivery that is gone.
    for id in &created_ids {
        logs.ok(DELETE, &json!({"id": id})).await;
        assert_eq!(
            logs.err(GET, &json!({"id": id})).await,
            NOT_FOUND,
            "case the delete of {id}: a deleted delivery is no longer reported"
        );
        assert_eq!(
            logs.err(DELETE, &json!({"id": id})).await,
            NOT_FOUND,
            "case the delete of {id} twice: a delivery that is gone cannot be deleted again"
        );
    }
    assert!(
        described(&logs).await.is_empty(),
        "every created delivery was deleted"
    );
    // The source and the destination the deliveries paired are still there, and the source
    // can be removed once no delivery is associated with it.
    logs.ok(
        "GetDeliverySource",
        &json!({"name": "vpc-flow-logs"}),
    )
    .await;
    logs.ok("GetDeliveryDestination", &json!({"name": "to-s3"}))
        .await;
    logs.ok("DeleteDeliverySource", &json!({"name": "vpc-flow-logs"}))
        .await;
    assert_eq!(
        logs.err("GetDeliverySource", &json!({"name": "vpc-flow-logs"}))
            .await,
        NOT_FOUND,
        "case the delete of the source: a deleted delivery source is no longer reported"
    );
    logs.ok("DeleteDeliveryDestination", &json!({"name": "to-s3"}))
        .await;
    assert_eq!(
        logs.err("GetDeliveryDestination", &json!({"name": "to-s3"}))
            .await,
        NOT_FOUND,
        "case the delete of the destination: a deleted delivery destination is no longer \
         reported"
    );
}

/// A create naming a resource the account does not hold is refused, and nothing is added.
#[tokio::test]
async fn a_create_naming_a_resource_this_account_does_not_hold_is_refused() {
    let logs = Logs::new();
    delivery_source(&logs, "vpc-flow-logs", "/vended/refusals").await;
    let destination =
        delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-refusals").await;

    // (case, the request, the code it is refused under, the whole message it is refused with)
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "a delivery source name no put ever created",
            json!({
                "deliverySourceName": "no-such-source",
                "deliveryDestinationArn": destination,
            }),
            NOT_FOUND,
            SOURCE_NOT_FOUND,
        ),
        (
            "a delivery destination ARN no put ever created",
            json!({
                "deliverySourceName": "vpc-flow-logs",
                "deliveryDestinationArn": format!("{destination}-missing"),
            }),
            NOT_FOUND,
            DESTINATION_NOT_FOUND,
        ),
        (
            "neither the source nor the destination",
            json!({
                "deliverySourceName": "no-such-source",
                "deliveryDestinationArn": format!("{destination}-missing"),
            }),
            NOT_FOUND,
            // The DESTINATION is the resource this create resolves first, so a request naming
            // neither is refused for the destination.
            DESTINATION_NOT_FOUND,
        ),
    ];
    for (case, body, expected_code, expected_message) in cases {
        let (code, message) = logs.refusal(CREATE, &body).await;
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code} with message {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case}: expected {expected_message:?}, got {message:?}"
        );
        assert!(
            described(&logs).await.is_empty(),
            "case {case}: a refused create adds no delivery"
        );
    }

    // A value that is not an ARN at all names no destination, which is a request-shape
    // problem rather than a missing resource.
    let code = logs
        .err(
            CREATE,
            &json!({
                "deliverySourceName": "vpc-flow-logs",
                "deliveryDestinationArn": "not-an-arn",
            }),
        )
        .await;
    assert_eq!(
        code, "ValidationException",
        "case a destination that is not an ARN: expected ValidationException, got {code}"
    );

    // A delivery id no create ever minted names no delivery.
    let code = logs.err(GET, &json!({"id": "0123456789abcdef"})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case a get of an id no create minted: expected {NOT_FOUND}, got {code}"
    );
}

/// A delivery IS the pair of its source and its destination, so a second create of one pair
/// answers the delivery that pair already names — while the same source paired with another
/// destination is another delivery.
#[tokio::test]
async fn a_second_create_of_one_pair_answers_the_delivery_it_already_names() {
    let logs = Logs::new();
    delivery_source(&logs, "vpc-flow-logs", "/vended/pairs-one").await;
    delivery_source(&logs, "waf_logs", "/vended/pairs-two").await;
    let first = delivery_destination(&logs, "to-s3", "arn:aws:s3:::vended-logs-pairs").await;
    let second =
        delivery_destination(&logs, "to-s3-other", "arn:aws:s3:::vended-logs-pairs-other").await;

    let created = delivery(
        &logs
            .ok(
                CREATE,
                &json!({"deliverySourceName": "vpc-flow-logs", "deliveryDestinationArn": first}),
            )
            .await,
        "the first create",
    );
    // "You can't delete a delivery source if any current deliveries are associated with it",
    // and the same holds for the destination.
    assert_eq!(
        logs.err("DeleteDeliverySource", &json!({"name": "vpc-flow-logs"}))
            .await,
        CONFLICT,
        "case a source a delivery is associated with: the delete is refused"
    );
    assert_eq!(
        logs.err("DeleteDeliveryDestination", &json!({"name": "to-s3"}))
            .await,
        CONFLICT,
        "case a destination a delivery is associated with: the delete is refused"
    );
    let again = delivery(
        &logs
            .ok(
                CREATE,
                &json!({"deliverySourceName": "vpc-flow-logs", "deliveryDestinationArn": first}),
            )
            .await,
        "the same pair twice",
    );
    assert_eq!(
        again, created,
        "case the same pair twice: the create answers the stored delivery, got {again:?}"
    );
    assert_eq!(
        described(&logs).await.len(),
        1,
        "case the same pair twice: the repeated create added nothing"
    );

    // (case, the pair, how many deliveries the account holds afterwards)
    let accepted: Vec<(&str, Value, usize)> = vec![
        (
            "the same source and another destination",
            json!({"deliverySourceName": "vpc-flow-logs", "deliveryDestinationArn": second}),
            2,
        ),
        (
            "another source and the first destination",
            json!({"deliverySourceName": "waf_logs", "deliveryDestinationArn": first}),
            3,
        ),
    ];
    for (case, body, expected) in accepted {
        let answer = logs.ok(CREATE, &body).await;
        let other = delivery(&answer, case);
        assert_ne!(
            other["id"], created["id"],
            "case {case}: another pair is another delivery, got {other}"
        );
        assert_eq!(
            described(&logs).await.len(),
            expected,
            "case {case}: the account holds {expected} deliveries"
        );
    }
}
