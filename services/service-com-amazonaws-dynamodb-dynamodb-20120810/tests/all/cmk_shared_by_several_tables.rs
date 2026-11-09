// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for ONE customer managed key carried by SEVERAL tables: the create
//! that associates it, the description that reports it, and the key state that gates every
//! table holding it at once.
//!
//! The rules these tests pin:
//!
//! > a `CreateTable` whose `SSESpecification` enables encryption associates the new table
//! > with the KMS key the specification names — in any form KMS accepts — and that table's
//! > `TableDescription` carries `SSEDescription` with the status `ENABLED`, the type `KMS`
//! > and the key ARN KMS RESOLVED the identifier to, from the create's own answer and on
//! > every later read; one key may be named by any number of tables, each of which keeps
//! > its own items.
//!
//! > the state of the key a table is encrypted with gates that table's whole data plane:
//! > while KMS will not use the key, every table naming it is refused and none of them
//! > stores anything, a table on another key or on the Amazon Web Services owned key is
//! > untouched, and once the key is usable again all of them serve the items they held
//! > before the key moved.
//!
//! The rows below differ along what the rules generalise over: the FORM the shared key's
//! identifier takes at create (key id, key ARN, alias name, alias ARN), the number of
//! tables holding the one key, and the key STATE the shared key is moved into.
//!
//! KMS is mounted beside DynamoDB over ONE shared engine, because the key ARN a table
//! reports and the key state its data plane is held to are that service's answers — the
//! same assembly `core/router` performs.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call, describe_when_active, emulator_with_kms, error_code, message, ok, ok_kms, REGION,
};

/// The package every KMS exception class a wrapped refusal names belongs to: the class name
/// is what a client classifies the fault on.
const KMS_PACKAGE: &str = "com.amazonaws.services.kms.model";

/// How the `Proxy` field ends the context of the wrapped KMS call.
const CONTEXT_TAIL: &str = "; Proxy: null)";

/// The alias the rows name the shared key by, created for it so the alias forms resolve to
/// the same key as the bare forms.
const SHARED_ALIAS: &str = "alias/shared-table-key";

/// Create a table in [`REGION`] keyed by a single `S` partition key `Id`, with the members
/// `extra` adds, and answer the `TableDescription` the create itself reported.
async fn create_table(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let mut request = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    ok(router, "CreateTable", request).await["TableDescription"].clone()
}

/// An `SSESpecification` that encrypts a table with the KMS key `key`.
fn encrypted_with(key: &str) -> Value {
    json!({"SSESpecification": {"Enabled": true, "SSEType": "KMS", "KMSMasterKeyId": key}})
}

/// A fresh symmetric KMS key in [`REGION`], as its key id and its key ARN.
async fn kms_key(router: &MultiServiceRouter) -> (String, String) {
    let key = ok_kms(router, "CreateKey", json!({})).await;
    let key_id = key["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let key_arn = key["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !key_id.is_empty() && !key_arn.is_empty(),
        "case the key: KMS must mint a key to name, got {key}"
    );
    (key_id, key_arn)
}

