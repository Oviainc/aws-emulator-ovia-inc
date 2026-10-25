// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule that keeps the two capacity models apart: an on-demand
//! capacity ceiling (`OnDemandThroughput`, carrying `MaxReadRequestUnits` /
//! `MaxWriteRequestUnits`) belongs to the on-demand capacity model only, so naming one
//! for a table whose billing mode is `PROVISIONED` is refused with HTTP 400
//! `ValidationException` — a permanently invalid request rather than a gap, a conflict or
//! a server-class answer — and the refused call leaves the stored table exactly as it
//! was.
//!
//! The rows of each table below differ along what the rule generalises over: the member
//! of the ceiling the request supplies, the operation that carries it, whether the
//! provisioned mode is the table's stored one or the one the request names, and whether
//! the ceiling is the table's own or an index's. The control row is the same ceiling on
//! an on-demand table, which this rule does not refuse.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, error_code, message, ok};

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// The message the refusal carries for `member` against `billing_mode`.
fn refusal(member: &str, billing_mode: &str) -> String {
    format!(
        "One or more parameter values were invalid: {member} for OnDemandThroughput cannot be \
         specified when the table BillingMode is {billing_mode}"
    )
}

/// Create one table in `billing_mode`, with the provisioned pair a provisioned table
/// needs.
async fn create_table(router: &MultiServiceRouter, name: &str, billing_mode: &str) -> Value {
    let mut request = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": billing_mode,
    });
    if billing_mode == "PROVISIONED" {
        request["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": 5, "WriteCapacityUnits": 5});
    }
    ok(router, "CreateTable", request).await
}

/// The provisioned pair `DescribeTable` reports for `name`.
async fn reported_capacity(router: &MultiServiceRouter, name: &str) -> (Value, Value) {
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    let throughput = &described["Table"]["ProvisionedThroughput"];
    (
        throughput["ReadCapacityUnits"].clone(),
        throughput["WriteCapacityUnits"].clone(),
    )
}

#[tokio::test]
async fn an_on_demand_ceiling_on_a_provisioned_table_is_refused_and_changes_nothing() {
    let router = emulator();
    create_table(&router, "CeilingProvisioned", "PROVISIONED").await;
    settle().await;

    // Rows differ in the member of the ceiling the request supplies — the refusal names
    // the one it sent — and in the values beside it: each member alone, both together
    // (the first the model declares is the one named), and a ceiling named beside a
    // provisioned pair the same request sets.
    let cases: [(&str, Value, &str); 4] = [
        (
            "the read ceiling alone",
            json!({"OnDemandThroughput": {"MaxReadRequestUnits": 100}}),
            "MaxReadRequestUnits",
        ),
        (
            "the write ceiling alone",
            json!({"OnDemandThroughput": {"MaxWriteRequestUnits": 7}}),
            "MaxWriteRequestUnits",
        ),
        (
            "both ceilings",
            json!({"OnDemandThroughput": {"MaxReadRequestUnits": 1, "MaxWriteRequestUnits": 2}}),
            "MaxReadRequestUnits",
        ),
        (
            "a ceiling beside a provisioned pair",
            json!({
                "OnDemandThroughput": {"MaxWriteRequestUnits": 40},
                "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 9},
            }),
            "MaxWriteRequestUnits",
        ),
    ];

    for (case, members, named) in cases {
        let mut request = json!({"TableName": "CeilingProvisioned"});
        for (member, value) in members.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = call(&router, "UpdateTable", request.clone()).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                refusal(named, "PROVISIONED")
            ),
            "case {case}: the ceiling contradicts the table's capacity mode, got {status} \
             with {body} for {request}"
        );
        // The refused update stored nothing: the table still reports the pair it was
        // created with.
        assert_eq!(
            reported_capacity(&router, "CeilingProvisioned").await,
            (json!(5), json!(5)),
            "case {case}: the refused update left the table's capacity alone"
        );
    }
}

#[tokio::test]
async fn the_provisioned_mode_the_refusal_names_is_the_one_in_force_for_the_update() {
    let router = emulator();
    create_table(&router, "CeilingStored", "PROVISIONED").await;
    create_table(&router, "CeilingSwitching", "PAY_PER_REQUEST").await;
    settle().await;

    // Rows differ in where the provisioned mode comes from: the table's stored one, with
    // the request naming no `BillingMode`; the same mode re-stated by the request; and a
    // request that switches an ON-DEMAND table to the provisioned mode in the same call
    // that names a ceiling — all three leave the table provisioned, so all three refuse.
    let cases: [(&str, &str, Value); 3] = [
        (
            "the table's stored mode",
            "CeilingStored",
            json!({"OnDemandThroughput": {"MaxReadRequestUnits": 100}}),
        ),
        (
            "the stored mode re-stated",
            "CeilingStored",
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
                "OnDemandThroughput": {"MaxReadRequestUnits": 100},
            }),
        ),
        (
            "a switch to the provisioned mode",
            "CeilingSwitching",
            json!({
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
                "OnDemandThroughput": {"MaxReadRequestUnits": 100},
            }),
        ),
    ];

    for (case, table, members) in cases {
        let mut request = json!({"TableName": table});
        for (member, value) in members.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        let (status, body) = call(&router, "UpdateTable", request.clone()).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                refusal("MaxReadRequestUnits", "PROVISIONED")
            ),
            "case {case}: the refusal names the mode in force, got {status} with {body} for \
             {request}"
        );
    }

    // Neither table moved: the stored one still reports its pair, and the on-demand one
    // still reports the zeroes an on-demand table reports.
    assert_eq!(
        (
            reported_capacity(&router, "CeilingStored").await,
            reported_capacity(&router, "CeilingSwitching").await,
        ),
        ((json!(5), json!(5)), (json!(0), json!(0))),
        "case after the refusals: neither table's capacity moved"
    );
}

