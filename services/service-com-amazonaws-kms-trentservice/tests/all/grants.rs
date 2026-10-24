// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the grants a KMS key carries: what `CreateGrant` persists,
//! and what `ListGrants` reports back, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rules asserted here, each over a TABLE rather than one call:
//!
//! 1. `CreateGrant` persists the grant the request describes and answers its id and a
//!    grant token; every member round-trips through `ListGrants`.
//! 2. A request carrying a `Name` is idempotent — a retry with identical parameters
//!    answers the original grant id and creates no second grant — while a request
//!    without one creates a new grant every time. The grant token is fresh either way.
//! 3. `DryRun` refuses the request it would otherwise have performed, and persists
//!    nothing; a request that would have failed for its own reason still fails that
//!    way.
//! 4. The `KeyId` is resolved the way every other operation resolves one, and the
//!    key's key state decides whether a grant may be created on it.
//! 5. The permitted `Operations` must be ones the key supports, which follows from its
//!    `KeyUsage` and the kind of key material its `KeySpec` gives it.
//! 6. A grant principal must be given as an ARN, and the grantee/retiring members must
//!    hold together.
//! 7. A grant token the service never minted is not valid.
//! 8. `ListGrants` pages over a key's grants in creation order, filtered as asked.
//! 9. `RevokeGrant` and `RetireGrant` delete the grant the request identifies, so its
//!    permissions — and the grant tokens that named it — are gone afterwards.
//! 10. `ListRetirableGrants` selects, across every key, the grants whose retiring
//!     principal the request names.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    assert_empty, b64, create_alias, create_key, describe, enabled_key, failure, is_lower_hex, ok,
    router, text,
};

/// The length of a grant id: KMS gives a grant 64 lowercase hexadecimal characters.
const GRANT_ID_LEN: usize = 64;

/// A principal ARN in the emulator's own account, which is the form KMS requires of a
/// grantee or retiring principal.
fn principal(resource: &str) -> String {
    format!(
        "arn:aws:iam::{account}:{resource}",
        account = engine::emulator_account_id()
    )
}

/// The `IssuingAccount` every grant created here is issued under: the account that
/// owns the keys.
fn issuing_account() -> String {
    principal("root")
}

