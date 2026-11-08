// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS faults a table's customer managed key raises: the one a
//! request naming a key KMS does not serve meets, and the one every data-plane request on
//! a table whose key KMS will not use meets.
//!
//! The rules these tests pin:
//!
//! > a DynamoDB request whose server-side encryption key KMS refuses is answered HTTP 400
//! > `ValidationException` whose message WRAPS the KMS fault — the fully qualified class of
//! > the KMS exception the attempt raised, that exception's own sentence, and the context
//! > of the KMS call — rather than a sentence of DynamoDB's own, so a client can tell a key
//! > that does not exist from one that is disabled from one being destroyed.
//!
//! > every data-plane request on a table encrypted with a customer managed key resolves
//! > that key's state against KMS first: while the key is not usable the request is refused
//! > and the store is left exactly as it was, a table on another, usable key and a table on
//! > the Amazon Web Services owned key are unaffected, and once the key is usable again the
//! > same requests succeed with every item written before unchanged.
//!
//! The rows of each table below differ along what the rule generalises over: the FORM the
//! unresolvable key identifier takes and the operation that carries it; the KEY STATE the
//! key is put into; and WHICH data-plane operation the request is.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{
    call, describe_when_active, emulator_with_kms, error_code, message, ok, ok_kms, REGION,
};

/// The package every KMS exception class a wrapped message names belongs to, from the
/// recorded refusals: the class name is what a client classifies the fault on.
const KMS_PACKAGE: &str = "com.amazonaws.services.kms.model";

/// How the `Proxy` field ends the context of the wrapped KMS call.
const CONTEXT_TAIL: &str = "; Proxy: null)";

