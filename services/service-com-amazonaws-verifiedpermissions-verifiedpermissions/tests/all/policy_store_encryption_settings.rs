// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `CreatePolicyStoreInput$encryptionSettings`: the request
//! member that selects which key a policy store and its child resources are
//! encrypted with.
//!
//! The rule under test: the encryption a create configures is the encryption the
//! store HOLDS — the request's union arm, with the key and the context it names,
//! is persisted and reported by a later read; the service's own default (an AWS
//! owned key) stands in only for a request that named no arm. A key form the
//! service never resolves is still the key the store was configured with, so the
//! read reports exactly what was configured.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the
//! emulator binary serves, over ONE shared engine, reusing the harness in
//! [`crate::harness`] — so a value is only visible to the later read if it really
//! reached state.

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions as service;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, error_code, fault_message, ok, router, router_with_engine, SIGNING_REGION,
};

/// `KmsKey`'s `@pattern`, as the model writes it — anchors included
/// (`smithy_query(query="shape", shape="KmsKey")`).
const KMS_KEY_PATTERN: &str = r"^[a-zA-Z0-9:/_-]+$";

/// A customer-managed KMS key ARN in the one account this emulator presents.
fn key_arn(key_id: &str) -> String {
    ::engine::emulator_arn("kms", SIGNING_REGION, &format!("key/{key_id}"))
}

/// Create a store from `request` and return its `policyStoreId`.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    ok(router, "CreatePolicyStore", request).await["policyStoreId"]
        .as_str()
        .expect("policyStoreId is required on the CreatePolicyStore response")
        .to_string()
}

/// The `encryptionState` `GetPolicyStore` reports for `store_id`.
async fn encryption_state(router: &MultiServiceRouter, store_id: &str) -> Value {
    ok(router, "GetPolicyStore", &json!({ "policyStoreId": store_id })).await["encryptionState"]
        .clone()
}

/// A `CreatePolicyStore` request body carrying `settings` as its
/// `encryptionSettings`, or omitting the member entirely when there are none.
fn create_request(settings: Option<&Value>) -> Value {
    let mut request = json!({ "validationSettings": { "mode": "OFF" } });
    if let Some(settings) = settings {
        request["encryptionSettings"] = settings.clone();
    }
    request
}