/// An encryption-context map as awsJson1_1 carries one.
fn context(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// Create a grant, requiring success, and answer its `GrantId` and `GrantToken`.
async fn create_grant(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
) -> (String, String) {
    let answer = ok(router, "CreateGrant", request).await;
    (
        text(case, &answer, "GrantId"),
        text(case, &answer, "GrantToken"),
    )
}

/// The `Grants` `ListGrants` reports for a request, in the order reported.
async fn list_grants(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<Value> {
    let answer = ok(router, "ListGrants", request).await;
    answer["Grants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
        .clone()
}

/// The single grant `ListGrants` reports for `grant_id` on `key_id`.
async fn grant_entry(
    router: &MultiServiceRouter,
    case: &str,
    key_id: &str,
    grant_id: &str,
) -> Value {
    let listed = list_grants(
        router,
        case,
        &json!({"KeyId": key_id, "GrantId": grant_id}),
    )
    .await;
    assert_eq!(
        listed.len(),
        1,
        "case {case}: expected exactly the grant {grant_id} on {key_id}, got {listed:?}"
    );
    listed[0].clone()
}

/// One row of the round-trip table: the grant to create, described member by member.
struct RoundTripCase {
    case: &'static str,
    /// The `Name`, or `None` to leave the member out.
    name: Option<&'static str>,
    /// The resource part of the grantee principal's ARN.
    grantee: &'static str,
    /// The resource part of the retiring principal's ARN, or `None`.
    retiring: Option<&'static str>,
    operations: &'static [&'static str],
    /// `Constraints.EncryptionContextEquals`; empty leaves it out.
    equals: &'static [(&'static str, &'static str)],
    /// `Constraints.EncryptionContextSubset`; empty leaves it out.
    subset: &'static [(&'static str, &'static str)],
}

/// Grants that differ along everything `CreateGrant` lets a request vary: whether a
/// `Name` and a retiring principal are given, how many and which operations are
/// permitted, and which of the two encryption-context constraints are set.
///
/// Every operation named here is one a `SYMMETRIC_DEFAULT` encryption key supports
/// (the key these rows are created against); which operations a key supports is the
/// subject of its own table below. The names satisfy `GrantNameType`'s modelled
/// `^[a-zA-Z0-9:/_-]+$` (`smithy_query(query="operation",operation="CreateGrant")`).
const ROUND_TRIP_CASES: &[RoundTripCase] = &[
    RoundTripCase {
        case: "one operation, nothing optional",
        name: None,
        grantee: "role/Reader",
        retiring: None,
        operations: &["Decrypt"],
        equals: &[],
        subset: &[],
    },
    RoundTripCase {
        case: "named, with a retiring principal",
        name: Some("named-grant"),
        grantee: "role/Writer",
        retiring: Some("role/Retirer"),
        operations: &["Encrypt", "Decrypt"],
        equals: &[],
        subset: &[],
    },
    RoundTripCase {
        case: "every operation a symmetric encryption key supports",
        name: Some("all/the:operations_1-2"),
        grantee: "user/Bob",
        retiring: Some("root"),
        operations: &[
            "CreateGrant",
            "Decrypt",
            "DescribeKey",
            "Encrypt",
            "GenerateDataKey",
            "GenerateDataKeyPair",
            "GenerateDataKeyPairWithoutPlaintext",
            "GenerateDataKeyWithoutPlaintext",
            "ReEncryptFrom",
            "ReEncryptTo",
            "RetireGrant",
        ],
        equals: &[],
        subset: &[],
    },
    RoundTripCase {
        case: "an EncryptionContextEquals constraint",
        name: None,
        grantee: "role/Exact",
        retiring: None,
        operations: &["Decrypt"],
        equals: &[("Department", "Finance")],
        subset: &[],
    },
    RoundTripCase {
        case: "an EncryptionContextSubset constraint of two pairs",
        name: Some("subset-grant"),
        grantee: "role/Subset",
        retiring: None,
        operations: &["Encrypt", "GenerateDataKey"],
        equals: &[],
        subset: &[("Department", "Finance"), ("Project", "Ledger")],
    },
    RoundTripCase {
        case: "both encryption-context constraints",
        name: None,
        grantee: "role/Both",
        retiring: Some("user/Alice"),
        operations: &["Decrypt"],
        equals: &[("a", "1")],
        subset: &[("b", "2")],
    },
];

/// The `CreateGrant` request a round-trip row describes, against `key_id`.
fn round_trip_request(row: &RoundTripCase, key_id: &str) -> Value {
    let mut request = json!({
        "KeyId": key_id,
        "GranteePrincipal": principal(row.grantee),
        "Operations": row.operations,
    });
    if let Some(name) = row.name {
        request["Name"] = json!(name);
    }
    if let Some(retiring) = row.retiring {
        request["RetiringPrincipal"] = json!(principal(retiring));
    }
    let mut constraints = Map::new();
    if !row.equals.is_empty() {
        constraints.insert("EncryptionContextEquals".to_string(), context(row.equals));
    }
    if !row.subset.is_empty() {
        constraints.insert("EncryptionContextSubset".to_string(), context(row.subset));
    }
    if !constraints.is_empty() {
        request["Constraints"] = Value::Object(constraints);
    }
    request
}

/// Rule 1: every member of a created grant is persisted and read back.
#[tokio::test]
async fn a_created_grant_round_trips_through_list_grants() {
    let router = router();
    let key_id = text("round trip", &enabled_key(&router).await, "KeyId");
    // KMS reports a grant's key by its ARN, so that is what the entry must carry.
    let key_arn = text("round trip", &describe(&router, &key_id).await, "Arn");
    let mut created = Vec::new();

    for row in ROUND_TRIP_CASES {
        let case = row.case;
        let request = round_trip_request(row, &key_id);
        let (grant_id, grant_token) = create_grant(&router, case, &request).await;

        assert!(
            grant_id.len() == GRANT_ID_LEN && is_lower_hex(&grant_id),
            "case {case}: expected a {GRANT_ID_LEN}-character lowercase hex GrantId, got \
             {grant_id:?}"
        );
        assert!(
            !grant_token.is_empty() && grant_token != grant_id,
            "case {case}: expected a grant token distinct from the grant id, got \
             {grant_token:?}"
        );

        let entry = grant_entry(&router, case, &key_id, &grant_id).await;
        assert_eq!(
            entry["GrantId"],
            json!(grant_id),
            "case {case}: expected the listed grant to be {grant_id}, got {entry}"
        );
        assert_eq!(
            entry["KeyId"],
            json!(key_arn),
            "case {case}: expected the grant to name key {key_arn}, got {entry}"
        );
        assert_eq!(
            entry["IssuingAccount"],
            json!(issuing_account()),
            "case {case}: expected IssuingAccount {}, got {entry}",
            issuing_account()
        );
        assert_eq!(
            entry["Operations"],
            json!(row.operations),
            "case {case}: expected Operations {:?}, got {entry}",
            row.operations
        );
        assert_eq!(
            entry["GranteePrincipal"],
            json!(principal(row.grantee)),
            "case {case}: expected GranteePrincipal {}, got {entry}",
            principal(row.grantee)
        );
        assert_eq!(
            entry["Name"],
            row.name.map_or(Value::Null, |name| json!(name)),
            "case {case}: expected Name {:?}, got {entry}",
            row.name
        );
        assert_eq!(
            entry["RetiringPrincipal"],
            row.retiring
                .map_or(Value::Null, |retiring| json!(principal(retiring))),
            "case {case}: expected RetiringPrincipal {:?}, got {entry}",
            row.retiring
        );
        assert!(
            entry["CreationDate"].is_number(),
            "case {case}: expected a CreationDate timestamp, got {entry}"
        );
        assert!(
            entry["GranteeServicePrincipal"].is_null(),
            "case {case}: expected no GranteeServicePrincipal, got {entry}"
        );

        if row.equals.is_empty() && row.subset.is_empty() {
            assert!(
                entry["Constraints"].is_null(),
                "case {case}: expected no Constraints, got {entry}"
            );
        } else {
            assert_eq!(
                entry["Constraints"]["EncryptionContextEquals"],
                if row.equals.is_empty() {
                    Value::Null
                } else {
                    context(row.equals)
                },
                "case {case}: expected EncryptionContextEquals {:?}, got {entry}",
                row.equals
            );
            assert_eq!(
                entry["Constraints"]["EncryptionContextSubset"],
                if row.subset.is_empty() {
                    Value::Null
                } else {
                    context(row.subset)
                },
                "case {case}: expected EncryptionContextSubset {:?}, got {entry}",
                row.subset
            );
        }

        created.push(grant_id);
    }

    // Every grant is still there, in the order it was created: a later create never
    // reshuffles or drops an earlier grant.
    let listed = list_grants(&router, "all grants", &json!({"KeyId": key_id})).await;
    let listed_ids: Vec<String> = listed
        .iter()
        .map(|entry| text("all grants", entry, "GrantId"))
        .collect();
    assert_eq!(
        listed_ids, created,
        "case all grants: expected the grants in creation order {created:?}, got {listed_ids:?}"
    );
}

/// Rule 2: a `Name` makes the request idempotent; without one every call creates a
/// grant.
#[tokio::test]
async fn a_name_makes_create_grant_idempotent() {
    let router = router();
    let grantee = principal("role/Idempotent");
    let other = principal("role/Other");

    // (case, first request, second request, whether the two share one grant id). Each
    // row runs against its own key, so the grant count is the row's alone.
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "named, identical parameters",
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            true,
        ),
        (
            "named, the operations in another order",
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt", "Encrypt"]}),
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Encrypt", "Decrypt"]}),
            true,
        ),
        (
            "named, the same encryption context in another order",
            json!({
                "Name": "retry",
                "GranteePrincipal": grantee,
                "Operations": ["Decrypt"],
                "Constraints": {"EncryptionContextEquals": {"a": "1", "b": "2"}},
            }),
            json!({
                "Name": "retry",
                "GranteePrincipal": grantee,
                "Operations": ["Decrypt"],
                "Constraints": {"EncryptionContextEquals": {"b": "2", "a": "1"}},
            }),
            true,
        ),
        (
            "named, another grantee",
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            json!({"Name": "retry", "GranteePrincipal": other, "Operations": ["Decrypt"]}),
            false,
        ),
        (
            "named, one more operation",
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            json!({"Name": "retry", "GranteePrincipal": grantee, "Operations": ["Decrypt", "Encrypt"]}),
            false,
        ),
        (
            "named, another constraint value",
            json!({
                "Name": "retry",
                "GranteePrincipal": grantee,
                "Operations": ["Decrypt"],
                "Constraints": {"EncryptionContextEquals": {"a": "1"}},
            }),
            json!({
                "Name": "retry",
                "GranteePrincipal": grantee,
                "Operations": ["Decrypt"],
                "Constraints": {"EncryptionContextEquals": {"a": "2"}},
            }),
            false,
        ),
        (
            "another name, identical parameters",
            json!({"Name": "retry-one", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            json!({"Name": "retry-two", "GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            false,
        ),
        (
            "unnamed, identical parameters",
            json!({"GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            json!({"GranteePrincipal": grantee, "Operations": ["Decrypt"]}),
            false,
        ),
    ];

    for (case, mut first, mut second, same) in rows {
        let key_id = text(case, &enabled_key(&router).await, "KeyId");
        first["KeyId"] = json!(key_id);
        second["KeyId"] = json!(key_id);

        let (first_id, first_token) = create_grant(&router, case, &first).await;
        let (second_id, second_token) = create_grant(&router, case, &second).await;

        assert_eq!(
            first_id == second_id,
            same,
            "case {case}: expected the second grant id to {} the first, got {first_id} then \
             {second_id}",
            if same { "match" } else { "differ from" }
        );
        assert_ne!(
            first_token, second_token,
            "case {case}: expected a fresh grant token per call, got {first_token} twice"
        );

        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        let expected = usize::from(!same) + 1;
        assert_eq!(
            listed.len(),
            expected,
            "case {case}: expected {expected} grants on the key, got {listed:?}"
        );
    }
}

/// Rule 3: `DryRun` reports the refusal a real call would have met, and writes
/// nothing.
#[tokio::test]
async fn dry_run_refuses_the_request_and_persists_nothing() {
    let router = router();

    // (case, the DryRun member or `None` to leave it out, whether to disable the key
    // first, the error code expected or `None` for success).
    let rows: &[(&str, Option<bool>, bool, Option<&str>)] = &[
        ("DryRun left out", None, false, None),
        ("DryRun false", Some(false), false, None),
        ("DryRun true", Some(true), false, Some("DryRunOperationException")),
        ("DryRun false on a disabled key", Some(false), true, Some("DisabledException")),
        (
            "DryRun true on a disabled key",
            Some(true),
            true,
            Some("DisabledException"),
        ),
    ];

    for (case, dry_run, disable, expected) in rows {
        let key_id = text(case, &enabled_key(&router).await, "KeyId");
        if *disable {
            ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;
        }
        let mut request = json!({
            "KeyId": key_id,
            "GranteePrincipal": principal("role/DryRun"),
            "Operations": ["Decrypt"],
        });
        if let Some(dry_run) = dry_run {
            request["DryRun"] = json!(dry_run);
        }

        match expected {
            Some(code) => {
                let (status, actual) = failure(&router, "CreateGrant", &request).await;
                assert_eq!(
                    (status, actual.as_str()),
                    (400, *code),
                    "case {case}: expected HTTP 400 {code}, got {status} {actual}"
                );
            }
            None => {
                create_grant(&router, case, &request).await;
            }
        }

        // The key is this row's own, so whatever is listed is what this row wrote.
        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        let held = usize::from(expected.is_none());
        assert_eq!(
            listed.len(),
            held,
            "case {case}: expected {held} grants persisted, got {listed:?}"
        );
    }
}

/// Rule 4: the `KeyId` is resolved like any other, and the key state decides whether
/// a grant may be created.
#[tokio::test]
async fn the_key_identifier_and_its_key_state_decide_the_answer() {
    let router = router();

    let enabled = text("setup", &enabled_key(&router).await, "KeyId");
    let enabled_arn = text("setup", &describe(&router, &enabled).await, "Arn");
    let reported_key = enabled_arn.clone();
    create_alias(&router, "setup", "alias/grantable", &enabled).await;

    let disabled = text("setup", &enabled_key(&router).await, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled})).await;

    let doomed = text("setup", &enabled_key(&router).await, "KeyId");
    ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": doomed, "PendingWindowInDays": 7}),
    )
    .await;

    // (case, the identifier the request carries, the error code expected or `None` for
    // success). Every accepted form names the same key, so a grant created through one
    // is a grant on that key.
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("a key id", enabled.clone(), None),
        ("a key ARN", enabled_arn, None),
        ("an alias of the key", "alias/grantable".to_string(), None),
        (
            "a key id nothing holds",
            "1234abcd-12ab-34cd-56ef-1234567890ab".to_string(),
            Some("NotFoundException"),
        ),
        (
            "an alias nothing holds",
            "alias/absent".to_string(),
            Some("NotFoundException"),
        ),
        (
            "an ARN that is not a KMS ARN",
            "arn:aws:kms:us-east-1".to_string(),
            Some("InvalidArnException"),
        ),
        ("a disabled key", disabled, Some("DisabledException")),
        (
            "a key pending deletion",
            doomed,
            Some("KMSInvalidStateException"),
        ),
    ];

    for (case, identifier, expected) in rows {
        let request = json!({
            "KeyId": identifier,
            "GranteePrincipal": principal("role/Resolved"),
            "Operations": ["Decrypt"],
        });
        match expected {
            Some(code) => {
                let (status, actual) = failure(&router, "CreateGrant", &request).await;
                assert_eq!(
                    (status, actual.as_str()),
                    (400, code),
                    "case {case}: expected HTTP 400 {code}, got {status} {actual}"
                );
            }
            None => {
                let (grant_id, _) = create_grant(&router, case, &request).await;
                let entry = grant_entry(&router, case, &enabled, &grant_id).await;
                assert_eq!(
                    entry["KeyId"],
                    json!(reported_key),
                    "case {case}: expected the grant to record key {reported_key}, got {entry}"
                );
            }
        }
    }
}

/// One row of the "the key must support the operation" table.
struct OperationCase {
    case: &'static str,
    key_spec: &'static str,
    key_usage: &'static str,
    operations: &'static [&'static str],
    /// `true` when the key supports every operation in the row.
    supported: bool,
}

/// Which `GrantOperation` values each kind of key supports.
///
/// The operation names are `smithy_query(shape="GrantOperation")`'s values; which key
/// a given one needs follows from the `KeyUsage` matrix
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`) and from
/// the documented examples, "you cannot create a grant for a symmetric encryption KMS
/// key that allows the Sign operation, or a grant for an asymmetric KMS key that
/// allows the GenerateDataKey operation"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`).
const OPERATION_CASES: &[OperationCase] = &[
    OperationCase {
        case: "a symmetric encryption key encrypts",
        key_spec: "SYMMETRIC_DEFAULT",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["Encrypt", "Decrypt", "ReEncryptFrom", "ReEncryptTo"],
        supported: true,
    },
    OperationCase {
        case: "a symmetric encryption key generates data keys",
        key_spec: "SYMMETRIC_DEFAULT",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["GenerateDataKey", "GenerateDataKeyWithoutPlaintext"],
        supported: true,
    },
    OperationCase {
        case: "a symmetric encryption key does not sign",
        key_spec: "SYMMETRIC_DEFAULT",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["Sign"],
        supported: false,
    },
    OperationCase {
        case: "a symmetric encryption key has no public half",
        key_spec: "SYMMETRIC_DEFAULT",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["GetPublicKey"],
        supported: false,
    },
    OperationCase {
        case: "one unsupported operation among supported ones is still refused",
        key_spec: "SYMMETRIC_DEFAULT",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["Encrypt", "GenerateMac", "Decrypt"],
        supported: false,
    },
    OperationCase {
        case: "an asymmetric encryption key encrypts",
        key_spec: "RSA_2048",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["Encrypt", "Decrypt", "GetPublicKey"],
        supported: true,
    },
    OperationCase {
        case: "an asymmetric key does not generate data keys",
        key_spec: "RSA_2048",
        key_usage: "ENCRYPT_DECRYPT",
        operations: &["GenerateDataKey"],
        supported: false,
    },
    OperationCase {
        case: "an asymmetric signing key signs",
        key_spec: "RSA_4096",
        key_usage: "SIGN_VERIFY",
        operations: &["Sign", "Verify", "GetPublicKey"],
        supported: true,
    },
    OperationCase {
        case: "an asymmetric signing key does not encrypt",
        key_spec: "RSA_4096",
        key_usage: "SIGN_VERIFY",
        operations: &["Encrypt"],
        supported: false,
    },
    OperationCase {
        case: "an HMAC key generates and verifies MACs",
        key_spec: "HMAC_256",
        key_usage: "GENERATE_VERIFY_MAC",
        operations: &["GenerateMac", "VerifyMac"],
        supported: true,
    },
    OperationCase {
        case: "an HMAC key does not encrypt",
        key_spec: "HMAC_256",
        key_usage: "GENERATE_VERIFY_MAC",
        operations: &["Encrypt"],
        supported: false,
    },
    OperationCase {
        case: "a key-agreement key derives a shared secret",
        key_spec: "ECC_NIST_P384",
        key_usage: "KEY_AGREEMENT",
        operations: &["DeriveSharedSecret", "GetPublicKey"],
        supported: true,
    },
    OperationCase {
        case: "a signing key does not derive a shared secret",
        key_spec: "ECC_NIST_P384",
        key_usage: "SIGN_VERIFY",
        operations: &["DeriveSharedSecret"],
        supported: false,
    },
    OperationCase {
        case: "key management is granted on any key",
        key_spec: "HMAC_512",
        key_usage: "GENERATE_VERIFY_MAC",
        operations: &["CreateGrant", "DescribeKey", "RetireGrant"],
        supported: true,
    },
];

