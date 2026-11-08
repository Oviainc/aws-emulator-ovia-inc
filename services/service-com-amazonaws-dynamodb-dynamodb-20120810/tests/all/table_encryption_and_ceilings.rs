// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two table settings `UpdateTable` moves beside its indexes:
//! server-side encryption and the on-demand capacity ceiling.
//!
//! The rules these tests pin:
//!
//! > an `SSESpecification` that enables encryption names a KMS key in any of the forms KMS
//! > accepts — a key id, a key ARN, an alias name, an alias ARN, or nothing at all, which
//! > is the default DynamoDB key — and the table then reports `SSEDescription` with the
//! > type `KMS` and the key ARN KMS RESOLVED that identifier to, `UPDATING` while the
//! > change is in flight and `ENABLED` once it is in force; a specification that enables
//! > nothing leaves the table encrypted with the Amazon Web Services owned key, which has
//! > no description at all; and an identifier KMS does not resolve, or a type the service
//! > does not offer, is refused with HTTP 400 `ValidationException`.
//!
//! > each member of an `OnDemandThroughput` is one ceiling of the on-demand capacity
//! > model, set by a value of 1 or more and REMOVED by -1 through that same member, while
//! > a member the request leaves out leaves its ceiling exactly as it was; a dimension
//! > with no maximum is reported as the same -1 that clears it, and a value that is
//! > neither — zero, or a negative number that is not the removal value — is refused with
//! > HTTP 400 `ValidationException`.
//!
//! The rows of each table below differ along what the rule generalises over: the FORM the
//! key identifier takes, the operation that carries the specification, which ceiling the
//! request names and whether it sets, clears or leaves it alone.
//!
//! The KMS service is mounted beside DynamoDB over ONE shared engine, because the key ARN
//! a table reports is that service's answer — the same assembly `core/router` performs.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator_with_kms, error_code, ok_in, ok_kms, ok_kms_in, REGION};

/// Wait out one transient span, so whatever the clock owes a table is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One table in `billing_mode`, created in [`REGION`] with the members `extra` adds.
async fn create_table(
    router: &MultiServiceRouter,
    name: &str,
    billing_mode: &str,
    extra: Value,
) -> Value {
    let mut request = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": billing_mode,
    });
    if billing_mode == "PROVISIONED" {
        request["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 5, "WriteCapacityUnits": 5});
    }
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    ok_in(router, REGION, "CreateTable", request).await
}

/// The `SSEDescription` `DescribeTable` reports for `name`.
async fn described_encryption(router: &MultiServiceRouter, name: &str) -> Value {
    ok_in(router, REGION, "DescribeTable", json!({"TableName": name})).await["Table"]
        ["SSEDescription"]
        .clone()
}

/// The `OnDemandThroughput` `DescribeTable` reports for `name`.
async fn described_ceilings(router: &MultiServiceRouter, name: &str) -> Value {
    ok_in(router, REGION, "DescribeTable", json!({"TableName": name})).await["Table"]
        ["OnDemandThroughput"]
        .clone()
}