/// RULE: a create's `encryptionSettings` arm is persisted and reported back by
/// `GetPolicyStore` as the matching `encryptionState` arm; a create that names no
/// arm gets the service's default, an AWS owned key.
///
/// The rows span both arms of the union, the three forms the model's `key` doc
/// names (ARN, alias, bare id), and the size of the encryption context (member
/// absent, explicitly empty, one entry, several).
#[tokio::test]
async fn create_policy_store_persists_the_encryption_settings_it_was_given() {
    // The key forms come from `KmsEncryptionSettings$key`: "The customer-managed
    // KMS key Amazon Resource Name (ARN), alias or ID to be used for encryption
    // processes" (`smithy_query(query="shape", shape="KmsEncryptionSettings")`).
    let arn = key_arn("1234abcd-12ab-34cd-56ef-1234567890ab");
    let second_arn = key_arn("abcdef01-2345-6789-abcd-ef0123456789");

    // (case, the request's `encryptionSettings`, the `encryptionState` expected)
    let cases: Vec<(&str, Option<Value>, Value)> = vec![
        ("no encryptionSettings member", None, json!({ "default": {} })),
        (
            "the default arm, explicitly",
            Some(json!({ "default": {} })),
            json!({ "default": {} }),
        ),
        (
            "a key ARN and one context entry",
            Some(json!({ "kmsEncryptionSettings": {
                "key": arn,
                "encryptionContext": { "team": "photos" },
            }})),
            json!({ "kmsEncryptionState": {
                "key": arn,
                "encryptionContext": { "team": "photos" },
            }}),
        ),
        (
            // No context member at all: the store is configured with an EMPTY
            // context, which the read reports as the empty map that
            // `KmsEncryptionState$encryptionContext`, being `@required`, must
            // carry.
            "a bare key id and no context member",
            Some(json!({ "kmsEncryptionSettings": {
                "key": "1234abcd-12ab-34cd-56ef-1234567890ab",
            }})),
            json!({ "kmsEncryptionState": {
                "key": "1234abcd-12ab-34cd-56ef-1234567890ab",
                "encryptionContext": {},
            }}),
        ),
        (
            "a bare key id and an explicitly empty context",
            Some(json!({ "kmsEncryptionSettings": {
                "key": "0fedcba9-8765-4321-0fed-cba987654321",
                "encryptionContext": {},
            }})),
            json!({ "kmsEncryptionState": {
                "key": "0fedcba9-8765-4321-0fed-cba987654321",
                "encryptionContext": {},
            }}),
        ),
        (
            "an alias and several context entries",
            Some(json!({ "kmsEncryptionSettings": {
                "key": "alias/photo-app",
                "encryptionContext": { "team": "photos", "tier": "gold", "zone": "a" },
            }})),
            json!({ "kmsEncryptionState": {
                "key": "alias/photo-app",
                "encryptionContext": { "team": "photos", "tier": "gold", "zone": "a" },
            }}),
        ),
        (
            // A second ARN, so the reported key follows the request rather than
            // the first key any store was created with.
            "another key ARN",
            Some(json!({ "kmsEncryptionSettings": { "key": second_arn }})),
            json!({ "kmsEncryptionState": {
                "key": second_arn,
                "encryptionContext": {},
            }}),
        ),
    ];

    let router = router();
    for (case, settings, expected) in &cases {
        let store_id = create(&router, &create_request(settings.as_ref())).await;

        let created = encryption_state(&router, &store_id).await;
        assert_eq!(
            created, *expected,
            "case {case:?}: expected encryptionState {expected}, got {created}"
        );

        // The configuration no later request changes stays where it is: an
        // update of the store's mutable settings reports the same arm.
        ok(
            &router,
            "UpdatePolicyStore",
            &json!({ "policyStoreId": store_id, "validationSettings": { "mode": "OFF" },
                     "description": "updated" }),
        )
        .await;
        let after_update = encryption_state(&router, &store_id).await;
        assert_eq!(
            after_update, *expected,
            "case {case:?}: an update must not change encryptionState, got {after_update}"
        );
    }

    // Two stores created in the same service hold their OWN configuration: the
    // read is per record, not a service-wide setting.
    let with_key = create(
        &router,
        &create_request(Some(&json!({ "kmsEncryptionSettings": { "key": arn }}))),
    )
    .await;
    let without_key = create(&router, &create_request(None)).await;
    let first = encryption_state(&router, &with_key).await;
    let second = encryption_state(&router, &without_key).await;
    assert_ne!(
        first, second,
        "two stores configured differently must report different encryptionState, got {first} and {second}"
    );
}

/// RULE: an `encryptionSettings` value the model refuses is refused at the
/// boundary with a 400 `ValidationException`, before any store is created.
///
/// The rows span the constraint classes reachable inside the arm: the `@pattern`
/// on `key` (broken by a disallowed character, and by the empty string the `+`
/// quantifier excludes) and the absence of that `@required` member.
#[tokio::test]
async fn an_encryption_settings_value_the_model_refuses_creates_no_store() {
    // (case, the request's `encryptionSettings`, a fragment the message states)
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "a key breaking the pattern",
            json!({ "kmsEncryptionSettings": { "key": "not a key!" } }),
            format!(
                "Value at '/encryptionSettings/kmsEncryptionSettings/key' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {KMS_KEY_PATTERN}"
            ),
        ),
        (
            "an empty key",
            json!({ "kmsEncryptionSettings": { "key": "" } }),
            format!(
                "Value at '/encryptionSettings/kmsEncryptionSettings/key' failed to satisfy \
                 constraint: Member must satisfy regular expression pattern: {KMS_KEY_PATTERN}"
            ),
        ),
        (
            "the required key absent",
            json!({ "kmsEncryptionSettings": { "encryptionContext": { "team": "photos" } } }),
            "Value at '/encryptionSettings/kmsEncryptionSettings/key' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
    ];

    // The store count comes from state rather than from a list response: the
    // point of the assertion is that the refused create left NOTHING behind, so
    // it is read where a store would have been written.
    let (router, engine) = router_with_engine();
    let stored_count = || {
        let state =
            engine.state::<service::ComAmazonawsVerifiedpermissionsVerifiedpermissionsState>();
        let state = state.lock();
        state.policy_stores.len()
    };
    let before = stored_count();

    for (case, settings, fragment) in &cases {
        let (status, body) = call(
            &router,
            "CreatePolicyStore",
            &create_request(Some(settings)),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a 400 refusal, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        let message = fault_message(&body);
        assert!(
            message.contains(fragment.as_str()),
            "case {case:?}: expected a message stating {fragment:?}, got {message:?}"
        );
    }

    let after = stored_count();
    assert_eq!(
        after, before,
        "a refused create must not create a store: the store count moved from {before} to {after}"
    );
}

