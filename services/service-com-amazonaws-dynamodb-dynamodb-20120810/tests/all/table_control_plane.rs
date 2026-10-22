// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the table control-plane STATE MACHINE: which calls leave work
//! for the clock to finish, which calls are refused while that work is outstanding, and
//! what a later reader sees.
//!
//! The rules pinned here:
//!
//! 1. only the part of an `UpdateTable` that asks DynamoDB to move capacity — the
//!    table's capacity mode, its provisioned throughput, its table class or one of its
//!    secondary indexes — is asynchronous ("while it's executing, the table status
//!    changes from ACTIVE to UPDATING" — the `UpdateTable` doc); a request that only
//!    flips the deletion-protection flag is applied with the call, so the table is
//!    reported `ACTIVE` and the next control-plane call is served;
//! 2. an `UpdateTable` that names no updatable member is invalid, not a conflict:
//!    `ValidationException`, whatever status the table is in;
//! 3. a `DeleteTable` is accepted once — a second one on a `DELETING` table conflicts
//!    with `ResourceInUseException` — and the table really disappears, after which
//!    `DescribeTable` answers `ResourceNotFoundException` and the name is free again;
//! 4. `BillingModeSummary.LastUpdateToPayPerRequestDateTime` reports the instant
//!    `PAY_PER_REQUEST` TOOK EFFECT as the capacity mode, so it is absent while the
//!    call that sets that mode is still settling and stable afterwards.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use, and each rule is asserted over a
//! SET of inputs — the kind of update, the kind of table, the capacity mode — rather
//! than one recorded example.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;
use smithy_server_runtime::MultiServiceRouter;

// One harness for the whole test target: the router, the signed request and the two
// assertions every suite makes come from `crate::emulator` rather than a fourth copy.
use crate::emulator::{call, emulator, error_code, ok};

/// Wait out one transient span, so whatever the clock owes is due.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(150)).await;
}

/// The `TableStatus` a description reports.
fn status_of(description: &Value) -> &Value {
    &description["TableStatus"]
}

/// Create a `PROVISIONED` table carrying one global secondary index, and wait for it to
/// become `ACTIVE`.
async fn create_active_table(router: &MultiServiceRouter, name: &str) -> Value {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Part", "AttributeType": "S"},
                {"AttributeName": "Owner", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByOwner",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            }],
        }),
    )
    .await;
    settle().await;
    let described = ok(router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        status_of(&described["Table"]),
        &json!("ACTIVE"),
        "case setup {name}: the table becomes ACTIVE once the transition is due, got {described}"
    );
    described
}

#[tokio::test]
async fn an_update_is_asynchronous_only_where_it_moves_capacity() {
    let router = emulator();

    // Each row is one kind of update. `UPDATING` rows ask DynamoDB to move capacity (the
    // capacity mode, the table's throughput, its storage class, an index); the `ACTIVE`
    // rows only flip the deletion-protection flag, which the service applies with the
    // call. The second column is the status the response reports, the third whether the
    // NEXT control-plane call is served straight away.
    let cases: [(&str, Value, &str, bool); 6] = [
        (
            "deletion protection on",
            json!({"DeletionProtectionEnabled": true}),
            "ACTIVE",
            true,
        ),
        (
            "deletion protection off",
            json!({"DeletionProtectionEnabled": false}),
            "ACTIVE",
            true,
        ),
        (
            "provisioned throughput moved",
            json!({"ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 11}}),
            "UPDATING",
            false,
        ),
        (
            "capacity mode switched",
            json!({"BillingMode": "PAY_PER_REQUEST"}),
            "UPDATING",
            false,
        ),
        (
            "table class changed",
            json!({"TableClass": "STANDARD_INFREQUENT_ACCESS"}),
            "UPDATING",
            false,
        ),
        (
            "index throughput moved",
            json!({"GlobalSecondaryIndexUpdates": [{"Update": {
                "IndexName": "ByOwner",
                "ProvisionedThroughput": {"ReadCapacityUnits": 7, "WriteCapacityUnits": 7},
            }}]}),
            "UPDATING",
            false,
        ),
    ];

    for (case, update, expected_status, next_call_served) in cases {
        let name = format!("Settling{}", case.replace(' ', ""));
        create_active_table(&router, &name).await;

        let mut request = update.clone();
        request["TableName"] = json!(name);
        let updated = ok(&router, "UpdateTable", request).await;
        let description = &updated["TableDescription"];
        assert_eq!(
            status_of(description),
            &json!(expected_status),
            "case {case}: expected TableStatus {expected_status}, got {updated}"
        );

        // The next control-plane call, issued immediately: served when the update left
        // nothing outstanding, refused with ResourceInUseException while capacity moves
        // ("While it's UPDATING, you can't issue another UpdateTable request").
        let (status, body) = call(
            &router,
            "UpdateTable",
            json!({"TableName": name, "DeletionProtectionEnabled": false}),
        )
        .await;
        if next_call_served {
            assert_eq!(
                (status, status_of(&body["TableDescription"]).clone()),
                (200, json!("ACTIVE")),
                "case {case}: the next UpdateTable is served on a settled table, got {status} \
                 with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceInUseException"),
                "case {case}: a second UpdateTable while capacity moves is refused, got {status} \
                 with {body}"
            );
        }

        // Whatever the row, the table settles on its own and is then mutable again.
        settle().await;
        let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status_of(&described["Table"]),
            &json!("ACTIVE"),
            "case {case}: the table settles to ACTIVE, got {described}"
        );
        let dropped = ok(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            status_of(&dropped["TableDescription"]),
            &json!("DELETING"),
            "case {case}: a settled table can be deleted, got {dropped}"
        );
    }
}