/// Create a table in [`REGION`] keyed by a single `S` partition key `Id`, with the members
/// `extra` adds, and wait for it to serve items.
async fn table_with(router: &MultiServiceRouter, name: &str, extra: Value) -> Value {
    let mut request = json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    ok(router, "CreateTable", request).await;
    describe_when_active(router, name, name).await
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

/// The `KeyState` KMS reports for `key_id`.
async fn key_state(router: &MultiServiceRouter, key_id: &str) -> String {
    ok_kms(router, "DescribeKey", json!({"KeyId": key_id})).await["KeyMetadata"]["KeyState"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// An `SSESpecification` that encrypts a table with the KMS key `key`.
fn encrypted_with(key: &str) -> Value {
    json!({"SSESpecification": {"Enabled": true, "SSEType": "KMS", "KMSMasterKeyId": key}})
}

#[tokio::test]
async fn a_key_kms_does_not_serve_is_refused_with_the_kms_exception_it_raised() {
    let router = emulator_with_kms();
    table_with(&router, "KeyRefusals", Value::Null).await;

    // Rows differ in the FORM the identifier takes and in the OPERATION carrying it: every
    // identifier KMS resolves to nothing raises the same KMS exception, whichever way it
    // was spelled and whichever call named it.
    let cases: [(&str, &str, Value); 5] = [
        (
            "a key ARN of this Region that names no key",
            "CreateTable",
            json!(format!(
                "arn:aws:kms:{REGION}:000000000000:key/11111111-2222-3333-4444-555555555555"
            )),
        ),
        (
            "a bare key id that names no key",
            "CreateTable",
            json!("99999999-8888-7777-6666-555555555555"),
        ),
        (
            "an alias no key carries",
            "CreateTable",
            json!("alias/no-such-key"),
        ),
        (
            "a key ARN of ANOTHER Region",
            "CreateTable",
            json!("arn:aws:kms:eu-west-1:000000000000:key/11111111-2222-3333-4444-555555555555"),
        ),
        (
            "a key ARN of this Region that names no key, carried by an update",
            "UpdateTable",
            json!(format!(
                "arn:aws:kms:{REGION}:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
            )),
        ),
    ];

    for (case, operation, identifier) in cases {
        let key = identifier.as_str().unwrap_or_default().to_string();
        let mut request = encrypted_with(&key);
        request["TableName"] = match operation {
            "CreateTable" => json!(format!("Refused{}", case.len())),
            _ => json!("KeyRefusals"),
        };
        if operation == "CreateTable" {
            request["KeySchema"] = json!([{"AttributeName": "Id", "KeyType": "HASH"}]);
            request["AttributeDefinitions"] =
                json!([{"AttributeName": "Id", "AttributeType": "S"}]);
            request["BillingMode"] = json!("PAY_PER_REQUEST");
        }
        let (status, body) = call(&router, operation, request.clone()).await;
        let refusal = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a key KMS does not serve is a client error, got {status} with {body}"
        );
        let expected_head = format!(
            "KMS validation error: {KMS_PACKAGE}.NotFoundException: Key '{key}' does not exist \
             (Service: Kms, Status Code: 400, Request ID: "
        );
        assert!(
            refusal.starts_with(&expected_head) && refusal.ends_with(CONTEXT_TAIL),
            "case {case}: the refusal must wrap the KMS fault as {expected_head:?}…\
             {CONTEXT_TAIL:?}, got {refusal:?}"
        );
    }

    // The id of the KMS call whose fault is reported belongs to THAT call: two refusals of
    // the same request name two different calls.
    let repeated = encrypted_with("alias/no-such-key");
    let mut request = repeated.clone();
    request["TableName"] = json!("KeyRefusals");
    let (_, first) = call(&router, "UpdateTable", request.clone()).await;
    let (_, second) = call(&router, "UpdateTable", request).await;
    assert_ne!(
        message(&first),
        message(&second),
        "case two refusals of one request: each names its own KMS call, got {:?} twice",
        message(&first)
    );

    // Nothing was stored by any refusal: the table is still encrypted with the owned key,
    // which has no description at all.
    let described = ok(&router, "DescribeTable", json!({"TableName": "KeyRefusals"})).await;
    assert_eq!(
        described["Table"]["SSEDescription"],
        Value::Null,
        "case after the refusals: the table's encryption is untouched, got {described}"
    );
}

#[tokio::test]
async fn naming_a_key_kms_will_not_use_refuses_the_call_that_names_it() {
    let router = emulator_with_kms();
    // Two keys, each put in a key state that refuses cryptographic operations: a table may
    // only be associated with a key its items can be encrypted under, so naming one of
    // these is refused wherever the identifier is carried.
    let (disabled_id, disabled_arn) = kms_key(&router).await;
    let (deleting_id, deleting_arn) = kms_key(&router).await;
    ok_kms(&router, "DisableKey", json!({"KeyId": disabled_id})).await;
    ok_kms(
        &router,
        "ScheduleKeyDeletion",
        json!({"KeyId": deleting_id, "PendingWindowInDays": 7}),
    )
    .await;
    table_with(&router, "UnusableKeyRefusals", Value::Null).await;

    // Rows differ in the KEY STATE and in the operation carrying the identifier.
    let cases: [(&str, &str, String, String); 4] = [
        (
            "CreateTable naming a disabled key",
            "CreateTable",
            disabled_arn.clone(),
            format!(
                "KMS key disabled error: {KMS_PACKAGE}.DisabledException: {disabled_arn} is \
                 disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; \
                 Request ID: "
            ),
        ),
        (
            "UpdateTable naming a disabled key",
            "UpdateTable",
            disabled_arn.clone(),
            format!(
                "KMS key disabled error: {KMS_PACKAGE}.DisabledException: {disabled_arn} is \
                 disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; \
                 Request ID: "
            ),
        ),
        (
            "CreateTable naming a key awaiting deletion",
            "CreateTable",
            deleting_arn.clone(),
            format!(
                "KMS validation error: {KMS_PACKAGE}.KMSInvalidStateException: {deleting_arn} is \
                 pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: \
                 KMSInvalidStateException; Request ID: "
            ),
        ),
        (
            "UpdateTable naming a key awaiting deletion",
            "UpdateTable",
            deleting_arn.clone(),
            format!(
                "KMS validation error: {KMS_PACKAGE}.KMSInvalidStateException: {deleting_arn} is \
                 pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: \
                 KMSInvalidStateException; Request ID: "
            ),
        ),
    ];

    for (case, operation, key, head) in cases {
        let mut request = encrypted_with(&key);
        if operation == "CreateTable" {
            request["TableName"] = json!(format!("Unusable{}", case.len()));
            request["KeySchema"] = json!([{"AttributeName": "Id", "KeyType": "HASH"}]);
            request["AttributeDefinitions"] =
                json!([{"AttributeName": "Id", "AttributeType": "S"}]);
            request["BillingMode"] = json!("PAY_PER_REQUEST");
        } else {
            request["TableName"] = json!("UnusableKeyRefusals");
        }
        let (status, body) = call(&router, operation, request).await;
        let refusal = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: a key KMS will not use is a client error, got {status} with {body}"
        );
        assert!(
            refusal.starts_with(&head) && refusal.ends_with(CONTEXT_TAIL),
            "case {case}: the refusal must wrap the KMS fault as {head:?}…{CONTEXT_TAIL:?}, got \
             {refusal:?}"
        );
    }

    // Nothing was associated: the table is still encrypted with the owned key.
    let described = ok(
        &router,
        "DescribeTable",
        json!({"TableName": "UnusableKeyRefusals"}),
    )
    .await;
    assert_eq!(
        described["Table"]["SSEDescription"],
        Value::Null,
        "case after the refusals: the table's encryption is untouched, got {described}"
    );
}

/// One data-plane request, as the refusal table below drives it: the operation and the
/// body it is sent with, keyed on the table name the row substitutes in.
struct DataPlaneCall {
    /// What the row is called in an assertion message.
    case: &'static str,
    operation: &'static str,
    /// The request body, built for the table `name`.
    body: fn(&str) -> Value,
}

/// Every data-plane request a table's key state is consulted for: each reads or writes the
/// table's ITEMS, which is what encryption covers, so each is refused while the key is not
/// usable. The rows span the single-item, the multi-item, the transactional and the PartiQL
/// surfaces, because the surface a read or a write arrives through is not what decides it.
const DATA_PLANE: &[DataPlaneCall] = &[
    DataPlaneCall {
        case: "GetItem",
        operation: "GetItem",
        body: |name| json!({"TableName": name, "Key": {"Id": {"S": "kept"}}, "ConsistentRead": true}),
    },
    DataPlaneCall {
        case: "PutItem",
        operation: "PutItem",
        body: |name| json!({"TableName": name, "Item": {"Id": {"S": "written-while-unusable"}}}),
    },
    DataPlaneCall {
        case: "UpdateItem",
        operation: "UpdateItem",
        body: |name| {
            json!({
                "TableName": name,
                "Key": {"Id": {"S": "kept"}},
                "UpdateExpression": "SET V = :v",
                "ExpressionAttributeValues": {":v": {"N": "99"}},
            })
        },
    },
    DataPlaneCall {
        case: "DeleteItem",
        operation: "DeleteItem",
        body: |name| json!({"TableName": name, "Key": {"Id": {"S": "kept"}}}),
    },
    DataPlaneCall {
        case: "Query",
        operation: "Query",
        body: |name| {
            json!({
                "TableName": name,
                "KeyConditionExpression": "Id = :id",
                "ExpressionAttributeValues": {":id": {"S": "kept"}},
            })
        },
    },
    DataPlaneCall {
        case: "Scan",
        operation: "Scan",
        body: |name| json!({"TableName": name}),
    },
    DataPlaneCall {
        case: "BatchGetItem",
        operation: "BatchGetItem",
        body: |name| json!({"RequestItems": {name: {"Keys": [{"Id": {"S": "kept"}}]}}}),
    },
    DataPlaneCall {
        case: "BatchWriteItem",
        operation: "BatchWriteItem",
        body: |name| {
            json!({
                "RequestItems": {
                    name: [{"PutRequest": {"Item": {"Id": {"S": "written-while-unusable"}}}}],
                },
            })
        },
    },
    DataPlaneCall {
        case: "TransactGetItems",
        operation: "TransactGetItems",
        body: |name| {
            json!({"TransactItems": [{"Get": {"TableName": name, "Key": {"Id": {"S": "kept"}}}}]})
        },
    },
    DataPlaneCall {
        case: "TransactWriteItems",
        operation: "TransactWriteItems",
        body: |name| {
            json!({
                "TransactItems": [{"Put": {
                    "TableName": name,
                    "Item": {"Id": {"S": "written-while-unusable"}},
                }}],
            })
        },
    },
    DataPlaneCall {
        case: "ExecuteStatement selecting an item",
        operation: "ExecuteStatement",
        body: |name| json!({"Statement": format!("SELECT * FROM \"{name}\" WHERE Id = 'kept'")}),
    },
    DataPlaneCall {
        case: "ExecuteStatement inserting an item",
        operation: "ExecuteStatement",
        body: |name| {
            json!({
                "Statement":
                    format!("INSERT INTO \"{name}\" VALUE {{'Id': 'written-while-unusable'}}"),
            })
        },
    },
    DataPlaneCall {
        case: "ExecuteTransaction",
        operation: "ExecuteTransaction",
        body: |name| {
            json!({
                "TransactStatements": [{
                    "Statement":
                        format!("INSERT INTO \"{name}\" VALUE {{'Id': 'written-while-unusable'}}"),
                }],
            })
        },
    },
];

/// Drive every row of [`DATA_PLANE`] against `name` and assert each is refused with the
/// wrapped KMS fault `head`…[`CONTEXT_TAIL`]. `state` names the key state under test.
async fn assert_data_plane_refused(
    router: &MultiServiceRouter,
    state: &str,
    name: &str,
    head: &str,
) {
    for row in DATA_PLANE {
        let (status, body) = call(router, row.operation, (row.body)(name)).await;
        let refusal = message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {state} / {}: the table's key refuses the request, got {status} with {body}",
            row.case
        );
        assert!(
            refusal.starts_with(head) && refusal.ends_with(CONTEXT_TAIL),
            "case {state} / {}: the refusal must wrap the KMS fault as {head:?}…\
             {CONTEXT_TAIL:?}, got {refusal:?}",
            row.case
        );
    }
}

