// SPDX-License-Identifier: CC0-1.0

//! `UpdateInstance` end to end over the `awsJson1_1` wire.
//!
//! The rule under test: `UpdateInstance` writes exactly the updatable members the request
//! CARRIES (`Name`, `PermissionSetsEnabled`, `EncryptionConfiguration`) onto the stored
//! instance its `InstanceArn` names, leaves every member the request omits alone, and
//! every later read answers from that one stored record; a request that misses one of the
//! documented member rules is refused with the modelled error and writes nothing.
//!
//! The documented rules, each asserted over a TABLE rather than over one example:
//!
//! * `Name` — "Updates the instance name": any `NameType` value (`^[\w+=,.@-]+$`,
//!   length 0..255) round-trips through `DescribeInstance` and `ListInstances`;
//! * `PermissionSetsEnabled` — "The only accepted value is true … they cannot be
//!   disabled": `true` succeeds, `false` is refused;
//! * "A request that specifies both EncryptionConfiguration and PermissionSetsEnabled
//!   returns a ValidationException";
//! * `EncryptionConfiguration$KmsKeyArn` — "Required when KeyType is
//!   CUSTOMER_MANAGED_KEY. Cannot be specified when KeyType is AWS_OWNED_KMS_KEY";
//! * an `InstanceArn` no instance of this account carries is a missing resource
//!   (`ResourceNotFoundException`).
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints and
//! the `X-Amz-Target` routing are exercised rather than bypassed. The harness is the one
//! `ssoadmin_permission_sets.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_ssoadmin_swbexternalservice::state::{
    ENCRYPTION_UPDATE_DELAY, KEY_TYPE_AWS_OWNED, KEY_TYPE_CUSTOMER_MANAGED,
    KMS_KEY_STATUS_ENABLED, KMS_KEY_STATUS_UPDATING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router, seeded_instance_arn, UNKNOWN_INSTANCE_ARN,
};

/// A customer managed key ARN in the hyphenated-UUID spelling the model's `KmsKeyArn`
/// pattern admits:
/// `^arn:aws(-[a-z]{1,5}){0,3}:kms:([a-z]{2,}(-[a-z0-9]+)+){1}:[0-9]{12}:key/(mrk-[a-f0-9]{32}|[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$`.
const KEY_ARN_UUID: &str =
    "arn:aws:kms:us-east-1:123456789012:key/0a1b2c3d-4e5f-6a7b-8c9d-0e1f2a3b4c5d";

/// The same pattern's multi-Region spelling: `mrk-` and 32 of `[a-f0-9]`.
const KEY_ARN_MRK: &str =
    "arn:aws:kms:us-west-2:123456789012:key/mrk-0123456789abcdef0123456789abcdef";

/// The `DescribeInstance` body for `arn`, which must answer 200.
async fn describe(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "DescribeInstance", &json!({"InstanceArn": arn})).await;
    assert_eq!(
        status, 200,
        "DescribeInstance expected 200 for {arn}, got {status} {body}"
    );
    body
}

/// The `InstanceMetadata` `ListInstances` reports for `arn`.
async fn listed(router: &MultiServiceRouter, arn: &str) -> Value {
    let (status, body) = call(router, "ListInstances", &json!({})).await;
    assert_eq!(status, 200, "ListInstances expected 200, got {status} {body}");
    body["Instances"]
        .as_array()
        .unwrap_or_else(|| panic!("ListInstances returned no Instances: {body}"))
        .iter()
        .find(|instance| instance["InstanceArn"].as_str() == Some(arn))
        .unwrap_or_else(|| panic!("ListInstances does not list {arn}: {body}"))
        .clone()
}

/// The `Name` values a successful update must round-trip. The rows span what `NameType`
/// (`^[\w+=,.@-]+$`, length 0..255) generalises over: each character class the pattern
/// admits, the single-character minimum the pattern leaves, and the published 255
/// maximum.
fn accepted_names() -> Vec<(&'static str, String)> {
    vec![
        ("a plain word", "IdentityCenterProd".to_string()),
        ("one character", "x".to_string()),
        ("digits and underscores", "team_42_9".to_string()),
        (
            "every character the pattern admits",
            "a_Z9+=,.@-".to_string(),
        ),
        ("the name it already carries", "IdentityCenter".to_string()),
        // `NameType`'s published upper length bound.
        ("255 characters", "n".repeat(255)),
    ]
}