#[tokio::test]
async fn an_update_naming_no_member_is_invalid_whatever_the_table_is_doing() {
    let router = emulator();
    create_active_table(&router, "Empty").await;

    // Rows differ in what the request carries BESIDES the table name: nothing, a
    // declaration that changes nothing on its own (AttributeDefinitions only declares
    // the attributes another member's key schema uses), and lists that ask for no entry.
    let refused: [(&str, Value); 4] = [
        ("table name only", json!({})),
        (
            "attribute definitions only",
            json!({"AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}]}),
        ),
        ("an empty index update list", json!({"GlobalSecondaryIndexUpdates": []})),
        (
            "an empty list and a declaration",
            json!({
                "AttributeDefinitions": [],
                "GlobalSecondaryIndexUpdates": [],
            }),
        ),
    ];
    for (case, extra) in &refused {
        let mut request = extra.clone();
        request["TableName"] = json!("Empty");
        let (status, body) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: an update that asks for nothing is invalid, got {status} with {body}"
        );
        let message = body["message"].as_str().unwrap_or_default().to_string();
        // The refusal has to name the alternatives, or the caller cannot fix the
        // request: the members are the operation's own updatable input members.
        for alternative in ["At least one of", "ProvisionedThroughput", "TableClass"] {
            assert!(
                message.contains(alternative),
                "case {case}: the refusal lists the alternatives (missing {alternative:?}), got \
                 {body}"
            );
        }
    }

    // A request that DOES name one of them is accepted, so the refusal above is about
    // the empty request and not about the table.
    for (case, extra) in [
        ("deletion protection", json!({"DeletionProtectionEnabled": false})),
        ("table class", json!({"TableClass": "STANDARD"})),
        (
            "throughput",
            json!({"ProvisionedThroughput": {"ReadCapacityUnits": 6, "WriteCapacityUnits": 6}}),
        ),
    ] {
        settle().await;
        let mut request = extra.clone();
        request["TableName"] = json!("Empty");
        let (status, body) = call(&router, "UpdateTable", request).await;
        assert_eq!(
            status, 200,
            "case {case}: an update naming a member is accepted, got {status} with {body}"
        );
    }

    // The same refusal while the table is in a transitional state: an empty update is
    // PERMANENTLY invalid, so it is answered before the in-use check — a conflict answer
    // would tell the caller to retry a request that can never succeed.
    let transitional: [(&str, Value); 2] = [
        (
            "while CREATING",
            json!({
                "TableName": "Fresh",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
        ),
        (
            "while UPDATING",
            json!({
                "TableName": "Moving",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
            }),
        ),
    ];
    for (case, create) in transitional {
        let name = create["TableName"].as_str().unwrap_or_default().to_string();
        ok(&router, "CreateTable", create.clone()).await;
        if case == "while UPDATING" {
            settle().await;
            ok(
                &router,
                "UpdateTable",
                json!({
                    "TableName": name,
                    "ProvisionedThroughput": {"ReadCapacityUnits": 8, "WriteCapacityUnits": 8},
                }),
            )
            .await;
        }
        let (status, body) = call(&router, "UpdateTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: an empty update is invalid rather than a conflict, got {status} with \
             {body}"
        );
    }
}

#[tokio::test]
async fn a_delete_is_accepted_once_and_the_table_then_disappears() {
    let router = emulator();

    // Rows differ in the kind of table being torn down: on-demand, provisioned,
    // provisioned with a secondary index, and one holding items.
    let cases: [(&str, Value, bool); 4] = [
        (
            "on-demand table",
            json!({
                "TableName": "DropOnDemand",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            false,
        ),
        (
            "provisioned table",
            json!({
                "TableName": "DropProvisioned",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }),
            false,
        ),
        (
            "table with a secondary index",
            json!({
                "TableName": "DropIndexed",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Owner", "AttributeType": "S"},
                ],
                "BillingMode": "PAY_PER_REQUEST",
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByOwner",
                    "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                }],
            }),
            false,
        ),
        (
            "table holding items",
            json!({
                "TableName": "DropWithItems",
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }),
            true,
        ),
    ];

    for (case, create, with_items) in cases {
        let name = create["TableName"].as_str().unwrap_or_default().to_string();
        ok(&router, "CreateTable", create.clone()).await;
        settle().await;
        if with_items {
            ok(
                &router,
                "PutItem",
                json!({"TableName": name, "Item": {"Part": {"S": "one"}}}),
            )
            .await;
        }

        let dropped = ok(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            status_of(&dropped["TableDescription"]),
            &json!("DELETING"),
            "case {case}: the accepted delete reports DELETING, got {dropped}"
        );

        // A second delete is a conflict, not a second acceptance: the table is a
        // resource which is still in use until the deletion completes.
        let (status, body) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceInUseException"),
            "case {case}: a second DeleteTable while DELETING conflicts, got {status} with {body}"
        );
        let deleting = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            status_of(&deleting["Table"]),
            &json!("DELETING"),
            "case {case}: the table is still readable as DELETING, got {deleting}"
        );

        // Once the deletion is due the table is GONE: a reader can observe completion.
        settle().await;
        for operation in ["DescribeTable", "DeleteTable"] {
            let (status, body) = call(&router, operation, json!({"TableName": name})).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case}: {operation} on the deleted table reports it gone, got {status} \
                 with {body}"
            );
        }
        // ...and the name is free, so a recreate does not collide.
        let recreated = ok(&router, "CreateTable", create.clone()).await;
        assert_eq!(
            status_of(&recreated["TableDescription"]),
            &json!("CREATING"),
            "case {case}: the deleted name can be used again, got {recreated}"
        );
    }
}