#[tokio::test]
async fn an_sse_specification_reports_the_key_arn_kms_resolved_it_to() {
    let router = emulator_with_kms();
    // A customer managed key and an alias for it, so the rows below can name one key in
    // every form KMS accepts.
    let key = ok_kms(&router, "CreateKey", json!({})).await;
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
    ok_kms(
        &router,
        "CreateAlias",
        json!({"AliasName": "alias/orders-table", "TargetKeyId": key_id}),
    )
    .await;
    let alias_arn =
        service_com_amazonaws_kms_trentservice::state::alias_arn(REGION, "alias/orders-table");
    // The default DynamoDB key is a predefined Amazon Web Services alias: KMS associates
    // it with a managed key, and that key's ARN is what a table naming no key reports.
    let default_key = ok_kms(
        &router,
        "DescribeKey",
        json!({"KeyId": "alias/aws/dynamodb"}),
    )
    .await;
    let default_arn = default_key["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows differ in the FORM the identifier takes, which is the whole of what the rule
    // generalises over: every form of one key resolves to that key's ARN.
    let cases: [(&str, Value, &str); 5] = [
        (
            "no key id at all, which is the default DynamoDB key",
            json!({"Enabled": true}),
            default_arn.as_str(),
        ),
        (
            "the default key named by its alias",
            json!({"Enabled": true, "KMSMasterKeyId": "alias/aws/dynamodb"}),
            default_arn.as_str(),
        ),
        (
            "a customer managed key by key id",
            json!({"Enabled": true, "KMSMasterKeyId": key_id, "SSEType": "KMS"}),
            key_arn.as_str(),
        ),
        (
            "the same key by key ARN",
            json!({"Enabled": true, "KMSMasterKeyId": key_arn}),
            key_arn.as_str(),
        ),
        (
            "the same key by alias ARN",
            json!({"Enabled": true, "KMSMasterKeyId": alias_arn}),
            key_arn.as_str(),
        ),
    ];

    // One table per row, so every row's update is in flight over the same span.
    for (row, (case, _, _)) in cases.iter().enumerate() {
        create_table(&router, &format!("Encrypted{row}"), "PAY_PER_REQUEST", json!({})).await;
        assert_eq!(
            described_encryption(&router, &format!("Encrypted{row}")).await,
            Value::Null,
            "case {case}: a table encrypted with the owned key reports no SSEDescription"
        );
    }
    settle().await;

    for (row, (case, specification, _)) in cases.iter().enumerate() {
        let updated = ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": format!("Encrypted{row}"),
                "SSESpecification": specification,
            }),
        )
        .await;
        assert_eq!(
            (
                updated["TableDescription"]["SSEDescription"]["Status"].clone(),
                updated["TableDescription"]["SSEDescription"]["SSEType"].clone(),
            ),
            (json!("UPDATING"), json!("KMS")),
            "case {case}: the change is in flight when the call answers, got {updated}"
        );
    }
    settle().await;

    for (row, (case, _, expected)) in cases.iter().enumerate() {
        let described = described_encryption(&router, &format!("Encrypted{row}")).await;
        assert_eq!(
            (
                described["Status"].clone(),
                described["SSEType"].clone(),
                described["KMSMasterKeyArn"].clone(),
            ),
            (json!("ENABLED"), json!("KMS"), json!(expected)),
            "case {case}: the table reports the resolved key ARN, got {described}"
        );
    }

    // A specification that enables nothing puts the table back on the owned key, which has
    // no description of its own.
    let cleared = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": "Encrypted0",
            "SSESpecification": {"Enabled": false},
        }),
    )
    .await;
    assert_eq!(
        cleared["TableDescription"]["SSEDescription"].clone(),
        Value::Null,
        "case disabling encryption: the owned key has no SSEDescription, got {cleared}"
    );
}