/// RULE: `encryptionSettings` is one of the parameters an `@idempotencyToken`
/// replay must repeat — the same token with the same settings answers with the
/// store the first call created, and the same token with different settings is a
/// 409 `ConflictException`.
///
/// The rows span the ways two settings differ: a different arm, a different key,
/// a different context value, an added context entry, and a dropped context.
#[tokio::test]
async fn a_replay_that_changes_the_encryption_settings_conflicts() {
    let arn = key_arn("1234abcd-12ab-34cd-56ef-1234567890ab");
    let kms = |key: &str, context: Value| {
        json!({ "kmsEncryptionSettings": { "key": key, "encryptionContext": context } })
    };

    // (case, the first call's settings, the replay's settings, whether the two
    //  are the same request)
    let cases: Vec<(&str, Option<Value>, Option<Value>, bool)> = vec![
        (
            "the same key and context",
            Some(kms(&arn, json!({ "team": "photos" }))),
            Some(kms(&arn, json!({ "team": "photos" }))),
            true,
        ),
        (
            "both absent",
            None,
            None,
            true,
        ),
        (
            "absent, then the default arm",
            None,
            Some(json!({ "default": {} })),
            false,
        ),
        (
            "the default arm, then a key",
            Some(json!({ "default": {} })),
            Some(kms(&arn, json!({}))),
            false,
        ),
        (
            "a different key",
            Some(kms(&arn, json!({}))),
            Some(kms(&key_arn("ffffffff-0000-1111-2222-333344445555"), json!({}))),
            false,
        ),
        (
            "a different context value",
            Some(kms(&arn, json!({ "team": "photos" }))),
            Some(kms(&arn, json!({ "team": "videos" }))),
            false,
        ),
        (
            "an added context entry",
            Some(kms(&arn, json!({ "team": "photos" }))),
            Some(kms(&arn, json!({ "team": "photos", "tier": "gold" }))),
            false,
        ),
        (
            "a dropped context",
            Some(kms(&arn, json!({ "team": "photos" }))),
            Some(kms(&arn, json!({}))),
            false,
        ),
    ];

    let router = router();
    for (index, (case, first, replay, same_request)) in cases.iter().enumerate() {
        let token = format!("token-{index}");
        let mut first_request = create_request(first.as_ref());
        first_request["clientToken"] = json!(token);
        let store_id = create(&router, &first_request).await;

        let mut replay_request = create_request(replay.as_ref());
        replay_request["clientToken"] = json!(token);
        let (status, body) = call(&router, "CreatePolicyStore", &replay_request).await;

        if *same_request {
            assert_eq!(
                status, 200,
                "case {case:?}: a replay of the same request must succeed, got {status} {body}"
            );
            assert_eq!(
                body["policyStoreId"].as_str().unwrap_or_default(),
                store_id,
                "case {case:?}: a replay must answer with the first store, got {body}"
            );
        } else {
            assert_eq!(
                status, 409,
                "case {case:?}: a replay with different settings must conflict, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ConflictException",
                "case {case:?}: expected a ConflictException, got {body}"
            );
        }
    }
}