#[tokio::test]
async fn deletion_protection_refuses_the_delete_until_an_update_clears_it() {
    let router = emulator();

    // The flag can be set at create time or by a later update; both leave the table
    // undeletable until an update clears it.
    let cases: [(&str, bool); 2] = [("protected at create", true), ("protected by update", false)];
    for (case, protected_at_create) in cases {
        let name = format!("Protected{}", case.replace(' ', ""));
        ok(
            &router,
            "CreateTable",
            json!({
                "TableName": name,
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
                "DeletionProtectionEnabled": protected_at_create,
            }),
        )
        .await;
        settle().await;
        if !protected_at_create {
            let guarded = ok(
                &router,
                "UpdateTable",
                json!({"TableName": name, "DeletionProtectionEnabled": true}),
            )
            .await;
            assert_eq!(
                status_of(&guarded["TableDescription"]),
                &json!("ACTIVE"),
                "case {case}: flipping the flag leaves the table settled, got {guarded}"
            );
        }

        let (status, body) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a protected table is refused rather than deleted, got {status} with \
             {body}"
        );
        let still_there = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (
                status_of(&still_there["Table"]).clone(),
                still_there["Table"]["DeletionProtectionEnabled"].clone(),
            ),
            (json!("ACTIVE"), json!(true)),
            "case {case}: the refused delete left the table exactly as it was, got {still_there}"
        );

        // Clearing the flag is served straight away, and the delete then goes through to
        // completion.
        ok(
            &router,
            "UpdateTable",
            json!({"TableName": name, "DeletionProtectionEnabled": false}),
        )
        .await;
        let dropped = ok(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            status_of(&dropped["TableDescription"]),
            &json!("DELETING"),
            "case {case}: the unprotected table is deleted, got {dropped}"
        );
        settle().await;
        let (status, body) = call(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: the delete completes and the table is gone, got {status} with {body}"
        );
    }
}