#[tokio::test]
async fn an_sse_specification_at_create_is_in_force_from_the_table_s_first_instant() {
    let router = emulator_with_kms();
    let key = ok_kms(&router, "CreateKey", json!({})).await;
    let key_arn = key["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // Rows differ in the key the create names: the default DynamoDB key, and a customer
    // managed one.
    let created = create_table(
        &router,
        "EncryptedAtCreate",
        "PAY_PER_REQUEST",
        json!({"SSESpecification": {"Enabled": true, "KMSMasterKeyId": key_arn}}),
    )
    .await;
    let defaulted = create_table(
        &router,
        "EncryptedByDefaultKey",
        "PROVISIONED",
        json!({"SSESpecification": {"Enabled": true}}),
    )
    .await;
    let default_arn = ok_kms(
        &router,
        "DescribeKey",
        json!({"KeyId": "alias/aws/dynamodb"}),
    )
    .await["KeyMetadata"]["Arn"]
        .clone();
    assert_eq!(
        (
            created["TableDescription"]["SSEDescription"]["Status"].clone(),
            created["TableDescription"]["SSEDescription"]["KMSMasterKeyArn"].clone(),
            defaulted["TableDescription"]["SSEDescription"]["Status"].clone(),
            defaulted["TableDescription"]["SSEDescription"]["KMSMasterKeyArn"].clone(),
        ),
        (
            json!("ENABLED"),
            json!(key_arn),
            json!("ENABLED"),
            default_arn.clone(),
        ),
        "case a create that enables encryption: it is in force at once, got {created} and \
         {defaulted}"
    );
    settle().await;
    assert_eq!(
        (
            described_encryption(&router, "EncryptedAtCreate").await["KMSMasterKeyArn"].clone(),
            described_encryption(&router, "EncryptedByDefaultKey").await["KMSMasterKeyArn"]
                .clone(),
        ),
        (json!(key_arn), default_arn),
        "case the created tables read back: each keeps the key it was created with"
    );
}

#[tokio::test]
async fn an_sse_specification_the_service_cannot_serve_is_refused() {
    let router = emulator_with_kms();
    create_table(&router, "EncryptedRefusals", "PAY_PER_REQUEST", json!({})).await;
    settle().await;

    // Rows differ in WHY the specification cannot be served: a key identifier KMS resolves
    // to nothing — in each form one can take — and an encryption type the service does not
    // offer.
    let cases: [(&str, Value); 4] = [
        (
            "an alias no key carries",
            json!({"Enabled": true, "KMSMasterKeyId": "alias/no-such-key"}),
        ),
        (
            "a key id no key carries",
            json!({
                "Enabled": true,
                "KMSMasterKeyId": "11111111-2222-3333-4444-555555555555",
            }),
        ),
        (
            "a key ARN of another Region",
            json!({
                "Enabled": true,
                "KMSMasterKeyId": "arn:aws:kms:eu-west-1:000000000000:key/\
                                   11111111-2222-3333-4444-555555555555",
            }),
        ),
        (
            "an encryption type the service does not offer",
            json!({"Enabled": true, "SSEType": "AES256"}),
        ),
    ];

    for (case, specification) in cases {
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": "EncryptedRefusals",
                "SSESpecification": specification.clone(),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: the specification cannot be served, so it is a client error, got \
             {status} with {body} for {specification}"
        );
    }

    // Nothing was stored: the table is still encrypted with the owned key.
    assert_eq!(
        described_encryption(&router, "EncryptedRefusals").await,
        Value::Null,
        "case after the refusals: the table's encryption is untouched"
    );
}

