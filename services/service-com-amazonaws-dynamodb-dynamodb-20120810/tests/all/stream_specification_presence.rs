// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE rule about every `TableDescription` this service answers
//! with: the `StreamSpecification` member describes a stream that is ON, so it is
//! reported while the table streams and is ABSENT — not present with the flag false —
//! whenever the table does not, while `LatestStreamArn`/`LatestStreamLabel` keep
//! naming the latest stream the table ever minted.
//!
//! That presence is the test a client applies: SDK code and IaC drift detection ask
//! whether the member is there, not what a flag inside it says, so a present-but-false
//! specification reads as "still streaming".
//!
//! The rows span the ways a table's stream reaches each state rather than one recorded
//! sequence: never asked for, asked for and turned down on the create, enabled on the
//! create, enabled by an update, turned off by an update (observed both while the
//! disable is still settling and once it has settled), and enabled again afterwards —
//! over every `StreamViewType` the model declares. Every assertion reads the member
//! back through `DescribeTable`, so nothing is proved that a client cannot observe.

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::ops::types::STREAM_VIEW_TYPE_VALUES;
use service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::state::STREAM_DISABLE_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, call_streams, emulator_with_streams, REGION};

/// Every value the model's `StreamViewType` enum declares, taken from the list the
/// streams crate derives from that model, so these rows cannot drift from the enum.
const VIEW_TYPES: &[&str] = STREAM_VIEW_TYPE_VALUES;

/// A `CreateTable` body for `name`, optionally carrying a `StreamSpecification`.
fn create_body(name: &str, stream: Option<Value>) -> Value {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    if let Some(specification) = stream {
        body["StreamSpecification"] = specification;
    }
    body
}