/// Rule 5: an operation the key does not support is refused, and nothing is persisted.
#[tokio::test]
async fn the_granted_operations_must_be_supported_by_the_key() {
    let router = router();

    for row in OPERATION_CASES {
        let case = row.case;
        let metadata = create_key(
            &router,
            &json!({"KeySpec": row.key_spec, "KeyUsage": row.key_usage}),
        )
        .await;
        let key_id = text(case, &metadata, "KeyId");
        let request = json!({
            "KeyId": key_id,
            "GranteePrincipal": principal("role/Operator"),
            "Operations": row.operations,
        });

        if row.supported {
            let (grant_id, _) = create_grant(&router, case, &request).await;
            let entry = grant_entry(&router, case, &key_id, &grant_id).await;
            assert_eq!(
                entry["Operations"],
                json!(row.operations),
                "case {case}: expected Operations {:?}, got {entry}",
                row.operations
            );
        } else {
            let (status, actual) = failure(&router, "CreateGrant", &request).await;
            assert_eq!(
                (status, actual.as_str()),
                (400, "ValidationException"),
                "case {case}: expected HTTP 400 ValidationException, got {status} {actual}"
            );
            let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
            assert!(
                listed.is_empty(),
                "case {case}: expected no grant persisted, got {listed:?}"
            );
        }
    }
}