/// The `Item` a `GetItem` of `key` reports for `name`, which is `Null` when the table
/// holds none.
async fn stored_item(router: &MultiServiceRouter, name: &str, key: &str) -> Value {
    ok(
        router,
        "GetItem",
        json!({"TableName": name, "Key": {"Id": {"S": key}}, "ConsistentRead": true}),
    )
    .await["Item"]
        .clone()
}

#[tokio::test]
async fn the_data_plane_is_refused_while_the_table_s_key_is_not_usable() {
    let router = emulator_with_kms();
    let (target_id, target_arn) = kms_key(&router).await;
    let (_, control_arn) = kms_key(&router).await;

    // Three tables: the one whose key the rows below move, one on ANOTHER usable key, and
    // one on the Amazon Web Services owned key, which names no customer key at all.
    table_with(&router, "Target", encrypted_with(&target_arn)).await;
    table_with(&router, "Control", encrypted_with(&control_arn)).await;
    table_with(&router, "Owned", Value::Null).await;
    for name in ["Target", "Control", "Owned"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Id": {"S": "kept"}, "V": {"N": "1"}}}),
        )
        .await;
        assert_eq!(
            stored_item(&router, name, "kept").await["V"],
            json!({"N": "1"}),
            "case before any key moves: {name} serves the item it was written"
        );
    }

    // Rows differ in the KEY STATE the table's key is in, which is what decides WHICH KMS
    // exception the refusal wraps and how DynamoDB introduces it.
    let states: [(&str, &str, Value, &str, &str, &str); 3] = [
        (
            "Disabled",
            "DisableKey",
            json!({"KeyId": target_id.clone()}),
            "Disabled",
            "KMS key disabled error",
            "DisabledException",
        ),
        (
            "PendingDeletion",
            "ScheduleKeyDeletion",
            json!({"KeyId": target_id.clone(), "PendingWindowInDays": 7}),
            "PendingDeletion",
            "KMS validation error",
            "KMSInvalidStateException",
        ),
        // "CancelKeyDeletion ... the key state ... changes to Disabled": cancelling a
        // scheduled deletion does not re-enable the key, so the table is still refused —
        // under the DISABLED fault now, which is the recoverable one.
        (
            "Disabled again after CancelKeyDeletion",
            "CancelKeyDeletion",
            json!({"KeyId": target_id.clone()}),
            "Disabled",
            "KMS key disabled error",
            "DisabledException",
        ),
    ];

    for (case, operation, body, expected_state, prefix, class) in states {
        ok_kms(&router, operation, body).await;
        assert_eq!(
            key_state(&router, &target_id).await,
            expected_state,
            "case {case}: KMS must report the key in that state before the data plane is \
             driven"
        );
        // The sentence is KMS's own — "<key arn> is <that key state, as prose>." — and the
        // class and the prefix are what the two faults are told apart by.
        let prose = if expected_state == "PendingDeletion" {
            "is pending deletion."
        } else {
            "is disabled."
        };
        let head = format!(
            "{prefix}: {KMS_PACKAGE}.{class}: {target_arn} {prose} (Service: AWSKMS; Status \
             Code: 400; Error Code: {class}; Request ID: "
        );
        assert_data_plane_refused(&router, case, "Target", &head).await;

        // A table on another, usable key and a table on the owned key are untouched by
        // this key's state: the gate is the TABLE's own key.
        for unaffected in ["Control", "Owned"] {
            assert_eq!(
                stored_item(&router, unaffected, "kept").await["V"],
                json!({"N": "1"}),
                "case {case}: {unaffected} is on another key and still serves its item"
            );
            ok(
                &router,
                "PutItem",
                json!({"TableName": unaffected, "Item": {"Id": {"S": "kept"}, "V": {"N": "1"}}}),
            )
            .await;
        }
    }

    // Enabling the key again restores every request it refused, and the store is exactly
    // what it was: the item written before the key moved is intact, and no write the gate
    // refused landed.
    ok_kms(&router, "EnableKey", json!({"KeyId": target_id.clone()})).await;
    assert_eq!(
        key_state(&router, &target_id).await,
        "Enabled",
        "case after EnableKey: KMS reports the key usable again"
    );
    assert_eq!(
        stored_item(&router, "Target", "kept").await["V"],
        json!({"N": "1"}),
        "case after EnableKey: the item written before the key moved is intact"
    );
    assert_eq!(
        stored_item(&router, "Target", "written-while-unusable").await,
        Value::Null,
        "case after EnableKey: no write the key state refused persisted anything"
    );
    ok(
        &router,
        "PutItem",
        json!({"TableName": "Target", "Item": {"Id": {"S": "after"}, "V": {"N": "2"}}}),
    )
    .await;
    assert_eq!(
        stored_item(&router, "Target", "after").await["V"],
        json!({"N": "2"}),
        "case after EnableKey: writes are served again"
    );
}