#[tokio::test]
async fn an_index_ceiling_follows_the_same_rule_as_the_table_s_own() {
    let router = emulator();
    create_table(&router, "CeilingIndexed", "PROVISIONED").await;
    settle().await;

    // An index's capacity mode is the table's, so a ceiling named per index is refused by
    // the same rule. Rows differ in the operation and in which index update carries it:
    // a create at CreateTable, a create at UpdateTable, and an update of a stored index.
    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "CeilingIndexed",
            "AttributeDefinitions": [{"AttributeName": "Other", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [{
                "Create": {
                    "IndexName": "Ceiling.Index",
                    "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
                },
            }],
        }),
    )
    .await;
    settle().await;

    let cases: [(&str, &str, Value, &str); 3] = [
        (
            "an index created with a new table",
            "CreateTable",
            json!({
                "TableName": "CeilingCreated",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Other", "AttributeType": "S"},
                ],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "New.Index",
                    "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                    "OnDemandThroughput": {"MaxReadRequestUnits": 50},
                }],
            }),
            "MaxReadRequestUnits",
        ),
        (
            "an index created on a stored table",
            "UpdateTable",
            json!({
                "TableName": "CeilingIndexed",
                "AttributeDefinitions": [{"AttributeName": "Other", "AttributeType": "S"}],
                "GlobalSecondaryIndexUpdates": [{
                    "Create": {
                        "IndexName": "Second.Index",
                        "KeySchema": [{"AttributeName": "Other", "KeyType": "HASH"}],
                        "Projection": {"ProjectionType": "ALL"},
                        "OnDemandThroughput": {"MaxWriteRequestUnits": 60},
                    },
                }],
            }),
            "MaxWriteRequestUnits",
        ),
        (
            "an update of a stored index",
            "UpdateTable",
            json!({
                "TableName": "CeilingIndexed",
                "GlobalSecondaryIndexUpdates": [{
                    "Update": {
                        "IndexName": "Ceiling.Index",
                        "OnDemandThroughput": {"MaxReadRequestUnits": 70},
                    },
                }],
            }),
            "MaxReadRequestUnits",
        ),
    ];

    for (case, operation, request, named) in cases {
        let (status, body) = call(&router, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body), message(&body)),
            (
                400,
                "ValidationException".to_string(),
                refusal(named, "PROVISIONED")
            ),
            "case {case} on {operation}: an index ceiling follows the table's capacity mode, \
             got {status} with {body} for {request}"
        );
    }

    // Nothing was created or moved: the refused create left no table behind, and the
    // stored index still reports the pair it was created with.
    let (status, body) = call(&router, "DescribeTable", json!({"TableName": "CeilingCreated"})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case after the refusals: the refused create stored no table, got {status} with {body}"
    );
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "CeilingIndexed"}),
    )
    .await;
    let index = crate::emulator::index_named(&described["Table"], "Ceiling.Index");
    assert_eq!(
        (
            index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            index["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            crate::emulator::index_named(&described["Table"], "Second.Index"),
        ),
        (json!(2), json!(2), Value::Null),
        "case after the refusals: the stored index is untouched and no second one exists, \
         got {described}"
    );
}

#[tokio::test]
async fn a_ceiling_on_an_on_demand_table_is_not_refused_by_this_rule() {
    let router = emulator();
    create_table(&router, "CeilingOnDemand", "PAY_PER_REQUEST").await;
    settle().await;

    // The control: the ceilings refused above belong to the on-demand capacity model, so
    // the SAME requests against an on-demand table are not this refusal. Whether that
    // path is served or still reported as unimplemented is another issue's; what this
    // rule says is only that it is not a `ValidationException` about the capacity mode.
    // Rows differ in how the table comes to be on-demand: stored that way, and switched
    // by the same request.
    let cases: [(&str, Value); 2] = [
        (
            "an on-demand table",
            json!({
                "TableName": "CeilingOnDemand",
                "OnDemandThroughput": {"MaxReadRequestUnits": 100},
            }),
        ),
        (
            "a switch to on-demand",
            json!({
                "TableName": "CeilingOnDemand",
                "BillingMode": "PAY_PER_REQUEST",
                "OnDemandThroughput": {"MaxWriteRequestUnits": 100},
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = call(&router, "UpdateTable", request.clone()).await;
        assert_ne!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: an on-demand ceiling on an on-demand table is not this refusal, \
             got {status} with {body} for {request}"
        );
    }
}