/// Rule 6: a principal is an ARN, and the grantee/retiring members hold together.
#[tokio::test]
async fn the_grant_principals_must_hold_together() {
    let router = router();
    let service_principal = "dynamodb.amazonaws.com";
    let source_arn = format!(
        "arn:aws:dynamodb:us-east-1:{account}:table/Orders",
        account = engine::emulator_account_id()
    );

    // (case, the members the request carries beside KeyId and Operations, the error
    // code expected or `None` for success).
    let rows: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "an IAM role ARN",
            json!({"GranteePrincipal": principal("role/App")}),
            None,
        ),
        (
            "an IAM user ARN",
            json!({"GranteePrincipal": principal("user/Bob")}),
            None,
        ),
        (
            "the account root ARN",
            json!({"GranteePrincipal": principal("root")}),
            None,
        ),
        (
            "an assumed-role ARN",
            json!({"GranteePrincipal": format!(
                "arn:aws:sts::{account}:assumed-role/App/session",
                account = engine::emulator_account_id(),
            )}),
            None,
        ),
        (
            "a retiring principal ARN",
            json!({
                "GranteePrincipal": principal("role/App"),
                "RetiringPrincipal": principal("role/Retirer"),
            }),
            None,
        ),
        (
            "a bare account id as the grantee",
            json!({"GranteePrincipal": engine::emulator_account_id()}),
            Some("InvalidArnException"),
        ),
        (
            "an IAM resource path as the grantee",
            json!({"GranteePrincipal": "role/App"}),
            Some("InvalidArnException"),
        ),
        (
            "a truncated ARN as the grantee",
            json!({"GranteePrincipal": "arn:aws:iam"}),
            Some("InvalidArnException"),
        ),
        (
            "an ARN with no account as the grantee",
            json!({"GranteePrincipal": "arn:aws:iam:::role/App"}),
            Some("InvalidArnException"),
        ),
        (
            "a bare account id as the retiring principal",
            json!({
                "GranteePrincipal": principal("role/App"),
                "RetiringPrincipal": engine::emulator_account_id(),
            }),
            Some("InvalidArnException"),
        ),
        (
            "neither grantee member",
            json!({}),
            Some("ValidationException"),
        ),
        (
            "both grantee members",
            json!({
                "GranteePrincipal": principal("role/App"),
                "GranteeServicePrincipal": service_principal,
            }),
            Some("ValidationException"),
        ),
        (
            "both retiring members",
            json!({
                "GranteePrincipal": principal("role/App"),
                "RetiringPrincipal": principal("role/Retirer"),
                "RetiringServicePrincipal": service_principal,
            }),
            Some("ValidationException"),
        ),
        (
            "a grantee service principal with no SourceArn",
            json!({
                "GranteeServicePrincipal": service_principal,
                "RetiringPrincipal": principal("role/Retirer"),
            }),
            Some("ValidationException"),
        ),
        (
            "a grantee service principal with no retiring principal",
            json!({
                "GranteeServicePrincipal": service_principal,
                "Constraints": {"SourceArn": source_arn},
            }),
            Some("ValidationException"),
        ),
        (
            "a grantee service principal with a SourceArn and a retiring principal",
            json!({
                "GranteeServicePrincipal": service_principal,
                "RetiringPrincipal": principal("role/Retirer"),
                "Constraints": {"SourceArn": source_arn},
            }),
            None,
        ),
    ];

    for (case, members, expected) in rows {
        let key_id = text(case, &enabled_key(&router).await, "KeyId");
        let mut request = json!({"KeyId": key_id, "Operations": ["Decrypt"]});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member.as_str()] = value.clone();
        }

        match expected {
            Some(code) => {
                let (status, actual) = failure(&router, "CreateGrant", &request).await;
                assert_eq!(
                    (status, actual.as_str()),
                    (400, code),
                    "case {case}: expected HTTP 400 {code}, got {status} {actual}"
                );
            }
            None => {
                let (grant_id, _) = create_grant(&router, case, &request).await;
                let entry = grant_entry(&router, case, &key_id, &grant_id).await;
                for member in [
                    "GranteePrincipal",
                    "GranteeServicePrincipal",
                    "RetiringPrincipal",
                ] {
                    assert_eq!(
                        entry[member],
                        request.get(member).cloned().unwrap_or(Value::Null),
                        "case {case}: expected {member} to round-trip, got {entry}"
                    );
                }
            }
        }
    }
}