async fn ddb_ok(router: &MultiServiceRouter, case: &str, op: &str, body: Value) -> Value {
    let (status, parsed) = call_in(router, REGION, op, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {op} to answer HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The table's own `TableDescription`, read back through `DescribeTable`.
async fn described(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    ddb_ok(router, case, "DescribeTable", json!({"TableName": name}))
        .await["Table"]
        .clone()
}

/// Wait out the `CREATING` -> `ACTIVE` transition the clock owes a new table, so an
/// update is not refused as in-use. The margin is slack for a loaded machine, not
/// part of the rule.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + std::time::Duration::from_millis(150)).await;
}

/// Wait out the `DISABLING` -> `DISABLED` span the clock owes a stream a disable
/// turned off, with the same kind of margin.
async fn settle_disable() {
    tokio::time::sleep(STREAM_DISABLE_DELAY + std::time::Duration::from_millis(150)).await;
}

/// The `StreamStatus` the STREAMS service reports for `stream_arn`.
async fn stream_status(router: &MultiServiceRouter, case: &str, stream_arn: &str) -> String {
    let (status, body) =
        call_streams(router, "DescribeStream", json!({"StreamArn": stream_arn})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStream must resolve the table's latest stream, got {status} \
         with {body}"
    );
    body["StreamDescription"]["StreamStatus"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Assert that `description` reports exactly the specification `expected` names, or
/// no `StreamSpecification` member at all where it is `None`.
fn assert_specification(case: &str, description: &Value, expected: Option<Value>) {
    let actual = description.get("StreamSpecification");
    match expected {
        None => assert_eq!(
            actual, None,
            "case {case:?}: a table whose stream is off reports no StreamSpecification, got \
             {actual:?}"
        ),
        Some(specification) => assert_eq!(
            actual,
            Some(&specification),
            "case {case:?}: expected the specification of the stream in force, got {actual:?}"
        ),
    }
}

#[tokio::test]
async fn the_specification_is_reported_only_while_the_table_streams() {
    // One row per `StreamViewType`, walking the whole lifecycle: on at create, off by
    // update (twice over: while the disable is settling, and once settled), then on
    // again in a DIFFERENT view type. The member's presence must follow the stream and
    // nothing else, and the table must keep naming its latest stream throughout.
    for view_type in VIEW_TYPES {
        let case = format!("{view_type} enabled, disabled, re-enabled");
        let router = emulator_with_streams();
        let name = "lifecycle";

        let created = ddb_ok(
            &router,
            &case,
            "CreateTable",
            create_body(
                name,
                Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
            ),
        )
        .await;
        let first_arn = created["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !first_arn.is_empty(),
            "case {case:?}: an enabled stream must be named by the create, got {created}"
        );
        assert_specification(
            &case,
            &described(&router, &case, name).await,
            Some(json!({"StreamEnabled": true, "StreamViewType": view_type})),
        );
        settle().await;

        // Turn it off. The stream is still DISABLING here, and already not a stream
        // the table carries settings for.
        ddb_ok(
            &router,
            &case,
            "UpdateTable",
            json!({"TableName": name, "StreamSpecification": {"StreamEnabled": false}}),
        )
        .await;
        let while_disabling = described(&router, &case, name).await;
        assert_specification(&format!("{case}, still disabling"), &while_disabling, None);
        assert_eq!(
            while_disabling["LatestStreamArn"].as_str(),
            Some(first_arn.as_str()),
            "case {case:?}: the table still names the stream it turned off, got \
             {while_disabling}"
        );

        settle_disable().await;
        let settled_status = stream_status(&router, &case, &first_arn).await;
        assert_eq!(
            settled_status, "DISABLED",
            "case {case:?}: the clock owes the stream a DISABLED status, got {settled_status}"
        );
        let once_disabled = described(&router, &case, name).await;
        assert_specification(&format!("{case}, disabled"), &once_disabled, None);
        assert_eq!(
            once_disabled["LatestStreamArn"].as_str(),
            Some(first_arn.as_str()),
            "case {case:?}: LatestStreamArn keeps pointing at the disabled stream, got \
             {once_disabled}"
        );
        assert!(
            once_disabled["LatestStreamLabel"].is_string(),
            "case {case:?}: the latest stream keeps its label too, got {once_disabled}"
        );

        // Enabling again mints a new stream, and the member is back — carrying the
        // view type now in force rather than the one the first stream had.
        let re_enabled = ddb_ok(
            &router,
            &case,
            "UpdateTable",
            json!({
                "TableName": name,
                "StreamSpecification": {"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"},
            }),
        )
        .await;
        let second_arn = re_enabled["TableDescription"]["LatestStreamArn"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_ne!(
            second_arn, first_arn,
            "case {case:?}: re-enabling names a new stream, got {re_enabled}"
        );
        assert_specification(
            &format!("{case}, re-enabled"),
            &described(&router, &case, name).await,
            Some(json!({"StreamEnabled": true, "StreamViewType": "KEYS_ONLY"})),
        );
    }
}

#[tokio::test]
async fn a_table_that_never_streamed_reports_no_specification_either() {
    // The same absence, reached without ever enabling a stream: a create that says
    // nothing about streams and one that explicitly turns them off are the same table.
    // Neither reports a specification, and neither names a stream at all — which is
    // what distinguishes them from the disabled table above. A specification that
    // leaves the stream off and still names a view type is not a row here: it states a
    // setting for a stream it is not asking for and is refused, which
    // `crate::request_validation_refusals` asserts.
    for (case, specification) in [
        ("no StreamSpecification", None),
        ("StreamEnabled false", Some(json!({"StreamEnabled": false}))),
    ] {
        let router = emulator_with_streams();
        ddb_ok(
            &router,
            case,
            "CreateTable",
            create_body("quiet", specification),
        )
        .await;
        let description = described(&router, case, "quiet").await;
        assert_specification(case, &description, None);
        for member in ["LatestStreamArn", "LatestStreamLabel"] {
            assert_eq!(
                description.get(member),
                None,
                "case {case:?}: a table with no stream names no {member}, got {description}"
            );
        }
    }
}