#[tokio::test]
async fn pay_per_request_is_dated_from_the_instant_the_mode_takes_effect() {
    let router = emulator();

    /// The `LastUpdateToPayPerRequestDateTime` of a description, when present.
    fn dated(description: &Value) -> Option<f64> {
        description["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].as_f64()
    }

    // A table's capacity mode at CREATE time: only an on-demand table is ever dated, and
    // only once the mode is in effect — the create response still reports CREATING.
    let cases: [(&str, &str, bool); 2] = [
        ("created on-demand", "PAY_PER_REQUEST", true),
        ("created provisioned", "PROVISIONED", false),
    ];
    for (case, mode, dated_once_active) in cases {
        let name = format!("Dated{}", case.replace(' ', ""));
        let mut create = json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": mode,
        });
        if mode == "PROVISIONED" {
            create["ProvisionedThroughput"] =
                json!({"ReadCapacityUnits": 2, "WriteCapacityUnits": 2});
        }
        let created = ok(&router, "CreateTable", create).await;
        assert_eq!(
            (
                created["TableDescription"]["BillingModeSummary"]["BillingMode"].clone(),
                dated(&created["TableDescription"]),
            ),
            (json!(mode), None),
            "case {case}: the create response reports the mode but no instant yet, got {created}"
        );

        settle().await;
        let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        let first = dated(&described["Table"]);
        assert_eq!(
            first.is_some(),
            dated_once_active,
            "case {case}: expected dated={dated_once_active} once ACTIVE, got {described}"
        );
        if let Some(instant) = first {
            let creation = described["Table"]["CreationDateTime"]
                .as_f64()
                .unwrap_or_default();
            assert!(
                instant >= creation,
                "case {case}: the mode took effect no earlier than the table did, got {instant} \
                 against {creation}"
            );
        }
        // Reading twice reports the same instant: it records when the mode was set, and
        // a read does not re-set it.
        let again = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            dated(&again["Table"]),
            first,
            "case {case}: the instant is stable across reads, got {again}"
        );
    }

    // Switching an existing table's mode: the switch is asynchronous, so its own
    // response carries no instant yet, and the instant appears once the switch is in
    // effect. Switching back to PROVISIONED does not erase when PAY_PER_REQUEST was
    // last set.
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Switched",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
        }),
    )
    .await;
    settle().await;
    let switched = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "Switched", "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    assert_eq!(
        (
            status_of(&switched["TableDescription"]).clone(),
            dated(&switched["TableDescription"]),
        ),
        (json!("UPDATING"), None),
        "case switch to on-demand: the accepted switch is not in effect yet, got {switched}"
    );
    settle().await;
    let in_effect = ok(&router, "DescribeTable", json!({"TableName": "Switched"})).await;
    let switch_instant = dated(&in_effect["Table"]);
    assert!(
        switch_instant.is_some() && status_of(&in_effect["Table"]) == &json!("ACTIVE"),
        "case switch to on-demand: the instant appears once the switch settles, got {in_effect}"
    );

    ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Switched",
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 4, "WriteCapacityUnits": 4},
        }),
    )
    .await;
    settle().await;
    let back = ok(&router, "DescribeTable", json!({"TableName": "Switched"})).await;
    assert_eq!(
        (
            back["Table"]["BillingModeSummary"]["BillingMode"].clone(),
            dated(&back["Table"]),
        ),
        (json!("PROVISIONED"), switch_instant),
        "case switch back to provisioned: the recorded instant is when PAY_PER_REQUEST was last \
         set, got {back}"
    );
}