/// Rule 7: only a grant token this service minted is valid.
#[tokio::test]
async fn a_grant_token_the_service_never_minted_is_refused() {
    let router = router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");
    let request = json!({
        "KeyId": key_id,
        "GranteePrincipal": principal("role/Tokens"),
        "Operations": ["Decrypt"],
    });
    let (_, minted) = create_grant(&router, "setup", &request).await;

    // (case, the GrantTokens the request carries, the error code expected or `None`).
    // `7465` is the hex of "te": a value that decodes but names no grant.
    let rows: Vec<(&str, Vec<String>, Option<&str>)> = vec![
        ("no tokens", vec![], None),
        ("a token this service minted", vec![minted.clone()], None),
        (
            "the same token twice",
            vec![minted.clone(), minted.clone()],
            None,
        ),
        (
            "a token that is not hexadecimal",
            vec!["not-a-grant-token".to_string()],
            Some("InvalidGrantTokenException"),
        ),
        (
            "a token that decodes to something else",
            vec!["7465".to_string()],
            Some("InvalidGrantTokenException"),
        ),
        (
            "one valid token and one that is not",
            vec![minted, "7465".to_string()],
            Some("InvalidGrantTokenException"),
        ),
    ];

    for (case, tokens, expected) in rows {
        let mut request = request.clone();
        if !tokens.is_empty() {
            request["GrantTokens"] = json!(tokens);
        }
        match expected {
            Some(code) => {
                let (status, actual) = failure(&router, "CreateGrant", &request).await;
                assert_eq!(
                    (status, actual.as_str()),
                    (400, code),
                    "case {case}: expected HTTP 400 {code}, got {status} {actual}"
                );
            }
            None => {
                create_grant(&router, case, &request).await;
            }
        }
    }

    // The key and its key state are judged before the tokens, so a key that cannot
    // carry a grant at all reports why rather than reporting a token problem.
    let disabled = text("key state first", &enabled_key(&router).await, "KeyId");
    ok(&router, "DisableKey", &json!({"KeyId": disabled})).await;
    let (status, actual) = failure(
        &router,
        "CreateGrant",
        &json!({
            "KeyId": disabled,
            "GranteePrincipal": principal("role/Tokens"),
            "Operations": ["Decrypt"],
            "GrantTokens": ["not-a-grant-token"],
        }),
    )
    .await;
    assert_eq!(
        (status, actual.as_str()),
        (400, "DisabledException"),
        "case key state first: expected the key state to be reported before the token, got \
         {status} {actual}"
    );
}

/// Rule 8: `ListGrants` pages over a key's grants in creation order, and filters.
#[tokio::test]
async fn list_grants_pages_and_filters() {
    let router = router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");
    let grantee = principal("role/Paged");
    let other = principal("role/Unpaged");

    // Seven grants in creation order, the last two for another grantee so the filter
    // has something to narrow.
    let mut created = Vec::new();
    for index in 0..7u32 {
        let principal = if index < 5 { &grantee } else { &other };
        let (grant_id, _) = create_grant(
            &router,
            "setup",
            &json!({
                "KeyId": key_id,
                "GranteePrincipal": principal,
                "Operations": ["Decrypt"],
            }),
        )
        .await;
        created.push(grant_id);
    }

    // Every page size, including one that divides the list exactly, one that does not,
    // and one larger than the list.
    for limit in [1usize, 2, 3, 7, 100] {
        let case = format!("Limit {limit}");
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut request = json!({"KeyId": key_id, "Limit": limit});
            if let Some(marker) = marker.as_deref() {
                request["Marker"] = json!(marker);
            }
            let answer = ok(&router, "ListGrants", &request).await;
            let page = answer["Grants"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case {case}: expected at most {limit} grants per page, got {}",
                page.len()
            );
            collected.extend(page.iter().map(|entry| text(&case, entry, "GrantId")));

            let truncated = answer["Truncated"].as_bool().unwrap_or_else(|| {
                panic!("case {case}: expected a Truncated flag in {answer}")
            });
            assert_eq!(
                truncated,
                collected.len() < created.len(),
                "case {case}: expected Truncated {} after {} of {} grants, got {answer}",
                collected.len() < created.len(),
                collected.len(),
                created.len()
            );
            if !truncated {
                assert!(
                    answer["NextMarker"].is_null(),
                    "case {case}: expected no NextMarker on the last page, got {answer}"
                );
                break;
            }
            marker = Some(text(&case, &answer, "NextMarker"));
        }
        assert_eq!(
            collected, created,
            "case {case}: expected every grant in creation order {created:?}, got {collected:?}"
        );
    }

    // The grantee filter narrows the list without disturbing its order.
    let filtered = list_grants(
        &router,
        "grantee filter",
        &json!({"KeyId": key_id, "GranteePrincipal": other}),
    )
    .await;
    let filtered_ids: Vec<String> = filtered
        .iter()
        .map(|entry| text("grantee filter", entry, "GrantId"))
        .collect();
    assert_eq!(
        filtered_ids,
        created[5..].to_vec(),
        "case grantee filter: expected the two grants of {other}, got {filtered_ids:?}"
    );

    // A well-formed grant id that names no grant of this key selects nothing.
    let absent = list_grants(
        &router,
        "absent grant id",
        &json!({"KeyId": key_id, "GrantId": "a".repeat(64)}),
    )
    .await;
    assert!(
        absent.is_empty(),
        "case absent grant id: expected no grants, got {absent:?}"
    );

    // (case, the members the request carries beside KeyId, the error code expected).
    let refusals: Vec<(&str, Value, &str)> = vec![
        (
            "a marker from no page",
            json!({"Marker": "zz"}),
            "InvalidMarkerException",
        ),
        (
            "a marker from the key list",
            json!({"Marker": "6b657973"}),
            "InvalidMarkerException",
        ),
        (
            "a grant id that is not one",
            json!({"GrantId": "not-a-grant-id"}),
            "InvalidGrantIdException",
        ),
        (
            "a grant id in upper case",
            json!({"GrantId": "A".repeat(64)}),
            "InvalidGrantIdException",
        ),
        (
            "a Limit past the documented bound",
            json!({"Limit": 101}),
            "ValidationException",
        ),
        (
            "the largest Limit the shared LimitType allows",
            json!({"Limit": 1000}),
            "ValidationException",
        ),
        (
            "both grantee filters",
            json!({"GranteePrincipal": grantee, "GranteeServicePrincipal": "dynamodb.amazonaws.com"}),
            "ValidationException",
        ),
    ];

    for (case, members, code) in refusals {
        let mut request = json!({"KeyId": key_id});
        for (member, value) in members.as_object().expect("the row's members are an object") {
            request[member.as_str()] = value.clone();
        }
        let (status, actual) = failure(&router, "ListGrants", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case}: expected HTTP 400 {code}, got {status} {actual}"
        );
    }

    // The documented upper bound itself is accepted.
    let whole = list_grants(
        &router,
        "Limit 100",
        &json!({"KeyId": key_id, "Limit": 100}),
    )
    .await;
    assert_eq!(
        whole.len(),
        created.len(),
        "case Limit 100: expected every grant on one page, got {whole:?}"
    );
}