/// The rule for `Name`: whatever `NameType` value an update carries becomes the
/// instance's name, is reported by both readers, and leaves every other member of the
/// record untouched.
#[tokio::test]
async fn an_update_writes_the_name_and_both_readers_report_it() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;
    let before = describe(&router, &arn).await;

    for (case, name) in accepted_names() {
        let (status, body) =
            call(&router, "UpdateInstance", &json!({"InstanceArn": arn, "Name": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for name {name:?}, got {status} {body}"
        );
        // `UpdateInstanceResponse` models no members, so the update carries no payload.
        assert_eq!(
            body,
            json!({}),
            "case {case:?}: UpdateInstance models an empty response, got {body:?}"
        );

        let described = describe(&router, &arn).await;
        assert_eq!(
            described["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: DescribeInstance must report the written name, got {:?}",
            described["Name"]
        );
        let metadata = listed(&router, &arn).await;
        assert_eq!(
            metadata["Name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: ListInstances must report the written name, got {:?}",
            metadata["Name"]
        );

        // Renaming an instance changes ONLY its name.
        for member in [
            "InstanceArn",
            "IdentityStoreId",
            "IdentityStoreArn",
            "OwnerAccountId",
            "CreatedDate",
            "Status",
            "PrimaryRegion",
            "Regions",
            "PermissionSetsEnabled",
            "EncryptionConfigurationDetails",
        ] {
            assert_eq!(
                described[member], before[member],
                "case {case:?}: member {member} must be untouched by a name update, got {:?} \
                 vs {:?}",
                described[member], before[member]
            );
        }
    }
}

/// An update that carries no updatable member changes nothing — and repeating an update
/// with the same name is idempotent.
#[tokio::test]
async fn an_update_that_carries_nothing_changes_nothing() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let (status, _) = call(
        &router,
        "UpdateInstance",
        &json!({"InstanceArn": arn, "Name": "SteadyState"}),
    )
    .await;
    assert_eq!(status, 200, "the first update expected 200, got {status}");
    let after_first = describe(&router, &arn).await;

    for (case, request) in [
        ("only the InstanceArn", json!({"InstanceArn": arn})),
        (
            "the same name again",
            json!({"InstanceArn": arn, "Name": "SteadyState"}),
        ),
    ] {
        let (status, body) = call(&router, "UpdateInstance", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );
        let described = describe(&router, &arn).await;
        assert_eq!(
            described, after_first,
            "case {case:?}: the record must be unchanged, got {described} vs {after_first}"
        );
    }
}

/// `PermissionSetsEnabled` — "Enables permission sets for this Identity Center instance.
/// The only accepted value is true. After permission sets are enabled, they cannot be
/// disabled": the flag is written by `true`, is refused by `false`, and never goes back.
///
/// The rows walk the whole class of values the member can carry, each with the state the
/// instance must report afterwards: the enablement before any update, `true` (accepted,
/// and idempotent on a second call), and `false` (refused, and the flag stays on).
#[tokio::test]
async fn permission_sets_can_only_be_enabled() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    // The instance this account holds is an ACCOUNT instance, so permission sets start
    // off; whatever it starts at is what a request that names no member must leave.
    let initial = describe(&router, &arn).await["PermissionSetsEnabled"].clone();

    // (case, the PermissionSetsEnabled value to send — `None` sends the member at all,
    //  the expected HTTP status, the enablement the instance must report afterwards)
    let rows: [(&str, Option<bool>, u16, Value); 5] = [
        ("no member at all", None, 200, initial.clone()),
        ("disable before enabling", Some(false), 400, initial),
        ("enable", Some(true), 200, Value::Bool(true)),
        ("enable again", Some(true), 200, Value::Bool(true)),
        ("disable after enabling", Some(false), 400, Value::Bool(true)),
    ];

    for (case, enabled, expected_status, expected_flag) in rows {
        let mut request = json!({"InstanceArn": arn});
        if let Some(enabled) = enabled {
            request["PermissionSetsEnabled"] = json!(enabled);
        }
        let (status, body) = call(&router, "UpdateInstance", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
        let described = describe(&router, &arn).await;
        assert_eq!(
            described["PermissionSetsEnabled"], expected_flag,
            "case {case:?}: expected PermissionSetsEnabled {expected_flag:?}, got {:?}",
            described["PermissionSetsEnabled"]
        );
    }
}

/// Enabling permission sets is a real state change, not a cosmetic flag: the
/// permission-set surface this instance withdraws while they are disabled is served once
/// an `UpdateInstance` enables them, and `CreatePermissionSet` is the operation that
/// shows it.
#[tokio::test]
async fn enabling_permission_sets_opens_the_permission_set_surface() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    let request = json!({"InstanceArn": arn, "Name": "SurfaceProbe"});
    let (status, body) = call(&router, "CreatePermissionSet", &request).await;
    assert_eq!(
        status, 400,
        "before enabling: the surface is withdrawn, so CreatePermissionSet must be \
         refused, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "before enabling: expected ValidationException, got {body}"
    );

    let (status, body) = call(
        &router,
        "UpdateInstance",
        &json!({"InstanceArn": arn, "PermissionSetsEnabled": true}),
    )
    .await;
    assert_eq!(status, 200, "enabling expected 200, got {status} {body}");

    let (status, body) = call(&router, "CreatePermissionSet", &request).await;
    assert_eq!(
        status, 200,
        "after enabling: the surface is served, so CreatePermissionSet must succeed, \
         got {status} {body}"
    );
}

/// The encryption configurations an update accepts, and what `DescribeInstance` must then
/// report. The rows span both `KmsKeyType` values and both `KmsKeyArn` spellings the
/// model's pattern admits.
const ACCEPTED_ENCRYPTION: &[(&str, &str, Option<&str>)] = &[
    ("an Amazon Web Services owned key", KEY_TYPE_AWS_OWNED, None),
    (
        "a customer managed key, UUID spelling",
        KEY_TYPE_CUSTOMER_MANAGED,
        Some(KEY_ARN_UUID),
    ),
    (
        "a customer managed key, multi-Region spelling",
        KEY_TYPE_CUSTOMER_MANAGED,
        Some(KEY_ARN_MRK),
    ),
];

/// The rule for `EncryptionConfiguration`: an accepted configuration is STORED, so
/// `DescribeInstance` reports the key type and key ARN that were written — and the change
/// is asynchronous, so it reports `UPDATING` until the update is due and `ENABLED` after.
#[tokio::test]
async fn an_accepted_encryption_configuration_is_stored_and_reported() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;

    for (case, key_type, key_arn) in ACCEPTED_ENCRYPTION {
        let mut configuration = json!({"KeyType": key_type});
        if let Some(key_arn) = key_arn {
            configuration["KmsKeyArn"] = json!(key_arn);
        }
        let (status, body) = call(
            &router,
            "UpdateInstance",
            &json!({"InstanceArn": arn, "EncryptionConfiguration": configuration}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} {body}"
        );

        // The change is in flight the instant the update returns: the caller can see it.
        let details = describe(&router, &arn).await["EncryptionConfigurationDetails"].clone();
        assert_eq!(
            details["EncryptionStatus"].as_str(),
            Some(KMS_KEY_STATUS_UPDATING),
            "case {case:?}: a just-accepted key change is still updating, got {details:?}"
        );
        assert_eq!(
            details["KeyType"].as_str(),
            Some(*key_type),
            "case {case:?}: expected the written KeyType, got {details:?}"
        );
        assert_eq!(
            details["KmsKeyArn"].as_str(),
            *key_arn,
            "case {case:?}: expected the written KmsKeyArn, got {details:?}"
        );
        // "particularly useful when the encryption status is UPDATE_FAILED": nothing
        // failed here, so no reason may be invented.
        assert_eq!(
            details["EncryptionStatusReason"],
            Value::Null,
            "case {case:?}: no key change failed, so no EncryptionStatusReason may be \
             reported, got {details:?}"
        );

        // Once the change is due, the same stored record reports it enabled — with no
        // write of its own in between.
        // A blocking wait, not `tokio::time` (the workspace's tokio carries no `time`
        // feature): this test drives one call at a time, so there is nothing to yield to.
        std::thread::sleep(ENCRYPTION_UPDATE_DELAY + std::time::Duration::from_millis(100));
        let settled = describe(&router, &arn).await["EncryptionConfigurationDetails"].clone();
        assert_eq!(
            settled["EncryptionStatus"].as_str(),
            Some(KMS_KEY_STATUS_ENABLED),
            "case {case:?}: a due key change must report enabled, got {settled:?}"
        );
        assert_eq!(
            settled["KeyType"].as_str(),
            Some(*key_type),
            "case {case:?}: the stored KeyType must not drift, got {settled:?}"
        );
        assert_eq!(
            settled["KmsKeyArn"].as_str(),
            *key_arn,
            "case {case:?}: the stored KmsKeyArn must not drift, got {settled:?}"
        );
    }
}