#[tokio::test]
async fn a_replica_s_kms_key_is_resolved_and_in_force_in_its_own_region() {
    let router = emulator_with_kms();
    // A KMS key is REGIONAL, and the key a replica is encrypted with is one its own
    // Region's endpoint serves — so this key is created in the Region the replica will
    // live in, not in the Region the table was created in.
    let key = ok_kms_in(&router, REPLICA_REGION, "CreateKey", json!({})).await;
    let key_id = key["KeyMetadata"]["KeyId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let key_arn = key["KeyMetadata"]["Arn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    ok_kms_in(
        &router,
        REPLICA_REGION,
        "CreateAlias",
        json!({"AliasName": "alias/replica-table", "TargetKeyId": key_id}),
    )
    .await;
    create_table(&router, "ReplicaEncrypted", "PAY_PER_REQUEST", json!({})).await;
    settle().await;

    // A replica's `KMSMasterKeyId` follows the same rule the table's own
    // `SSESpecification` does: whichever form the identifier takes, KMS resolves it, the
    // replica's own table is encrypted with that key, and the group reports its ARN.
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": "ReplicaEncrypted",
            "ReplicaUpdates": [{"Create": {
                "RegionName": REPLICA_REGION,
                "KMSMasterKeyId": "alias/replica-table",
            }}],
        }),
    )
    .await;
    settle().await;

    let local = ok_in(
        &router,
        REGION,
        "DescribeTable",
        json!({"TableName": "ReplicaEncrypted"}),
    )
    .await;
    let replica = ok_in(
        &router,
        REPLICA_REGION,
        "DescribeTable",
        json!({"TableName": "ReplicaEncrypted"}),
    )
    .await;
    assert_eq!(
        (
            local["Table"]["Replicas"][0]["KMSMasterKeyId"].clone(),
            replica["Table"]["SSEDescription"]["KMSMasterKeyArn"].clone(),
            replica["Table"]["SSEDescription"]["Status"].clone(),
            local["Table"]["SSEDescription"].clone(),
        ),
        (
            json!(key_arn),
            json!(key_arn),
            json!("ENABLED"),
            Value::Null,
        ),
        "case a replica with a key: the replica is encrypted with it and the local table is \
         not, got {local} and {replica}"
    );

    // An identifier KMS does not resolve in the replica's Region refuses the whole update,
    // exactly as it does on the table's own specification — and creates no replica. The
    // second row is the same key ARN as above, which names a key of ANOTHER Region from
    // the Region this replica would live in.
    for (case, region, key) in [
        ("an alias no key carries", "eu-west-1", "alias/no-such-key"),
        ("a key of another Region", "eu-central-1", key_arn.as_str()),
    ] {
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": "ReplicaEncrypted",
                "ReplicaUpdates": [{"Create": {
                    "RegionName": region,
                    "KMSMasterKeyId": key,
                }}],
            }),
        )
        .await;
        let (absent_status, absent) = call_in(
            &router,
            region,
            "DescribeTable",
            json!({"TableName": "ReplicaEncrypted"}),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&body).as_str(),
                absent_status,
                error_code(&absent).as_str(),
            ),
            (400, "ValidationException", 400, "ResourceNotFoundException"),
            "case {case}: refused and nothing created, got {body} and {absent}"
        );
    }
}

/// The Region the replica of the encryption case lives in — another Region than [`REGION`],
/// because a replica is a table of a DIFFERENT Region than the one it replicates.
const REPLICA_REGION: &str = "us-west-2";

#[tokio::test]
async fn each_on_demand_ceiling_is_set_and_cleared_through_its_own_member() {
    let router = emulator_with_kms();
    create_table(&router, "Ceilings", "PAY_PER_REQUEST", json!({})).await;
    settle().await;
    assert_eq!(
        described_ceilings(&router, "Ceilings").await,
        json!({"MaxReadRequestUnits": -1, "MaxWriteRequestUnits": -1}),
        "case a table with no maximum: both dimensions report the no-maximum value"
    );

    // Rows differ in which member the request names and in what it asks of it: setting one
    // ceiling, setting the other, setting both at once, clearing one with -1, clearing the
    // other, re-setting after a clear, and naming no ceiling at all. Each row states the
    // pair in force after it, so the row that leaves a member out shows that member
    // standing.
    let cases: [(&str, Value, Value); 7] = [
        (
            "the read ceiling alone",
            json!({"MaxReadRequestUnits": 100}),
            json!({"MaxReadRequestUnits": 100, "MaxWriteRequestUnits": -1}),
        ),
        (
            "the write ceiling alone, leaving the read one standing",
            json!({"MaxWriteRequestUnits": 7}),
            json!({"MaxReadRequestUnits": 100, "MaxWriteRequestUnits": 7}),
        ),
        (
            "both ceilings at once",
            json!({"MaxReadRequestUnits": 5, "MaxWriteRequestUnits": 9}),
            json!({"MaxReadRequestUnits": 5, "MaxWriteRequestUnits": 9}),
        ),
        (
            "clearing the read ceiling",
            json!({"MaxReadRequestUnits": -1}),
            json!({"MaxReadRequestUnits": -1, "MaxWriteRequestUnits": 9}),
        ),
        (
            "re-setting the read ceiling at the documented minimum",
            json!({"MaxReadRequestUnits": 1}),
            json!({"MaxReadRequestUnits": 1, "MaxWriteRequestUnits": 9}),
        ),
        (
            "clearing both ceilings",
            json!({"MaxReadRequestUnits": -1, "MaxWriteRequestUnits": -1}),
            json!({"MaxReadRequestUnits": -1, "MaxWriteRequestUnits": -1}),
        ),
        (
            "a ceiling re-set after both were cleared",
            json!({"MaxWriteRequestUnits": 40}),
            json!({"MaxReadRequestUnits": -1, "MaxWriteRequestUnits": 40}),
        ),
    ];

    for (case, requested, expected) in cases {
        let updated = ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({"TableName": "Ceilings", "OnDemandThroughput": requested.clone()}),
        )
        .await;
        assert_eq!(
            (
                updated["TableDescription"]["OnDemandThroughput"].clone(),
                updated["TableDescription"]["TableStatus"].clone(),
            ),
            (expected.clone(), json!("ACTIVE")),
            "case {case}: the answer reports the pair in force, got {updated} for {requested}"
        );
        assert_eq!(
            described_ceilings(&router, "Ceilings").await,
            expected,
            "case {case}: a later read reports the same pair"
        );
    }
}