/// The `KeyState` KMS reports for `key_id`, which is the state the gate below is driven in.
async fn key_state(router: &MultiServiceRouter, key_id: &str) -> String {
    ok_kms(router, "DescribeKey", json!({"KeyId": key_id})).await["KeyMetadata"]["KeyState"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `SSEDescription` `DescribeTable` reports for `name`.
async fn described_encryption(router: &MultiServiceRouter, name: &str) -> Value {
    ok(router, "DescribeTable", json!({"TableName": name})).await["Table"]["SSEDescription"].clone()
}

/// The `Item` a consistent `GetItem` of `key` reports for `name`, `Null` when none.
async fn stored_item(router: &MultiServiceRouter, name: &str, key: &str) -> Value {
    ok(
        router,
        "GetItem",
        json!({"TableName": name, "Key": {"Id": {"S": key}}, "ConsistentRead": true}),
    )
    .await["Item"]
        .clone()
}

/// The names the tables sharing one key are created under, one per identifier form, and the
/// form each row names the key by. The forms are the ones `SSESpecification$KMSMasterKeyId`
/// states: "You can specify the key using ... its key ID, Amazon Resource Name (ARN), alias
/// name, or alias ARN".
fn shared_key_rows(key_id: &str, key_arn: &str, alias_arn: &str) -> Vec<(&'static str, String)> {
    vec![
        ("the key named by its key id", key_id.to_string()),
        ("the key named by its key ARN", key_arn.to_string()),
        ("the key named by its alias name", SHARED_ALIAS.to_string()),
        ("the key named by its alias ARN", alias_arn.to_string()),
    ]
}

#[tokio::test]
async fn one_customer_key_encrypts_every_table_that_names_it() {
    let router = emulator_with_kms();
    let (key_id, key_arn) = kms_key(&router).await;
    ok_kms(
        &router,
        "CreateAlias",
        json!({"AliasName": SHARED_ALIAS, "TargetKeyId": key_id.clone()}),
    )
    .await;
    let alias_arn = service_com_amazonaws_kms_trentservice::state::alias_arn(REGION, SHARED_ALIAS);
    let rows = shared_key_rows(&key_id, &key_arn, &alias_arn);

    // Every row names the SAME key in a different form, and each row's table is a table of
    // its own: the create's own answer already carries the resolved ARN, because the
    // encryption is in force from the table's first instant.
    for (row, (case, identifier)) in rows.iter().enumerate() {
        let name = format!("Shared{row}");
        let created = create_table(&router, &name, encrypted_with(identifier)).await;
        assert_eq!(
            (
                created["SSEDescription"]["Status"].clone(),
                created["SSEDescription"]["SSEType"].clone(),
                created["SSEDescription"]["KMSMasterKeyArn"].clone(),
            ),
            (json!("ENABLED"), json!("KMS"), json!(key_arn)),
            "case {case}: the create reports the resolved key ARN, got {created}"
        );
        assert_eq!(
            created["SSEDescription"]["InaccessibleEncryptionDateTime"].clone(),
            Value::Null,
            "case {case}: no call has detected an inaccessible key, got {created}"
        );
    }

    // Each table reads back the one key it shares, and holds only its own items: sharing a
    // key is not sharing a table.
    for (row, (case, _)) in rows.iter().enumerate() {
        let name = format!("Shared{row}");
        describe_when_active(&router, case, &name).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Id": {"S": name.clone()}, "V": {"N": "1"}}}),
        )
        .await;
    }
    for (row, (case, _)) in rows.iter().enumerate() {
        let name = format!("Shared{row}");
        let described = described_encryption(&router, &name).await;
        assert_eq!(
            (
                described["Status"].clone(),
                described["SSEType"].clone(),
                described["KMSMasterKeyArn"].clone(),
            ),
            (json!("ENABLED"), json!("KMS"), json!(key_arn)),
            "case {case}: a later read reports the same resolved key ARN, got {described}"
        );
        assert_eq!(
            stored_item(&router, &name, &name).await["V"],
            json!({"N": "1"}),
            "case {case}: the table serves the item it was written"
        );
        for other in 0..rows.len() {
            if other == row {
                continue;
            }
            assert_eq!(
                stored_item(&router, &name, &format!("Shared{other}")).await,
                Value::Null,
                "case {case}: a table sharing the key holds none of another table's items"
            );
        }
    }
}

#[tokio::test]
async fn the_shared_key_s_state_gates_every_table_holding_it() {
    let router = emulator_with_kms();
    let (shared_id, shared_arn) = kms_key(&router).await;
    let (_, other_id_arn) = kms_key(&router).await;

    // Three tables name the shared key, one names another key, and one is on the Amazon Web
    // Services owned key — so the gate can be shown to follow the KEY a table names rather
    // than the request or the store as a whole.
    let sharing = ["SharedA", "SharedB", "SharedC"];
    for name in sharing {
        create_table(&router, name, encrypted_with(&shared_arn)).await;
    }
    create_table(&router, "OnAnotherKey", encrypted_with(&other_id_arn)).await;
    create_table(&router, "OnOwnedKey", Value::Null).await;
    let unaffected = ["OnAnotherKey", "OnOwnedKey"];
    for name in sharing.iter().chain(unaffected.iter()) {
        describe_when_active(&router, name, name).await;
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Id": {"S": "kept"}, "V": {"N": "1"}}}),
        )
        .await;
    }

    // Rows differ in the key STATE the shared key is moved into, which is what decides
    // which KMS exception the refusal wraps and how DynamoDB introduces it. The prose is
    // KMS's own sentence for that state, and the `KeyState` is what KMS must report before
    // the data plane is driven — so a transition that silently did nothing fails on its own
    // row rather than as a mismatched refusal further down.
    let states: [(&str, &str, Value, &str, &str, &str, &str); 3] = [
        (
            "the shared key disabled",
            "DisableKey",
            json!({"KeyId": shared_id.clone()}),
            "Disabled",
            "KMS key disabled error",
            "DisabledException",
            "is disabled.",
        ),
        (
            "the shared key awaiting deletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": shared_id.clone(), "PendingWindowInDays": 7}),
            "PendingDeletion",
            "KMS validation error",
            "KMSInvalidStateException",
            "is pending deletion.",
        ),
        // "CancelKeyDeletion ... the key state ... changes to Disabled": cancelling the
        // deletion does not re-enable the key, so every table holding it is still refused.
        (
            "the shared key disabled again after the deletion was cancelled",
            "CancelKeyDeletion",
            json!({"KeyId": shared_id.clone()}),
            "Disabled",
            "KMS key disabled error",
            "DisabledException",
            "is disabled.",
        ),
    ];

    for (case, operation, body, expected_state, prefix, class, prose) in states {
        ok_kms(&router, operation, body).await;
        assert_eq!(
            key_state(&router, &shared_id).await,
            expected_state,
            "case {case}: KMS must report the shared key in that state before the data plane \
             is driven"
        );
        let head = format!(
            "{prefix}: {KMS_PACKAGE}.{class}: {shared_arn} {prose} (Service: AWSKMS; Status Code: \
             400; Error Code: {class}; Request ID: "
        );
        for name in sharing {
            for (surface, request) in [
                (
                    "GetItem",
                    json!({
                        "TableName": name,
                        "Key": {"Id": {"S": "kept"}},
                        "ConsistentRead": true,
                    }),
                ),
                (
                    "PutItem",
                    json!({"TableName": name, "Item": {"Id": {"S": "while-unusable"}}}),
                ),
                ("Scan", json!({"TableName": name})),
            ] {
                let (status, answered) = call(&router, surface, request).await;
                let refusal = message(&answered);
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ValidationException"),
                    "case {case} / {name} / {surface}: the shared key refuses the request, got \
                     {status} with {answered}"
                );
                assert!(
                    refusal.starts_with(&head) && refusal.ends_with(CONTEXT_TAIL),
                    "case {case} / {name} / {surface}: the refusal must wrap the KMS fault as \
                     {head:?}…{CONTEXT_TAIL:?}, got {refusal:?}"
                );
            }
        }
        // A table on another key and one on the owned key are untouched: the gate is the
        // table's OWN key.
        for name in unaffected {
            assert_eq!(
                stored_item(&router, name, "kept").await["V"],
                json!({"N": "1"}),
                "case {case}: {name} names another key and still serves its item"
            );
        }
    }

    // Enabling the one key restores every table that holds it, with the item each held
    // before the key moved intact and nothing the gate refused persisted.
    ok_kms(&router, "EnableKey", json!({"KeyId": shared_id})).await;
    for name in sharing {
        assert_eq!(
            stored_item(&router, name, "kept").await["V"],
            json!({"N": "1"}),
            "case after the shared key was enabled: {name} serves the item written before"
        );
        assert_eq!(
            stored_item(&router, name, "while-unusable").await,
            Value::Null,
            "case after the shared key was enabled: no write {name} refused persisted anything"
        );
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Id": {"S": "after"}, "V": {"N": "2"}}}),
        )
        .await;
        assert_eq!(
            stored_item(&router, name, "after").await["V"],
            json!({"N": "2"}),
            "case after the shared key was enabled: {name} is written again"
        );
    }
}