/// An update that misses a documented member rule is refused with the modelled error and
/// writes NOTHING: the stored record is byte-identical before and after.
///
/// The rows span the refusals the operation's documentation names — the one-of rule, each
/// half of the `KeyType`/`KmsKeyArn` pairing, a `KmsKeyType` the model does not carry,
/// and an `InstanceArn` no instance answers for.
#[tokio::test]
async fn a_refused_update_writes_nothing() {
    let router = router();
    let arn = seeded_instance_arn(&router).await;
    let (status, _) = call(
        &router,
        "UpdateInstance",
        &json!({"InstanceArn": arn, "Name": "BeforeTheRefusals"}),
    )
    .await;
    assert_eq!(status, 200, "the setup update expected 200, got {status}");
    let before = describe(&router, &arn).await;

    // (case, request body, expected HTTP status, expected error code)
    let rows: Vec<(&str, Value, u16, &str)> = vec![
        (
            "both EncryptionConfiguration and PermissionSetsEnabled",
            json!({
                "InstanceArn": arn,
                "PermissionSetsEnabled": true,
                "EncryptionConfiguration": {"KeyType": KEY_TYPE_AWS_OWNED},
            }),
            400,
            "ValidationException",
        ),
        (
            "a customer managed key with no KmsKeyArn",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {"KeyType": KEY_TYPE_CUSTOMER_MANAGED},
            }),
            400,
            "ValidationException",
        ),
        (
            "an Amazon Web Services owned key with a KmsKeyArn",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {
                    "KeyType": KEY_TYPE_AWS_OWNED,
                    "KmsKeyArn": KEY_ARN_UUID,
                },
            }),
            400,
            "ValidationException",
        ),
        (
            "a KeyType the KmsKeyType enum does not carry",
            json!({
                "InstanceArn": arn,
                "EncryptionConfiguration": {"KeyType": "CUSTOMER_MANAGED_KEY_V2"},
            }),
            400,
            "ValidationException",
        ),
        (
            // `NameType`'s pattern `^[\w+=,.@-]+$` admits no space…
            "a Name outside the NameType pattern",
            json!({"InstanceArn": arn, "Name": "Identity Center"}),
            400,
            "ValidationException",
        ),
        (
            // …and the `+` quantifier admits no empty value, whatever the 0 minimum
            // length allows.
            "an empty Name",
            json!({"InstanceArn": arn, "Name": ""}),
            400,
            "ValidationException",
        ),
        (
            // One past `NameType`'s published 255 maximum.
            "a 256-character Name",
            json!({"InstanceArn": arn, "Name": "n".repeat(256)}),
            400,
            "ValidationException",
        ),
        (
            "a pattern-valid InstanceArn that was never created",
            json!({"InstanceArn": UNKNOWN_INSTANCE_ARN, "Name": "Orphan"}),
            // Every refusal of a request on this awsJson1_1 surface is HTTP 400; the
            // error code is what tells a missing instance from a bad one.
            400,
            "ResourceNotFoundException",
        ),
    ];

    for (case, request, expected_status, expected_code) in rows {
        let (status, body) = call(&router, "UpdateInstance", &request).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let after = describe(&router, &arn).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must write nothing, got {after} vs {before}"
        );
    }
}