/// The `Grants` `ListRetirableGrants` reports for a request, in the order reported.
async fn list_retirable(router: &MultiServiceRouter, case: &str, request: &Value) -> Vec<Value> {
    let answer = ok(router, "ListRetirableGrants", request).await;
    answer["Grants"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
        .clone()
}

/// The `GrantId` of every entry of a grant listing, in the order reported.
fn listed_ids(case: &str, listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|entry| text(case, entry, "GrantId"))
        .collect()
}

/// A key carrying exactly one grant, as `(key id, key ARN, grant id, grant token)`.
async fn key_with_grant(
    router: &MultiServiceRouter,
    case: &str,
) -> (String, String, String, String) {
    let key_id = text(case, &enabled_key(router).await, "KeyId");
    let key_arn = text(case, &describe(router, &key_id).await, "Arn");
    let (grant_id, grant_token) = create_grant(
        router,
        case,
        &json!({
            "KeyId": key_id,
            "GranteePrincipal": principal("role/Grantee"),
            "RetiringPrincipal": principal("role/Retirer"),
            "Operations": ["Encrypt", "Decrypt", "RetireGrant"],
        }),
    )
    .await;
    (key_id, key_arn, grant_id, grant_token)
}

/// Rule 9: `RevokeGrant` deletes the grant the request names, and only once.
#[tokio::test]
async fn revoke_grant_deletes_the_grant_it_names() {
    let router = router();

    // Every form a `KeyId` takes names the same key, so the grant is revoked through
    // each of them.
    for case in ["a key id", "a key ARN", "an alias of the key"] {
        let (key_id, key_arn, grant_id, grant_token) = key_with_grant(&router, case).await;
        let identifier = match case {
            "a key ARN" => key_arn,
            "an alias of the key" => {
                let alias = format!("alias/revoke-{}", &grant_id[..8]);
                create_alias(&router, case, &alias, &key_id).await;
                alias
            }
            _ => key_id.clone(),
        };
        let request = json!({"KeyId": identifier, "GrantId": grant_id});

        let answer = ok(&router, "RevokeGrant", &request).await;
        assert_empty(case, "RevokeGrant", &answer);

        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        assert!(
            listed.is_empty(),
            "case {case}: expected the revoked grant to be gone, got {listed:?}"
        );

        // The grant token minted for it named that grant, and the permission it
        // carried is gone with it.
        let (status, actual) = failure(
            &router,
            "Encrypt",
            &json!({
                "KeyId": key_id,
                "Plaintext": b64(b"after revoke"),
                "GrantTokens": [grant_token],
            }),
        )
        .await;
        assert_eq!(
            (status, actual.as_str()),
            (400, "AccessDeniedException"),
            "case {case}: expected the revoked grant's token to carry no permission, got \
             {status} {actual}"
        );

        // Revoking it a second time finds nothing to revoke.
        let (status, actual) = failure(&router, "RevokeGrant", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, "NotFoundException"),
            "case {case}: expected a second revoke to be refused, got {status} {actual}"
        );
    }

    // Every refusal leaves the grant exactly as it was.
    let (key_id, _, grant_id, _) = key_with_grant(&router, "refusals").await;
    let (other_key, _, _, _) = key_with_grant(&router, "another key").await;
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a grant id that is not one",
            json!({"KeyId": key_id, "GrantId": "not-a-grant-id"}),
            "InvalidGrantIdException",
        ),
        (
            "a well-formed grant id nothing holds",
            json!({"KeyId": key_id, "GrantId": "a".repeat(64)}),
            "NotFoundException",
        ),
        (
            "a grant of another key",
            json!({"KeyId": other_key, "GrantId": grant_id}),
            "NotFoundException",
        ),
        (
            "a key nothing holds",
            json!({"KeyId": "1234abcd-12ab-34cd-56ef-1234567890ab", "GrantId": grant_id}),
            "NotFoundException",
        ),
        (
            "an ARN that is not a KMS ARN",
            json!({"KeyId": "arn:aws:kms:us-east-1", "GrantId": grant_id}),
            "InvalidArnException",
        ),
        (
            "a dry run",
            json!({"KeyId": key_id, "GrantId": grant_id, "DryRun": true}),
            "DryRunOperationException",
        ),
    ];

    for (case, request, code) in rows {
        let (status, actual) = failure(&router, "RevokeGrant", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case}: expected HTTP 400 {code}, got {status} {actual}"
        );
        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        assert_eq!(
            listed_ids(case, &listed),
            vec![grant_id.clone()],
            "case {case}: expected the grant to survive the refusal, got {listed:?}"
        );
    }
}

/// Rule 9: `RetireGrant` deletes the grant the request identifies, however it is
/// identified, and only once.
#[tokio::test]
async fn retire_grant_deletes_the_grant_it_identifies() {
    let router = router();

    // "To identify the grant to retire, use a grant token, or both the grant ID and a
    // key identifier (key ID or key ARN) of the KMS key."
    for case in [
        "a grant token",
        "a key id and a grant id",
        "a key ARN and a grant id",
    ] {
        let (key_id, key_arn, grant_id, grant_token) = key_with_grant(&router, case).await;
        let request = match case {
            "a grant token" => json!({"GrantToken": grant_token}),
            "a key ARN and a grant id" => json!({"KeyId": key_arn, "GrantId": grant_id}),
            _ => json!({"KeyId": key_id, "GrantId": grant_id}),
        };

        let answer = ok(&router, "RetireGrant", &request).await;
        assert_empty(case, "RetireGrant", &answer);

        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        assert!(
            listed.is_empty(),
            "case {case}: expected the retired grant to be gone, got {listed:?}"
        );

        // Retiring it a second time finds no grant, and so no permission to retire
        // one: "permission to retire a grant is determined primarily by the grant".
        let (status, actual) = failure(&router, "RetireGrant", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, "AccessDeniedException"),
            "case {case}: expected a second retire to be denied, got {status} {actual}"
        );
    }

    // Every refusal leaves the grant exactly as it was.
    let (key_id, _, grant_id, grant_token) = key_with_grant(&router, "refusals").await;
    let rows: Vec<(&str, Value, &str)> = vec![
        ("nothing identifying a grant", json!({}), "ValidationException"),
        (
            "a key id with no grant id",
            json!({"KeyId": key_id}),
            "ValidationException",
        ),
        (
            "a grant id with no key id",
            json!({"GrantId": grant_id}),
            "ValidationException",
        ),
        (
            "a token the service never minted",
            json!({"GrantToken": "not-a-grant-token"}),
            "InvalidGrantTokenException",
        ),
        (
            "a grant id that is not one",
            json!({"KeyId": key_id, "GrantId": "not-a-grant-id"}),
            "InvalidGrantIdException",
        ),
        (
            "a well-formed grant id nothing holds",
            json!({"KeyId": key_id, "GrantId": "a".repeat(64)}),
            "AccessDeniedException",
        ),
        (
            "a dry run by grant token",
            json!({"GrantToken": grant_token, "DryRun": true}),
            "DryRunOperationException",
        ),
        (
            "a dry run by key id and grant id",
            json!({"KeyId": key_id, "GrantId": grant_id, "DryRun": true}),
            "DryRunOperationException",
        ),
    ];

    for (case, request, code) in rows {
        let (status, actual) = failure(&router, "RetireGrant", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case}: expected HTTP 400 {code}, got {status} {actual}"
        );
        let listed = list_grants(&router, case, &json!({"KeyId": key_id})).await;
        assert_eq!(
            listed_ids(case, &listed),
            vec![grant_id.clone()],
            "case {case}: expected the grant to survive the refusal, got {listed:?}"
        );
    }
}