#[tokio::test]
async fn a_batch_statement_slot_reports_the_key_fault_of_its_own_table() {
    let router = emulator_with_kms();
    let (target_id, target_arn) = kms_key(&router).await;
    table_with(&router, "BatchTarget", encrypted_with(&target_arn)).await;
    table_with(&router, "BatchOwned", Value::Null).await;
    for name in ["BatchTarget", "BatchOwned"] {
        ok(
            &router,
            "PutItem",
            json!({"TableName": name, "Item": {"Id": {"S": "kept"}, "V": {"N": "1"}}}),
        )
        .await;
    }
    ok_kms(&router, "DisableKey", json!({"KeyId": target_id})).await;

    // "The entire batch must consist of either read statements or write statements": both
    // slots are reads, one on the table whose key is disabled and one on a table that has
    // no customer key — so the batch reports the KMS fault in the slot that earned it and
    // serves the other.
    let answered = ok(
        &router,
        "BatchExecuteStatement",
        json!({
            "Statements": [
                {"Statement": "SELECT * FROM \"BatchTarget\" WHERE Id = 'kept'"},
                {"Statement": "SELECT * FROM \"BatchOwned\" WHERE Id = 'kept'"},
            ],
        }),
    )
    .await;
    let slots = answered["Responses"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        slots.len(),
        2,
        "case a batch over two tables: one slot per statement, got {answered}"
    );
    let head = format!(
        "KMS key disabled error: {KMS_PACKAGE}.DisabledException: {target_arn} is disabled. \
         (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: "
    );
    let refusal = slots[0]["Error"]["Message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        slots[0]["Error"]["Code"],
        json!("ValidationError"),
        "case the slot on the disabled key: it is refused on its own merits, got {}",
        slots[0]
    );
    assert!(
        refusal.starts_with(&head) && refusal.ends_with(CONTEXT_TAIL),
        "case the slot on the disabled key: the refusal must wrap the KMS fault as \
         {head:?}…{CONTEXT_TAIL:?}, got {refusal:?}"
    );
    assert_eq!(
        slots[1]["Item"]["V"],
        json!({"N": "1"}),
        "case the slot on the owned key: it is served, got {}",
        slots[1]
    );
    // The REFUSED slot read nothing it could report, and the request itself was not a
    // failure: a client reads each slot on its own.
    assert_eq!(
        call(&router, "GetItem", json!({"TableName": "BatchOwned",
            "Key": {"Id": {"S": "kept"}}, "ConsistentRead": true}))
        .await
        .0,
        200,
        "case the table with no customer key: its data plane is never gated"
    );
}