#[tokio::test]
async fn an_on_demand_ceiling_value_the_member_does_not_take_is_refused() {
    let router = emulator_with_kms();
    create_table(&router, "CeilingValues", "PAY_PER_REQUEST", json!({})).await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": "CeilingValues",
            "OnDemandThroughput": {"MaxReadRequestUnits": 12, "MaxWriteRequestUnits": 13},
        }),
    )
    .await;

    // Rows differ in the value and the member it is named for: zero and a negative number
    // that is not the removal value are refused for either dimension, on the create as on
    // the update.
    let cases: [(&str, &str, Value); 5] = [
        (
            "zero for the read ceiling",
            "UpdateTable",
            json!({"TableName": "CeilingValues", "OnDemandThroughput": {"MaxReadRequestUnits": 0}}),
        ),
        (
            "zero for the write ceiling",
            "UpdateTable",
            json!({
                "TableName": "CeilingValues",
                "OnDemandThroughput": {"MaxWriteRequestUnits": 0},
            }),
        ),
        (
            "a negative number that is not the removal value",
            "UpdateTable",
            json!({
                "TableName": "CeilingValues",
                "OnDemandThroughput": {"MaxReadRequestUnits": -2},
            }),
        ),
        (
            "the same value on a create",
            "CreateTable",
            json!({
                "TableName": "CeilingValuesCreated",
                "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
                "OnDemandThroughput": {"MaxWriteRequestUnits": 0},
            }),
        ),
        (
            "both members at once, one of them invalid",
            "UpdateTable",
            json!({
                "TableName": "CeilingValues",
                "OnDemandThroughput": {"MaxReadRequestUnits": 10, "MaxWriteRequestUnits": -5},
            }),
        ),
    ];

    for (case, operation, request) in cases {
        let (status, body) = call_in(&router, REGION, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case} on {operation}: the value is not one the member takes, got {status} \
             with {body} for {request}"
        );
    }

    // Nothing moved, and the refused create stored no table.
    let (status, absent) = call_in(
        &router,
        REGION,
        "DescribeTable",
        json!({"TableName": "CeilingValuesCreated"}),
    )
    .await;
    assert_eq!(
        (
            described_ceilings(&router, "CeilingValues").await,
            (status, error_code(&absent)),
        ),
        (
            json!({"MaxReadRequestUnits": 12, "MaxWriteRequestUnits": 13}),
            (400, "ResourceNotFoundException".to_string()),
        ),
        "case after the refusals: the stored pair is untouched and no table was created, got \
         {absent}"
    );
}