/// Rule 10: `ListRetirableGrants` selects the grants a principal may retire, across
/// every key, and pages over them in creation order.
#[tokio::test]
async fn list_retirable_grants_selects_by_retiring_principal() {
    let router = router();
    let retirer = principal("role/Retirer");
    let other = principal("role/OtherRetirer");

    // Two keys, so the selection is shown to span keys rather than to be one key's
    // grant list under another name.
    let first_key = text("setup", &enabled_key(&router).await, "KeyId");
    let second_key = text("setup", &enabled_key(&router).await, "KeyId");

    let mut retirable: Vec<String> = Vec::new();
    let mut unrelated: Vec<String> = Vec::new();
    for index in 0..6u32 {
        let key = if index % 2 == 0 { &first_key } else { &second_key };
        let mine = index % 3 != 2;
        let (grant_id, _) = create_grant(
            &router,
            "setup",
            &json!({
                "KeyId": key,
                "GranteePrincipal": principal("role/Grantee"),
                "RetiringPrincipal": if mine { &retirer } else { &other },
                "Operations": ["Decrypt"],
            }),
        )
        .await;
        if mine {
            retirable.push(grant_id);
        } else {
            unrelated.push(grant_id);
        }
    }

    // (case, the retiring principal asked about, the grants expected).
    let selections: Vec<(&str, &String, &Vec<String>)> = vec![
        ("the retiring principal of four grants", &retirer, &retirable),
        ("the retiring principal of two grants", &other, &unrelated),
    ];
    for (case, principal, expected) in selections {
        let listed = list_retirable(
            &router,
            case,
            &json!({"RetiringPrincipal": principal}),
        )
        .await;
        assert_eq!(
            &listed_ids(case, &listed),
            expected,
            "case {case}: expected {expected:?}, got {listed:?}"
        );
    }

    // A principal that retires nothing selects nothing.
    let none = list_retirable(
        &router,
        "a principal with no grants",
        &json!({"RetiringPrincipal": principal("role/Nobody")}),
    )
    .await;
    assert!(
        none.is_empty(),
        "case a principal with no grants: expected no grants, got {none:?}"
    );

    // Every page size, including one that divides the selection exactly, one that does
    // not, and one larger than it.
    for limit in [1usize, 2, 3, 4, 100] {
        let case = format!("Limit {limit}");
        let mut collected: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        loop {
            let mut request = json!({"RetiringPrincipal": retirer, "Limit": limit});
            if let Some(marker) = marker.as_deref() {
                request["Marker"] = json!(marker);
            }
            let answer = ok(&router, "ListRetirableGrants", &request).await;
            let page = answer["Grants"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {answer}"))
                .clone();
            assert!(
                page.len() <= limit,
                "case {case}: expected at most {limit} grants per page, got {}",
                page.len()
            );
            collected.extend(listed_ids(&case, &page));

            let truncated = answer["Truncated"].as_bool().unwrap_or_else(|| {
                panic!("case {case}: expected a Truncated flag in {answer}")
            });
            assert_eq!(
                truncated,
                collected.len() < retirable.len(),
                "case {case}: expected Truncated {} after {} of {} grants, got {answer}",
                collected.len() < retirable.len(),
                collected.len(),
                retirable.len()
            );
            if !truncated {
                assert!(
                    answer["NextMarker"].is_null(),
                    "case {case}: expected no NextMarker on the last page, got {answer}"
                );
                break;
            }
            marker = Some(text(&case, &answer, "NextMarker"));
        }
        assert_eq!(
            collected, retirable,
            "case {case}: expected every retirable grant in creation order {retirable:?}, got \
             {collected:?}"
        );
    }

    // (case, the members the request carries, the error code expected).
    let refusals: Vec<(&str, Value, &str)> = vec![
        ("no principal at all", json!({}), "ValidationException"),
        (
            "both principal filters",
            json!({
                "RetiringPrincipal": retirer,
                "RetiringServicePrincipal": "dynamodb.amazonaws.com",
            }),
            "ValidationException",
        ),
        (
            "a principal that is not an ARN",
            json!({"RetiringPrincipal": engine::emulator_account_id()}),
            "InvalidArnException",
        ),
        (
            "a marker from no page",
            json!({"RetiringPrincipal": retirer, "Marker": "zz"}),
            "InvalidMarkerException",
        ),
        (
            "a marker from the per-key grant list",
            json!({"RetiringPrincipal": retirer, "Marker": "6772616e74732f312f61"}),
            "InvalidMarkerException",
        ),
        (
            "a Limit past the documented bound",
            json!({"RetiringPrincipal": retirer, "Limit": 101}),
            "ValidationException",
        ),
    ];
    for (case, request, code) in refusals {
        let (status, actual) = failure(&router, "ListRetirableGrants", &request).await;
        assert_eq!(
            (status, actual.as_str()),
            (400, code),
            "case {case}: expected HTTP 400 {code}, got {status} {actual}"
        );
    }

    // A grant that has been retired is no longer retirable.
    ok(
        &router,
        "RetireGrant",
        &json!({"KeyId": first_key, "GrantId": retirable[0]}),
    )
    .await;
    let after = list_retirable(
        &router,
        "after retiring one",
        &json!({"RetiringPrincipal": retirer}),
    )
    .await;
    assert_eq!(
        listed_ids("after retiring one", &after),
        retirable[1..].to_vec(),
        "case after retiring one: expected the retired grant to be gone, got {after:?}"
    );
}

/// Rule 7 and 9 together: a grant token carries exactly the permission its grant
/// carries, for as long as the grant is held, and nothing once it is gone.
#[tokio::test]
async fn a_grant_token_carries_only_its_grant_s_permission() {
    let router = router();
    let plaintext = b64(b"grant token round trip");

    for case in ["revoked", "retired"] {
        let (key_id, _, grant_id, grant_token) = key_with_grant(&router, case).await;

        // The token `CreateGrant` minted names a held grant that permits both
        // operations on this key, so both are authorized by it.
        let sealed = ok(
            &router,
            "Encrypt",
            &json!({
                "KeyId": key_id,
                "Plaintext": plaintext,
                "GrantTokens": [grant_token],
            }),
        )
        .await;
        let blob = text(case, &sealed, "CiphertextBlob");
        let opened = ok(
            &router,
            "Decrypt",
            &json!({"CiphertextBlob": blob, "GrantTokens": [grant_token]}),
        )
        .await;
        assert_eq!(
            opened["Plaintext"],
            json!(plaintext),
            "case {case}: expected the plaintext back, got {opened}"
        );

        // Deleting the grant takes the permission with it.
        if case == "revoked" {
            ok(
                &router,
                "RevokeGrant",
                &json!({"KeyId": key_id, "GrantId": grant_id}),
            )
            .await;
        } else {
            ok(&router, "RetireGrant", &json!({"GrantToken": grant_token})).await;
        }

        for (operation, request) in [
            (
                "Encrypt",
                json!({
                    "KeyId": key_id,
                    "Plaintext": plaintext,
                    "GrantTokens": [grant_token],
                }),
            ),
            (
                "Decrypt",
                json!({"CiphertextBlob": blob, "GrantTokens": [grant_token]}),
            ),
        ] {
            let (status, actual) = failure(&router, operation, &request).await;
            assert_eq!(
                (status, actual.as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected {operation} to deny the gone grant's token, got \
                 {status} {actual}"
            );
        }
    }
}

/// Rule 7: a grant token authorizes a cryptographic operation only on the grant's own
/// key, only for the operations the grant permits, and only where the request's
/// encryption context satisfies the grant's constraints.
#[tokio::test]
async fn a_grant_token_is_scoped_to_its_grant_s_key_operations_and_context() {
    let router = router();
    let grantee = principal("role/Scoped");

    // Two keys, so a grant on one is shown not to authorize the other.
    let key_a = text("setup", &enabled_key(&router).await, "KeyId");
    let key_b = text("setup", &enabled_key(&router).await, "KeyId");

    // One grant per scope the rule varies over.
    let (_, unconstrained) = create_grant(
        &router,
        "setup",
        &json!({
            "KeyId": key_a,
            "GranteePrincipal": grantee,
            "Operations": ["Encrypt", "Decrypt"],
        }),
    )
    .await;
    let (_, decrypt_only) = create_grant(
        &router,
        "setup",
        &json!({
            "KeyId": key_a,
            "GranteePrincipal": grantee,
            "Operations": ["Decrypt"],
        }),
    )
    .await;
    let (_, equals_constrained) = create_grant(
        &router,
        "setup",
        &json!({
            "KeyId": key_a,
            "GranteePrincipal": grantee,
            "Operations": ["Encrypt", "Decrypt"],
            "Constraints": {"EncryptionContextEquals": {"Department": "Finance"}},
        }),
    )
    .await;
    let (_, subset_constrained) = create_grant(
        &router,
        "setup",
        &json!({
            "KeyId": key_a,
            "GranteePrincipal": grantee,
            "Operations": ["Encrypt", "Decrypt"],
            "Constraints": {"EncryptionContextSubset": {"Department": "Finance"}},
        }),
    )
    .await;

    // (case, the token presented, the key encrypted under, the EncryptionContext the
    // request carries, whether the grant authorizes it).
    //
    // "EncryptionContextEquals and EncryptionContextSubset … allow the permissions in
    // the grant only when the encryption context in the request matches
    // (EncryptionContextEquals) or includes (EncryptionContextSubset) the encryption
    // context specified in the constraint", and "in a grant constraint, the key in
    // each key-value pair is not case sensitive, but the value is case sensitive"
    // (`smithy_query(query="doc",shape="GrantConstraints")`).
    let rows: Vec<(&str, &String, &String, Value, bool)> = vec![
        (
            "an unconstrained grant on the key",
            &unconstrained,
            &key_a,
            Value::Null,
            true,
        ),
        (
            "an unconstrained grant on another key",
            &unconstrained,
            &key_b,
            Value::Null,
            false,
        ),
        (
            "a grant that permits only Decrypt",
            &decrypt_only,
            &key_a,
            Value::Null,
            false,
        ),
        (
            "an EncryptionContextEquals constraint, matched exactly",
            &equals_constrained,
            &key_a,
            json!({"Department": "Finance"}),
            true,
        ),
        (
            "an EncryptionContextEquals constraint, the key in another case",
            &equals_constrained,
            &key_a,
            json!({"DEPARTMENT": "Finance"}),
            true,
        ),
        (
            "an EncryptionContextEquals constraint, the value in another case",
            &equals_constrained,
            &key_a,
            json!({"Department": "finance"}),
            false,
        ),
        (
            "an EncryptionContextEquals constraint, no context at all",
            &equals_constrained,
            &key_a,
            Value::Null,
            false,
        ),
        (
            "an EncryptionContextEquals constraint, one pair too many",
            &equals_constrained,
            &key_a,
            json!({"Department": "Finance", "Project": "Ledger"}),
            false,
        ),
        (
            "an EncryptionContextSubset constraint, matched exactly",
            &subset_constrained,
            &key_a,
            json!({"Department": "Finance"}),
            true,
        ),
        (
            "an EncryptionContextSubset constraint, with another pair beside it",
            &subset_constrained,
            &key_a,
            json!({"Project": "Ledger", "Department": "Finance"}),
            true,
        ),
        (
            "an EncryptionContextSubset constraint, the pair absent",
            &subset_constrained,
            &key_a,
            json!({"Project": "Ledger"}),
            false,
        ),
        (
            "an EncryptionContextSubset constraint, no context at all",
            &subset_constrained,
            &key_a,
            Value::Null,
            false,
        ),
    ];

    for (case, token, key_id, context, authorized) in rows {
        let mut request = json!({
            "KeyId": key_id,
            "Plaintext": b64(b"scoped"),
            "GrantTokens": [token],
        });
        if !context.is_null() {
            request["EncryptionContext"] = context.clone();
        }

        if authorized {
            ok(&router, "Encrypt", &request).await;
        } else {
            let (status, actual) = failure(&router, "Encrypt", &request).await;
            assert_eq!(
                (status, actual.as_str()),
                (400, "AccessDeniedException"),
                "case {case}: expected HTTP 400 AccessDeniedException, got {status} {actual}"
            );
        }
    }

    // A request that presents no grant token is not asking to be authorized by a
    // grant, so a constraint on some grant of the key does not narrow it.
    ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_a, "Plaintext": b64(b"no token")}),
    )
    .await;
}
