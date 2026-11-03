// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the error semantics of the key-state, identifier,
//! import-material and custom-key-store paths, driven over the wire through the shared
//! [`super::harness`].
//!
//! Four rules are under test, each over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. A key outside the `Enabled` key state cannot be used in a cryptographic
//!    operation, and every one of them refuses it with the same error — `Disabled`
//!    answers `DisabledException` — while the metadata plane stays readable. The rows
//!    differ in WHICH cryptographic operation is called.
//! 2. A `KeyId` that is an ARN is resolved in three steps: its SHAPE as a KMS key or
//!    alias ARN, then AUTHORIZATION of the account it names, and only then the
//!    existence of the resource. So an ARN whose account-id segment is not a 12-digit
//!    account id is refused with `AccessDeniedException` by every operation that
//!    resolves one, whatever resource it goes on to name, while an ARN naming a
//!    well-formed account that holds no such key is reported as not found. The rows
//!    differ in the account segment, in the resource the ARN names, and in the
//!    operation.
//! 3. Importing key material applies only to a key created with no key material, so
//!    `GetParametersForImport` refuses a key whose material KMS generated and leaves it
//!    untouched; for a key with `Origin=EXTERNAL` it answers a real RSA wrapping public
//!    key, an import token and the instant both expire. The rows differ in the origin,
//!    the key spec and the wrapping key spec.
//! 4. `UpdateCustomKeyStore` resolves its target first, and a `CustomKeyStoreId` that
//!    names no key store of this account and Region is ONE refusal —
//!    `CustomKeyStoreNotFoundException` — whether it is a well-formed id nothing holds
//!    or a value that is not a key store id at all. The rows differ in how the id fails
//!    to name a key store.
//!
//! The key-store rows seed the records DIRECTLY into the store — the same records a
//! create leaves behind — exactly as [`super::custom_key_stores`] does. What a create
//! itself answers is [`super::create_custom_key_store`].

use std::sync::Arc;

use serde_json::{json, Value};

use service_com_amazonaws_kms_trentservice::state::{
    key_spec_profile, mint_custom_key_store_id, mint_material, random_bytes, service_state,
    CloudHsmCluster, CustomKeyStore, CustomKeyStoreBacking, KeyMaterial, MaterialKind, XksProxy,
    KEY_SPEC_SYMMETRIC_DEFAULT,
};

use super::harness::{
    b64, create_key, describe, ok, refused, rotate_on_demand, router, router_with_engine, text,
    tracked_last_usage, unb64, wrap, REGION,
};

/// The Region a row belongs to when the row is about regional scoping.
const OTHER_REGION: &str = "eu-west-2";

/// Rule 1: every cryptographic operation refuses a key that is not enabled, with the
/// error that names the key state it is in, and the metadata plane stays readable.
#[tokio::test]
async fn a_disabled_key_refuses_every_cryptographic_operation() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "disable then use"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");

    // A ciphertext sealed while the key was still enabled, so the `Decrypt` row is
    // refused for the key state rather than for the ciphertext.
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"envelope me")}),
    )
    .await;
    let ciphertext = text("setup", &sealed, "CiphertextBlob");

    ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;

    // Rows: the cryptographic operation, and the request that reaches its key-state
    // check. They differ in which operation is called, not in the key.
    let rows: Vec<(&str, Value)> = vec![
        (
            "Encrypt",
            json!({"KeyId": key_id, "Plaintext": b64(b"envelope me")}),
        ),
        ("Decrypt", json!({"CiphertextBlob": ciphertext})),
        (
            "GenerateDataKey",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
        (
            "GenerateDataKey",
            json!({"KeyId": key_id, "NumberOfBytes": 16}),
        ),
        ("GenerateDataKey", json!({"KeyId": key_id})),
        (
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": key_id, "KeySpec": "AES_128"}),
        ),
    ];
    for (operation, body) in rows {
        let case = format!("{operation} {body}");
        let (status, code, message) = refused(&router, REGION, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "DisabledException",
            "case {case:?}: expected DisabledException, got {code:?}"
        );
        assert_eq!(
            message,
            format!("{key_arn} is disabled."),
            "case {case:?}: expected the refusal to name the key and its state, got \
             {message:?}"
        );
    }

    // The metadata plane is unaffected: a disabled key is still described, and still
    // reports the state that refuses the calls above.
    let described = describe(&router, &key_id).await;
    assert_eq!(
        described["KeyState"].as_str().unwrap_or_default(),
        "Disabled",
        "case metadata: a disabled key is still described, got {described}"
    );

    // Re-enabling it makes the same call succeed, so the refusal was the key state and
    // nothing else.
    ok(&router, "EnableKey", &json!({"KeyId": key_id})).await;
    let generated = ok(
        &router,
        "GenerateDataKey",
        &json!({"KeyId": key_id, "KeySpec": "AES_256"}),
    )
    .await;
    let plaintext = unb64("re-enabled", &generated, "Plaintext");
    assert_eq!(
        plaintext.len(),
        32,
        "case re-enabled: AES_256 names a 256-bit data key, got {} bytes",
        plaintext.len()
    );
    assert_eq!(
        generated["KeyId"].as_str().unwrap_or_default(),
        key_arn,
        "case re-enabled: the answer names the key by its ARN, got {generated}"
    );
}

/// Rule 1, continued: the data key `GenerateDataKey` returns in the clear is the one it
/// sealed, which is what makes the sealed copy usable — proved by opening it through
/// `Decrypt`.
#[tokio::test]
async fn a_generated_data_key_round_trips_through_decrypt() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "data keys"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: how the request states the length of the data key, and how many bytes that
    // is — the modelled `DataKeySpec` values and the modelled `NumberOfBytes` bounds.
    // Which requests state a length at all is the subject of `super::data_key`.
    let rows: Vec<(&str, Value, usize)> = vec![
        ("KeySpec AES_256", json!({"KeySpec": "AES_256"}), 32),
        ("KeySpec AES_128", json!({"KeySpec": "AES_128"}), 16),
        ("NumberOfBytes 1", json!({"NumberOfBytes": 1}), 1),
        ("NumberOfBytes 1024", json!({"NumberOfBytes": 1024}), 1024),
        ("NumberOfBytes 70", json!({"NumberOfBytes": 70}), 70),
    ];
    for (case, mut body, expected_len) in rows {
        body["KeyId"] = json!(key_id);
        let generated = ok(&router, "GenerateDataKey", &body).await;
        let plaintext = unb64(case, &generated, "Plaintext");
        assert_eq!(
            plaintext.len(),
            expected_len,
            "case {case:?}: expected a {expected_len}-byte data key, got {}",
            plaintext.len()
        );
        let ciphertext = text(case, &generated, "CiphertextBlob");
        let opened = ok(&router, "Decrypt", &json!({"CiphertextBlob": ciphertext})).await;
        let recovered = unb64(case, &opened, "Plaintext");
        assert_eq!(
            recovered, plaintext,
            "case {case:?}: the sealed copy must open to the data key that was returned"
        );
    }
}

/// Rule 2: an ARN whose account-id segment is not an account id is refused for want of
/// permission by every operation that resolves a `KeyId`, before the resource it names
/// is looked up — while an ARN naming a well-formed account that holds no such key is
/// reported as not found.
#[tokio::test]
async fn an_arn_whose_account_segment_is_not_an_account_id_is_not_authorized() {
    let router = router();
    let account = engine::emulator_account_id();
    let metadata = create_key(&router, &json!({"Description": "arn resolution"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: the account segment of the ARN. `AccountIdType` is `@length(12, 12)` with
    // `@pattern(^[0-9]{12}$)` (`smithy_query(query="shape",shape="AccountIdType")`), so
    // the rows walk each published bound and one past it, plus a segment that is not
    // numeric at all.
    let unauthorized_accounts = [
        "notanumericaccount",
        "aws",
        "11111111111",
        "1111111111111",
        "00000000000a",
        "-11122223333",
    ];
    // Rows: what the rest of the ARN names — an existing key, an absent key, and an
    // alias — so the refusal cannot depend on whether the resource exists.
    let resources = [
        format!("key/{key_id}"),
        "key/00000000-0000-0000-0000-000000000000".to_string(),
        "alias/no-such-alias".to_string(),
    ];
    // Rows: the operations that resolve a `KeyId`, across the metadata plane, the
    // key-management plane and the data plane.
    let operations = [
        "GetKeyLastUsage",
        "ListKeyPolicies",
        "UpdatePrimaryRegion",
        "DescribeKey",
        "ListKeyRotations",
        "GetKeyRotationStatus",
        "Encrypt",
        "GenerateDataKey",
    ];

    for bad_account in unauthorized_accounts {
        for resource in &resources {
            let identifier = format!("arn:aws:kms:{REGION}:{bad_account}:{resource}");
            for operation in operations {
                let case = format!("{operation} on {identifier}");
                let body = request_for(operation, &identifier);
                let (status, code, message) = refused(&router, REGION, operation, &body).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} ({code})"
                );
                assert_eq!(
                    code, "AccessDeniedException",
                    "case {case:?}: expected AccessDeniedException, got {code:?}"
                );
                assert_eq!(
                    message,
                    format!(
                        "User: arn:aws:iam::{account}:user/emulator is not authorized to \
                         perform: kms:{operation} on this resource because the resource \
                         does not exist in this Region, no resource-based policies allow \
                         access, or a resource-based policy explicitly denies access"
                    ),
                    "case {case:?}: expected the refusal to name the principal and the \
                     action, got {message:?}"
                );
                assert!(
                    !message.contains(identifier.as_str()),
                    "case {case:?}: authorization is evaluated before the identifier's \
                     contents are, so the refusal must not echo the resource, got \
                     {message:?}"
                );
            }
        }
    }

    // The boundary: an account segment that IS a 12-digit account id, but not this
    // account's, is a well-formed ARN naming a key this endpoint does not hold — a
    // missing resource, not an authorization failure.
    for well_formed_account in ["999999999999", "111122223333"] {
        let identifier = format!("arn:aws:kms:{REGION}:{well_formed_account}:key/{key_id}");
        for operation in operations {
            let case = format!("{operation} on {identifier}");
            let body = request_for(operation, &identifier);
            let (status, code, _message) = refused(&router, REGION, operation, &body).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case:?}: expected NotFoundException for a well-formed account that \
                 holds no such key, got {code:?}"
            );
        }
    }

    // And this account's own ARN still resolves, so the account check refuses nothing
    // it should not.
    let own = format!("arn:aws:kms:{REGION}:{account}:key/{key_id}");
    let described = describe(&router, &own).await;
    assert_eq!(
        described["KeyId"].as_str().unwrap_or_default(),
        key_id,
        "case own account ARN: expected the key to resolve, got {described}"
    );
}

/// The request that reaches `operation`'s `KeyId` resolution, for an identifier under
/// test. Only the members the model requires beside `KeyId` are added.
fn request_for(operation: &str, identifier: &str) -> Value {
    let mut body = json!({"KeyId": identifier});
    match operation {
        "UpdatePrimaryRegion" => body["PrimaryRegion"] = json!("us-west-2"),
        "Encrypt" => body["Plaintext"] = json!(b64(b"hello kms")),
        "GenerateDataKey" => body["KeySpec"] = json!("AES_256"),
        _ => {}
    }
    body
}

/// Rule 2, continued: `GetKeyLastUsage` reports the last successful cryptographic
/// operation performed with the key, once KMS's usage tracking has produced the record
/// (the empty record a read taken straight after the call is answered is
/// [`super::key_last_usage`]'s table), and the identifiers it cannot resolve are refused
/// by the kind of failure they are.
#[tokio::test]
async fn get_key_last_usage_reports_the_last_cryptographic_operation() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "usage tracking"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let creation_date = metadata["CreationDate"].as_f64().unwrap_or_default();

    // A key that has had no cryptographic operation reports no usage at all, and began
    // being tracked when it was created.
    //
    // This response names the key by its KEY ID: `KeyId` here is "the globally unique
    // identifier for the KMS key"
    // (`smithy_query(query="doc",shape="GetKeyLastUsageResponse",member="KeyId")`), with
    // none of the "the Amazon Resource Name (key ARN)" wording the same member carries on
    // the cryptographic operations — and the recorded response is the bare key id.
    let fresh = ok(&router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await;
    assert_eq!(
        fresh["KeyId"].as_str().unwrap_or_default(),
        key_id,
        "case fresh key: this answer names the key by its key ID — \"the globally unique \
         identifier for the KMS key\" — not by its ARN, got {fresh}"
    );
    assert_ne!(
        fresh["KeyId"].as_str().unwrap_or_default(),
        key_arn,
        "case fresh key: the key ARN is not the globally unique identifier this member \
         carries, got {fresh}"
    );
    assert_eq!(
        fresh["KeyLastUsage"],
        json!({}),
        "case fresh key: a key with no tracked usage record reports an empty KeyLastUsage, \
         got {fresh}"
    );
    assert_eq!(
        fresh["KeyCreationDate"].as_f64().unwrap_or_default(),
        creation_date,
        "case fresh key: KeyCreationDate must be the key's creation date, got {fresh}"
    );
    assert_eq!(
        fresh["TrackingStartDate"].as_f64().unwrap_or_default(),
        creation_date,
        "case fresh key: tracking starts when the key is created, got {fresh}"
    );

    // Rows: a cryptographic operation, the request that performs it, and the operation
    // name the usage record must then name. Each row's call succeeds, so each row
    // replaces the record the previous one left — once the tracking has produced it; the
    // read taken straight after the call is answered the empty record.
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"usage")}),
    )
    .await;
    let ciphertext = text("setup", &sealed, "CiphertextBlob");
    let rows: Vec<(&str, Value)> = vec![
        (
            "Encrypt",
            json!({"KeyId": key_id, "Plaintext": b64(b"usage")}),
        ),
        (
            "GenerateDataKey",
            json!({"KeyId": key_id, "KeySpec": "AES_256"}),
        ),
        (
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": key_id, "NumberOfBytes": 32}),
        ),
        ("Decrypt", json!({"CiphertextBlob": ciphertext})),
    ];
    let mut seen_event_ids: Vec<String> = Vec::new();
    for (operation, body) in rows {
        ok(&router, operation, &body).await;
        let reported = tracked_last_usage(&router, operation, &key_id, operation, &Value::Null).await;
        let usage = reported["KeyLastUsage"].clone();
        assert_eq!(
            usage["Operation"].as_str().unwrap_or_default(),
            operation,
            "case {operation}: expected the record to name the operation, got {reported}"
        );
        assert!(
            usage["Timestamp"].as_f64().unwrap_or_default() >= creation_date,
            "case {operation}: expected a timestamp at or after the key's creation, got \
             {reported}"
        );
        let event_id = usage["CloudTrailEventId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !event_id.is_empty() && !seen_event_ids.contains(&event_id),
            "case {operation}: expected a CloudTrail event id of its own, got \
             {event_id:?} after {seen_event_ids:?}"
        );
        assert!(
            !usage["KmsRequestId"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {operation}: expected a request id on the record, got {reported}"
        );
        seen_event_ids.push(event_id);

        // Reading the record does not change it: asking again answers the same.
        let again = ok(&router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await;
        assert_eq!(
            again, reported,
            "case {operation}: a second read must answer the same record"
        );
    }

    // A refused cryptographic operation did not use the key, so it leaves the record
    // alone. The record of the last row is already produced, so nothing the clock owes
    // can move it either.
    let before = ok(&router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await;
    let (_status, code, _message) = refused(
        &router,
        REGION,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"too big"), "EncryptionAlgorithm": "SM2PKE"}),
    )
    .await;
    assert_eq!(
        code, "InvalidKeyUsageException",
        "case refused Encrypt: expected the request to be refused, got {code:?}"
    );
    let after = ok(&router, "GetKeyLastUsage", &json!({"KeyId": key_id})).await;
    assert_eq!(
        after, before,
        "case refused Encrypt: a refused call must not be recorded as a use of the key"
    );
}

/// Rule 2, continued: how `GetKeyLastUsage` refuses the identifiers it cannot resolve.
#[tokio::test]
async fn get_key_last_usage_refuses_the_identifiers_it_cannot_resolve() {
    let router = router();
    let account = engine::emulator_account_id();
    let metadata = create_key(&router, &json!({"Description": "arn refusals"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    // Rows: identifiers that are ARN-shaped but are not KMS key ARNs this endpoint can
    // resolve. They differ in HOW the ARN fails: an extra colon-delimited field, a
    // missing Region, another service, an empty resource, too few segments.
    let invalid_arns = vec![
        format!("arn:aws:kms::{REGION}:{account}:key/{key_id}"),
        format!("arn:aws:kms::{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"),
        format!("arn:aws:kms::{account}:key/{key_id}"),
        format!("arn:aws:kms:{REGION}:{account}:thing/{key_id}"),
        format!("arn:aws:kms:{REGION}:{account}:key/"),
        "arn:aws:s3:::my-bucket".to_string(),
        "arn:aws:kms".to_string(),
    ];
    for identifier in invalid_arns {
        let (status, code, message) = refused(
            &router,
            REGION,
            "GetKeyLastUsage",
            &json!({"KeyId": identifier}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {identifier:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {identifier:?}: expected NotFoundException, got {code:?}"
        );
        assert_eq!(
            message,
            format!("Invalid arn {identifier}"),
            "case {identifier:?}: expected the refusal to name the ARN, got {message:?}"
        );
    }

    // "Alias names are not supported": an alias is well formed, and is not an
    // identifier this operation accepts — a judgement on the identifier's KIND, which
    // is why the refusal states the rule and names nothing of this endpoint.
    for identifier in [
        "alias/aliases-are-not-supported".to_string(),
        format!("arn:aws:kms:{REGION}:{account}:alias/aliases-are-not-supported"),
    ] {
        let (status, code, message) = refused(
            &router,
            REGION,
            "GetKeyLastUsage",
            &json!({"KeyId": identifier}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {identifier:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidArnException",
            "case {identifier:?}: expected InvalidArnException for an alias, got {code:?}"
        );
        assert_eq!(
            message, "Key Aliases are not supported for this operation.",
            "case {identifier:?}: expected the refusal to state the rule, got {message:?}"
        );
    }

    // A well-formed key identifier that names no key of this endpoint is reported as
    // the missing KEY it asked for, by that key's ARN.
    for identifier in [
        "00000000-0000-0000-0000-000000000000".to_string(),
        format!("arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"),
    ] {
        let (_status, code, message) = refused(
            &router,
            REGION,
            "GetKeyLastUsage",
            &json!({"KeyId": identifier}),
        )
        .await;
        assert_eq!(
            code, "NotFoundException",
            "case {identifier:?}: expected NotFoundException, got {code:?}"
        );
        let key_arn =
            format!("arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000");
        assert_eq!(
            message,
            format!("Key '{key_arn}' does not exist"),
            "case {identifier:?}: expected the refusal to name the missing key, got {message:?}"
        );
    }

    // And the same key ARN in another Region names a resource this endpoint can never
    // serve, which is reported by naming that Region.
    let (_status, code, message) = refused(
        &router,
        REGION,
        "GetKeyLastUsage",
        &json!({"KeyId": format!("arn:aws:kms:ap-south-1:{account}:key/{key_id}")}),
    )
    .await;
    assert_eq!(
        code, "NotFoundException",
        "case another Region: expected NotFoundException, got {code:?}"
    );
    assert_eq!(
        message, "Invalid arn ap-south-1",
        "case another Region: expected the refusal to name the ARN's Region, got {message:?}"
    );
}

/// Rule 3: `GetParametersForImport` refuses a key whose key material KMS generated, and
/// leaves that key exactly as it was.
#[tokio::test]
async fn get_parameters_for_import_refuses_a_key_whose_material_kms_generated() {
    let router = router();

    // Rows: the create request, and the origin the key then has. They differ in whether
    // the origin is defaulted or stated, and in the key spec and usage — none of which
    // changes the answer, because the ORIGIN is what refuses the call.
    let rows: Vec<(&str, Value)> = vec![
        ("defaulted origin", json!({"Description": "native"})),
        (
            "stated AWS_KMS origin",
            json!({"Origin": "AWS_KMS", "KeySpec": "SYMMETRIC_DEFAULT"}),
        ),
        (
            "an HMAC key",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "an asymmetric signing key",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "a multi-Region key",
            json!({"MultiRegion": true, "Description": "native mrk"}),
        ),
    ];
    for (case, request) in rows {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let (status, code, message) = refused(
            &router,
            REGION,
            "GetParametersForImport",
            &json!({
                "KeyId": key_id,
                "WrappingAlgorithm": "RSAES_OAEP_SHA_256",
                "WrappingKeySpec": "RSA_2048",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?}"
        );
        assert_eq!(
            message,
            format!("{key_arn} origin is AWS_KMS which is not valid for this operation."),
            "case {case:?}: expected the refusal to name the key and its origin, got \
             {message:?}"
        );

        // The key is untouched: same origin, same key state, and still usable.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["Origin"].as_str().unwrap_or_default(),
            "AWS_KMS",
            "case {case:?}: the refused call must not change the key's origin, got {described}"
        );
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: the refused call must not change the key state, got {described}"
        );
    }
}

/// Rule 3, continued: for a key created with no key material the same call answers a
/// real RSA wrapping public key, an import token, and the instant both expire.
#[tokio::test]
async fn get_parameters_for_import_answers_a_wrapping_key_for_an_external_origin_key() {
    let router = router();

    // Rows: the `WrappingKeySpec` asked for, and the RSA modulus that spec names in
    // bytes (`smithy_query(query="shape",shape="WrappingKeySpec")` — RSA_2048, RSA_3072
    // and RSA_4096 are the three RSA values).
    let rows = [
        ("RSA_2048", 256usize),
        ("RSA_3072", 384),
        ("RSA_4096", 512),
    ];
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for (wrapping_key_spec, modulus_len) in rows {
        // A key created with no key material waits for `ImportKeyMaterial`, so it is
        // the key this operation exists for.
        let metadata = create_key(
            &router,
            &json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
        )
        .await;
        let key_id = text(wrapping_key_spec, &metadata, "KeyId");
        let key_arn = text(wrapping_key_spec, &metadata, "Arn");
        assert_eq!(
            metadata["KeyState"].as_str().unwrap_or_default(),
            "PendingImport",
            "case {wrapping_key_spec}: a key with no key material waits in PendingImport, \
             got {metadata}"
        );

        // Rows within the row: the wrapping algorithm the caller says it will use,
        // which the parameters are issued for.
        for wrapping_algorithm in ["RSAES_OAEP_SHA_256", "RSAES_OAEP_SHA_1"] {
            let case = format!("{wrapping_key_spec} with {wrapping_algorithm}");
            let answer = ok(
                &router,
                "GetParametersForImport",
                &json!({
                    "KeyId": key_id,
                    "WrappingAlgorithm": wrapping_algorithm,
                    "WrappingKeySpec": wrapping_key_spec,
                }),
            )
            .await;
            assert_eq!(
                answer["KeyId"].as_str().unwrap_or_default(),
                key_arn,
                "case {case:?}: the answer names the key by its ARN, got {answer}"
            );

            // "The public key (or wrapping key) of an RSA key pair that KMS generates":
            // a DER-encoded X.509 SubjectPublicKeyInfo, so it opens with the DER tag
            // for a SEQUENCE and is long enough to carry the modulus the spec names.
            let public_key = unb64(&case, &answer, "PublicKey");
            assert_eq!(
                public_key.first().copied(),
                Some(0x30),
                "case {case:?}: expected DER-encoded public key, got first byte {:?}",
                public_key.first()
            );
            assert!(
                public_key.len() > modulus_len,
                "case {case:?}: expected a public key carrying a {modulus_len}-byte \
                 modulus, got {} bytes",
                public_key.len()
            );

            // The import token is opaque, non-empty, and minted per call: "the public
            // key and its import token are permanently linked".
            let token = unb64(&case, &answer, "ImportToken");
            assert!(
                !token.is_empty() && !tokens.contains(&token),
                "case {case:?}: expected an import token of its own, got {} bytes",
                token.len()
            );
            tokens.push(token);

            // "Each public key and import token set is valid for 24 hours. The
            // expiration date and time appear in the ParametersValidTo field."
            let valid_to = answer["ParametersValidTo"].as_f64().unwrap_or_default();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the clock is after the epoch")
                .as_secs_f64();
            let twenty_four_hours = 24.0 * 60.0 * 60.0;
            assert!(
                (valid_to - (now + twenty_four_hours)).abs() < 60.0,
                "case {case:?}: expected ParametersValidTo 24 hours ahead of {now}, got \
                 {valid_to}"
            );

            // The call reports parameters; it does not move the key out of the state it
            // is waiting in.
            let described = describe(&router, &key_id).await;
            assert_eq!(
                described["KeyState"].as_str().unwrap_or_default(),
                "PendingImport",
                "case {case:?}: the key still waits for its key material, got {described}"
            );
        }
    }

    // `SM2` is the one modelled wrapping key spec that is not an RSA key pair, and this
    // service generates none, so it is reported as not implemented rather than answered
    // with a key pair of another kind.
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let key_id = text("sm2", &metadata, "KeyId");
    let (status, code, _message) = refused(
        &router,
        REGION,
        "GetParametersForImport",
        &json!({
            "KeyId": key_id,
            "WrappingAlgorithm": "SM2PKE",
            "WrappingKeySpec": "SM2",
        }),
    )
    .await;
    assert_eq!(
        status, 501,
        "case SM2: expected HTTP 501 for a wrapping key this service does not generate, \
         got {status} ({code})"
    );
}

/// Rule 3, continued: `DeleteImportedKeyMaterial` applies the same origin gate, and
/// deleting the key material a key holds leaves that key waiting for material again.
#[tokio::test]
async fn delete_imported_key_material_refuses_a_key_whose_material_kms_generated() {
    let (router, engine) = router_with_engine();

    // Rows: keys whose key material KMS generated, differing in key spec, usage and
    // whether the origin was defaulted or stated. The ORIGIN is what refuses the call,
    // so every row meets the same refusal — the one `GetParametersForImport` gives.
    let rows: Vec<(&str, Value)> = vec![
        ("defaulted origin", json!({"Description": "native"})),
        (
            "stated AWS_KMS origin",
            json!({"Origin": "AWS_KMS", "KeySpec": "SYMMETRIC_DEFAULT"}),
        ),
        (
            "an HMAC key",
            json!({"KeySpec": "HMAC_512", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
        (
            "an asymmetric signing key",
            json!({"KeySpec": "ECC_NIST_P384", "KeyUsage": "SIGN_VERIFY"}),
        ),
    ];
    for (case, request) in rows {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let (status, code, message) = refused(
            &router,
            REGION,
            "DeleteImportedKeyMaterial",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?}"
        );
        assert_eq!(
            message,
            format!("{key_arn} origin is AWS_KMS which is not valid for this operation."),
            "case {case:?}: expected the refusal to name the key and its origin, got \
             {message:?}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: the refused call must not change the key state, got {described}"
        );
    }

    // A key created with no key material has none to delete, and the call asks for
    // exactly the state it is already in: the empty identifier says nothing was
    // deleted, and the key still waits.
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let waiting = text("waiting", &metadata, "KeyId");
    let waiting_arn = text("waiting", &metadata, "Arn");
    let answer = ok(
        &router,
        "DeleteImportedKeyMaterial",
        &json!({"KeyId": waiting}),
    )
    .await;
    assert_eq!(
        answer["KeyId"].as_str().unwrap_or_default(),
        waiting_arn,
        "case waiting key: the answer names the key by its ARN, got {answer}"
    );
    assert_eq!(
        answer["KeyMaterialId"].as_str().unwrap_or_default(),
        "",
        "case waiting key: nothing was deleted, got {answer}"
    );
    assert_eq!(
        describe(&router, &waiting).await["KeyState"]
            .as_str()
            .unwrap_or_default(),
        "PendingImport",
        "case waiting key: the key still waits for its key material"
    );

    // Rows: a key that HOLDS imported key material, in each key state that material
    // can be deleted from. `ImportKeyMaterial` is still a stub, so the material is
    // seeded onto the record — the same record an import leaves behind.
    let state_rows: Vec<(&str, bool, &str)> = vec![
        ("an enabled key with imported material", false, "PendingImport"),
        // "When the specified KMS key is in the PendingDeletion state, this operation
        // does not change the KMS key's state."
        ("a key awaiting deletion", true, "PendingDeletion"),
    ];
    for (case, schedule_deletion, expected_state) in state_rows {
        let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_material_id = seed_imported_material(&engine, &key_id);
        assert_eq!(
            describe(&router, &key_id).await["KeyState"]
                .as_str()
                .unwrap_or_default(),
            "Enabled",
            "case {case:?}: a key with key material is usable"
        );
        if schedule_deletion {
            ok(
                &router,
                "ScheduleKeyDeletion",
                &json!({"KeyId": key_id, "PendingWindowInDays": 7}),
            )
            .await;
        }

        // A `KeyMaterialId` that names no key material of this key identifies nothing
        // to delete.
        let (_status, code, _message) = refused(
            &router,
            REGION,
            "DeleteImportedKeyMaterial",
            &json!({"KeyId": key_id, "KeyMaterialId": "0".repeat(64)}),
        )
        .await;
        assert_eq!(
            code, "NotFoundException",
            "case {case:?}: expected NotFoundException for unknown key material, got \
             {code:?}"
        );

        let answer = ok(
            &router,
            "DeleteImportedKeyMaterial",
            &json!({"KeyId": key_id, "KeyMaterialId": key_material_id}),
        )
        .await;
        assert_eq!(
            answer["KeyMaterialId"].as_str().unwrap_or_default(),
            key_material_id,
            "case {case:?}: the answer identifies the deleted key material, got {answer}"
        );
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            expected_state,
            "case {case:?}: expected the key state {expected_state}, got {described}"
        );
        assert!(
            described["CurrentKeyMaterialId"].is_null(),
            "case {case:?}: a key with no key material reports no current material, got \
             {described}"
        );

        // Without its key material the key is unusable in a cryptographic operation.
        let (_status, encrypt_code, _message) = refused(
            &router,
            REGION,
            "Encrypt",
            &json!({"KeyId": key_id, "Plaintext": b64(b"no material")}),
        )
        .await;
        assert_eq!(
            encrypt_code, "KMSInvalidStateException",
            "case {case:?}: a key with no key material cannot encrypt, got {encrypt_code:?}"
        );
    }
}

/// Give a key created with no key material the key material an import would leave on
/// it, and report that material's `KeyMaterialId`.
fn seed_imported_material(engine: &Arc<engine::InMemoryEngine>, key_id: &str) -> String {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    let key = state
        .key_mut_in(REGION, key_id)
        .expect("the key is held");
    let material = mint_material(
        key_spec_profile(KEY_SPEC_SYMMETRIC_DEFAULT).expect("the default key spec has a profile"),
    )
    .expect("the platform produces entropy for key material");
    let key_material_id = material.id().expect("symmetric key material carries an id");
    key.material = material;
    key.current_key_material_id = Some(key_material_id.clone());
    key.key_state = "Enabled".to_string();
    key.enabled = true;
    key_material_id
}

/// Rule 3, continued: the wrapping key `GetParametersForImport` hands out really is the
/// public half of a key pair this service holds, so key material wrapped with it is
/// material `ImportKeyMaterial` opens — and the key then holds exactly those bytes.
#[tokio::test]
async fn imported_key_material_is_unwrapped_and_becomes_the_key_s_material() {
    let router = router();

    // Rows: the key the material is imported into and the kind of material it holds,
    // the wrapping algorithm the caller uses, and how the caller states the expiry.
    let rows: Vec<(&str, Value, MaterialKind, &str, Value)> = vec![
        (
            "a symmetric encryption key, material that does not expire",
            json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
            MaterialKind::Aes256,
            "RSAES_OAEP_SHA_256",
            json!({"ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE"}),
        ),
        (
            "a symmetric encryption key, material that expires",
            json!({"Origin": "EXTERNAL", "KeySpec": "SYMMETRIC_DEFAULT"}),
            MaterialKind::Aes256,
            "RSAES_OAEP_SHA_1",
            json!({"ExpirationModel": "KEY_MATERIAL_EXPIRES", "ValidTo": in_days(30.0),
                   "KeyMaterialDescription": "imported by the test"}),
        ),
        (
            "an HMAC key",
            json!({"Origin": "EXTERNAL", "KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            MaterialKind::Hmac(32),
            "RSAES_OAEP_SHA_256",
            json!({"ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE"}),
        ),
    ];
    for (case, create, kind, wrapping_algorithm, expiry) in rows {
        let metadata = create_key(&router, &create).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");

        let parameters = ok(
            &router,
            "GetParametersForImport",
            &json!({
                "KeyId": key_id,
                "WrappingAlgorithm": wrapping_algorithm,
                "WrappingKeySpec": "RSA_2048",
            }),
        )
        .await;
        let public_key = unb64(case, &parameters, "PublicKey");
        let import_token = text(case, &parameters, "ImportToken");

        // The material the caller generates, and the identifier KMS derives from it —
        // computed here from the same bytes, so the answer proves the key ended up
        // holding exactly what was wrapped.
        let material = random_bytes(kind.material_len().expect("the kind holds bytes"))
            .expect("the platform produces entropy");
        let expected_id = KeyMaterial::from_bytes(kind, &material)
            .expect("the bytes are material of this kind")
            .id();
        let wrapped = wrap(case, &public_key, wrapping_algorithm, &material);

        let mut request = expiry.clone();
        request["KeyId"] = json!(key_id);
        request["ImportToken"] = json!(import_token);
        request["EncryptedKeyMaterial"] = json!(b64(&wrapped));
        let imported = ok(&router, "ImportKeyMaterial", &request).await;
        assert_eq!(
            imported["KeyId"].as_str().unwrap_or_default(),
            key_arn,
            "case {case:?}: the answer names the key by its ARN, got {imported}"
        );
        assert_eq!(
            imported["KeyMaterialId"].as_str().map(str::to_string),
            expected_id,
            "case {case:?}: the key must hold exactly the material that was wrapped, got \
             {imported}"
        );

        // The key has key material now, so it is usable and reports what the caller
        // stated about that material.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"].as_str().unwrap_or_default(),
            "Enabled",
            "case {case:?}: a key with key material is usable, got {described}"
        );
        assert_eq!(
            described["Origin"].as_str().unwrap_or_default(),
            "EXTERNAL",
            "case {case:?}: importing material does not change the origin, got {described}"
        );
        assert_eq!(
            described["ExpirationModel"].as_str().unwrap_or_default(),
            expiry["ExpirationModel"].as_str().unwrap_or_default(),
            "case {case:?}: expected the stated expiration model, got {described}"
        );
        assert_eq!(
            described["ValidTo"].as_f64(),
            expiry["ValidTo"].as_f64(),
            "case {case:?}: expected the stated expiry, got {described}"
        );
        assert_eq!(
            described["CurrentKeyMaterialId"].as_str().map(str::to_string),
            expected_id,
            "case {case:?}: expected the imported material to be the current one, got \
             {described}"
        );

        // The key-materials reader reports the same import: the members that are only
        // present for an EXTERNAL-origin symmetric encryption key are filled from what
        // the caller stated. `ALL_KEY_MATERIAL` is only valid for a key whose material
        // can rotate, which is the symmetric encryption rows.
        if matches!(kind, MaterialKind::Aes256) {
            let listed = ok(
                &router,
                "ListKeyRotations",
                &json!({"KeyId": key_id, "IncludeKeyMaterial": "ALL_KEY_MATERIAL"}),
            )
            .await;
            let entry = listed["Rotations"][0].clone();
            assert_eq!(
                entry["KeyMaterialId"].as_str().map(str::to_string),
                expected_id,
                "case {case:?}: the listing names the imported material, got {listed}"
            );
            assert_eq!(
                entry["ImportState"].as_str().unwrap_or_default(),
                "IMPORTED",
                "case {case:?}: the material is imported into KMS, got {listed}"
            );
            assert_eq!(
                entry["ExpirationModel"].as_str().unwrap_or_default(),
                expiry["ExpirationModel"].as_str().unwrap_or_default(),
                "case {case:?}: expected the stated expiration model, got {listed}"
            );
            assert_eq!(
                entry["ValidTo"].as_f64(),
                expiry["ValidTo"].as_f64(),
                "case {case:?}: expected the stated expiry, got {listed}"
            );
            assert_eq!(
                entry["KeyMaterialDescription"].as_str(),
                expiry["KeyMaterialDescription"].as_str(),
                "case {case:?}: expected the stated description, got {listed}"
            );
        }
    }
}

/// Rule 3, continued: an import that cannot be served by the parameters it names, and
/// the revert when the material is deleted again.
#[tokio::test]
async fn an_import_is_refused_unless_it_matches_the_parameters_it_names() {
    let router = router();
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let other = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let other_key_id = text("setup", &other, "KeyId");

    let parameters = ok(
        &router,
        "GetParametersForImport",
        &json!({
            "KeyId": key_id,
            "WrappingAlgorithm": "RSAES_OAEP_SHA_256",
            "WrappingKeySpec": "RSA_2048",
        }),
    )
    .await;
    let public_key = unb64("setup", &parameters, "PublicKey");
    let import_token = text("setup", &parameters, "ImportToken");
    let material = random_bytes(32).expect("the platform produces entropy");
    let wrapped = wrap("setup", &public_key, "RSAES_OAEP_SHA_256", &material);

    // Rows: an import that cannot be served, and the refusal it meets. They differ in
    // WHAT does not match: the token, the key the token was issued for, the wrapping,
    // the material's length, and how the request states its expiry.
    let short = random_bytes(16).expect("the platform produces entropy");
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a token this service never minted",
            json!({
                "KeyId": key_id,
                "ImportToken": b64(&[0u8; 32]),
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
            "InvalidImportTokenException",
        ),
        (
            "a token issued for another key",
            json!({
                "KeyId": other_key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
            "InvalidImportTokenException",
        ),
        (
            "key material that does not open under the wrapping key",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&[7u8; 256]),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
            "InvalidCiphertextException",
        ),
        (
            "key material of the wrong length for the key",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrap(
                    "short material",
                    &public_key,
                    "RSAES_OAEP_SHA_256",
                    &short,
                )),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
            }),
            "IncorrectKeyMaterialException",
        ),
        (
            "material that expires with no expiry stated",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_EXPIRES",
            }),
            "ValidationException",
        ),
        (
            "material that does not expire with an expiry stated",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
                "ValidTo": in_days(30.0),
            }),
            "ValidationException",
        ),
        (
            "an expiry in the past",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ValidTo": in_days(-1.0),
            }),
            "ValidationException",
        ),
        (
            "an expiry past the longest the service admits",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ValidTo": in_days(366.0),
            }),
            "ValidationException",
        ),
        (
            "an expiry no clock can represent",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ValidTo": 9.0e18_f64,
            }),
            "ValidationException",
        ),
        (
            "an expiry before the epoch",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ValidTo": -9.0e18_f64,
            }),
            "ValidationException",
        ),
        (
            "a key material id with material that has never been in the key",
            json!({
                "KeyId": key_id,
                "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
                "ImportType": "EXISTING_KEY_MATERIAL",
                "KeyMaterialId": "a".repeat(64),
            }),
            "IncorrectKeyMaterialException",
        ),
    ];
    for (case, request, expected_code) in rows {
        let (status, code, _message) =
            refused(&router, REGION, "ImportKeyMaterial", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}"
        );
        // Nothing was imported: the key still waits for its key material.
        assert_eq!(
            describe(&router, &key_id).await["KeyState"]
                .as_str()
                .unwrap_or_default(),
            "PendingImport",
            "case {case:?}: a refused import leaves the key waiting"
        );
    }

    // The import the rows above could not make, made: the key becomes usable, and what
    // it encrypts can be decrypted with the material the caller supplied.
    let imported = ok(
        &router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": import_token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
        }),
    )
    .await;
    let key_material_id = text("imported", &imported, "KeyMaterialId");
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"imported material")}),
    )
    .await;
    let opened = ok(
        &router,
        "Decrypt",
        &json!({"CiphertextBlob": text("imported", &sealed, "CiphertextBlob")}),
    )
    .await;
    assert_eq!(
        unb64("imported", &opened, "Plaintext"),
        b"imported material".to_vec(),
        "case imported: a key with imported material encrypts and decrypts"
    );

    // Deleting the material reverts the key to waiting for material, and the same
    // material can be imported again with the token it was linked to.
    let deleted = ok(
        &router,
        "DeleteImportedKeyMaterial",
        &json!({"KeyId": key_id}),
    )
    .await;
    assert_eq!(
        deleted["KeyMaterialId"].as_str().unwrap_or_default(),
        key_material_id,
        "case deleted: the answer identifies the deleted key material, got {deleted}"
    );
    assert_eq!(
        describe(&router, &key_id).await["KeyState"]
            .as_str()
            .unwrap_or_default(),
        "PendingImport",
        "case deleted: the key waits for its key material again"
    );
    let reimported = ok(
        &router,
        "ImportKeyMaterial",
        &json!({
            "KeyId": key_id,
            "ImportToken": import_token,
            "EncryptedKeyMaterial": b64(&wrapped),
            "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE",
        }),
    )
    .await;
    assert_eq!(
        reimported["KeyMaterialId"].as_str().unwrap_or_default(),
        key_material_id,
        "case reimported: the same material carries the same identifier, got {reimported}"
    );
    assert_eq!(
        describe(&router, &key_id).await["KeyState"]
            .as_str()
            .unwrap_or_default(),
        "Enabled",
        "case reimported: the key is usable again"
    );
}

/// `days` from now, as an awsJson1_1 timestamp.
fn in_days(days: f64) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64();
    (now + days * 24.0 * 60.0 * 60.0).trunc()
}

/// Rule 4: `UpdateCustomKeyStore` resolves its target first, and every
/// `CustomKeyStoreId` that names no key store of this account and Region meets the same
/// refusal — with no key store brought into existence.
#[tokio::test]
async fn update_custom_key_store_collapses_every_unresolvable_target_onto_one_refusal() {
    let (router, engine) = router_with_engine();
    let held = cloud_hsm_store(REGION, "held", "cluster-2vkpqmfjzck");
    let elsewhere = cloud_hsm_store(OTHER_REGION, "elsewhere", "cluster-5kq2jmzvb7tx");
    seed(&engine, &[held.clone(), elsewhere.clone()]);

    // Rows: ids that name no key store HERE. They differ in how they fail: a
    // well-formed id nothing holds, a value that is not a key store id at all, one that
    // is only the prefix, the shortest and the longest the modelled length admits, and
    // the id of a key store this account holds in another Region.
    let rows: Vec<(&str, String)> = vec![
        ("a well-formed id nothing holds", "cks-0000000000000000".to_string()),
        ("not a key store id at all", "not-a-cks".to_string()),
        ("the prefix alone", "cks-".to_string()),
        ("the shortest id the model admits", "c".to_string()),
        ("the longest id the model admits", "c".repeat(64)),
        ("an id of another Region", elsewhere.custom_key_store_id.clone()),
    ];
    for (case, custom_key_store_id) in rows {
        let (status, code, message) = refused(
            &router,
            REGION,
            "UpdateCustomKeyStore",
            &json!({
                "CustomKeyStoreId": custom_key_store_id,
                "NewCustomKeyStoreName": "renamed",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "CustomKeyStoreNotFoundException",
            "case {case:?}: expected CustomKeyStoreNotFoundException, got {code:?}"
        );
        assert!(
            message.is_empty(),
            "case {case:?}: the refusal carries the error code alone, got {message:?}"
        );

        // The reader agrees about the same id, so a client resolves a key store the
        // same way through either operation.
        let (_status, describe_code, describe_message) = refused(
            &router,
            REGION,
            "DescribeCustomKeyStores",
            &json!({"CustomKeyStoreId": custom_key_store_id}),
        )
        .await;
        assert_eq!(
            describe_code, "CustomKeyStoreNotFoundException",
            "case {case:?}: DescribeCustomKeyStores must agree, got {describe_code:?}"
        );
        assert!(
            describe_message.is_empty(),
            "case {case:?}: DescribeCustomKeyStores carries the error code alone, got \
             {describe_message:?}"
        );
    }

    // No refused update brought a key store into existence: the Region still holds the
    // one key store it was seeded with, unchanged.
    let listed = ok(&router, "DescribeCustomKeyStores", &json!({})).await;
    let entries = listed["CustomKeyStores"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        entries.len(),
        1,
        "case after the refusals: expected only the seeded key store, got {listed}"
    );
    assert_eq!(
        entries[0]["CustomKeyStoreId"].as_str().unwrap_or_default(),
        held.custom_key_store_id,
        "case after the refusals: expected the seeded key store, got {listed}"
    );
    assert_eq!(
        entries[0]["CustomKeyStoreName"].as_str().unwrap_or_default(),
        "held",
        "case after the refusals: a refused update changes no name, got {listed}"
    );
}

/// Rule 4, continued: the properties an update IS given are applied to the key store,
/// and read back through `DescribeCustomKeyStores`.
#[tokio::test]
async fn update_custom_key_store_applies_the_properties_it_is_given() {
    let (router, engine) = router_with_engine();
    let hsm = cloud_hsm_store(REGION, "hsm", "cluster-2vkpqmfjzck");
    let other_hsm = cloud_hsm_store(REGION, "taken", "cluster-7bzcmrtvj4k5");
    let mut connected = cloud_hsm_store(REGION, "connected", "cluster-4xvbjs6r2tzqvmi");
    connected.connection_state = "CONNECTED".to_string();
    let xks = external_store(REGION, "xks", "https://xks.example.com", "/kms/xks/v1");
    seed(
        &engine,
        &[
            hsm.clone(),
            other_hsm.clone(),
            connected.clone(),
            xks.clone(),
        ],
    );

    // Rows: an update of an CloudHSM key store, and what it must leave reported. The
    // trust anchor certificate is NOT a property of the request: clusters that share a
    // backup history have the same cluster certificate, so it stays as it is.
    let hsm_rows: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the friendly name alone",
            json!({"NewCustomKeyStoreName": "hsm-renamed"}),
            "hsm-renamed",
            "cluster-2vkpqmfjzck",
        ),
        (
            "a related cluster alone",
            json!({"CloudHsmClusterId": "cluster-5kq2jmzvb7tx"}),
            "hsm-renamed",
            "cluster-5kq2jmzvb7tx",
        ),
        (
            "both at once",
            json!({
                "NewCustomKeyStoreName": "hsm-final",
                "CloudHsmClusterId": "cluster-7bzcmrtvj4k5",
                "KeyStorePassword": "kmsuser-pw",
            }),
            "hsm-final",
            "cluster-7bzcmrtvj4k5",
        ),
    ];
    for (case, mut request, expected_name, expected_cluster) in hsm_rows {
        request["CustomKeyStoreId"] = json!(hsm.custom_key_store_id);
        let answer = ok(&router, "UpdateCustomKeyStore", &request).await;
        let empty = answer.is_null() || answer.as_object().is_some_and(|m| m.is_empty());
        assert!(
            empty,
            "case {case:?}: UpdateCustomKeyStore returns no property values, got {answer}"
        );
        let entry = reported(&router, &hsm.custom_key_store_id).await;
        assert_eq!(
            entry["CustomKeyStoreName"].as_str().unwrap_or_default(),
            expected_name,
            "case {case:?}: expected the reported name to be {expected_name}, got {entry}"
        );
        assert_eq!(
            entry["CloudHsmClusterId"].as_str().unwrap_or_default(),
            expected_cluster,
            "case {case:?}: expected the reported cluster to be {expected_cluster}, got \
             {entry}"
        );
        assert_eq!(
            entry["TrustAnchorCertificate"].as_str().unwrap_or_default(),
            "-----BEGIN CERTIFICATE-----\nZmlyc3Q=\n-----END CERTIFICATE-----",
            "case {case:?}: a related cluster has the same cluster certificate, got {entry}"
        );
    }

    // Rows: an update of an external key store, and what it must leave reported.
    let xks_rows: Vec<(&str, Value, Value)> = vec![
        (
            "the proxy endpoint and path",
            json!({
                "XksProxyUriEndpoint": "https://xks-moved.example.com",
                "XksProxyUriPath": "/example/kms/xks/v2",
            }),
            json!({
                "Connectivity": "PUBLIC_ENDPOINT",
                "AccessKeyId": "ABCDEFGHIJKLMNOPQRST",
                "UriEndpoint": "https://xks-moved.example.com",
                "UriPath": "/example/kms/xks/v2",
            }),
        ),
        (
            "the proxy authentication credential",
            json!({
                "XksProxyAuthenticationCredential": {
                    "AccessKeyId": "UVWXYZ234567ABCDEFGH",
                    "RawSecretAccessKey": "c2VjcmV0LXRoZS1wcm94eS1ob2xkcy1hbmQta21zLWtub3dz",
                },
            }),
            json!({
                "Connectivity": "PUBLIC_ENDPOINT",
                "AccessKeyId": "UVWXYZ234567ABCDEFGH",
                "UriEndpoint": "https://xks-moved.example.com",
                "UriPath": "/example/kms/xks/v2",
            }),
        ),
        (
            "the connectivity, with the endpoint service it is then reached through",
            json!({
                "XksProxyConnectivity": "VPC_ENDPOINT_SERVICE",
                "XksProxyUriEndpoint": "https://xks-vpce.example.com",
                "XksProxyVpcEndpointServiceName":
                    "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890",
                "XksProxyVpcEndpointServiceOwner": "111122223333",
            }),
            json!({
                "Connectivity": "VPC_ENDPOINT_SERVICE",
                "AccessKeyId": "UVWXYZ234567ABCDEFGH",
                "UriEndpoint": "https://xks-vpce.example.com",
                "UriPath": "/example/kms/xks/v2",
                "VpcEndpointServiceName":
                    "com.amazonaws.vpce.us-east-1.vpce-svc-0abcdef1234567890",
                "VpcEndpointServiceOwner": "111122223333",
            }),
        ),
    ];
    for (case, mut request, expected_proxy) in xks_rows {
        request["CustomKeyStoreId"] = json!(xks.custom_key_store_id);
        ok(&router, "UpdateCustomKeyStore", &request).await;
        let entry = reported(&router, &xks.custom_key_store_id).await;
        assert_eq!(
            entry["XksProxyConfiguration"], expected_proxy,
            "case {case:?}: expected the reported proxy configuration to be \
             {expected_proxy}, got {entry}"
        );
    }

    // Rows: updates that cannot be applied, and the refusal each meets. None of them
    // changes anything.
    let refusals: Vec<(&str, String, Value, &str)> = vec![
        (
            "a name another key store of the account already has",
            hsm.custom_key_store_id.clone(),
            json!({"NewCustomKeyStoreName": "taken"}),
            "CustomKeyStoreNameInUseException",
        ),
        (
            "a cluster change while the key store is connected",
            connected.custom_key_store_id.clone(),
            json!({"CloudHsmClusterId": "cluster-5kq2jmzvb7tx"}),
            "CustomKeyStoreInvalidStateException",
        ),
        (
            "a proxy property on an CloudHSM key store",
            hsm.custom_key_store_id.clone(),
            json!({"XksProxyUriPath": "/kms/xks/v1"}),
            "ValidationException",
        ),
        (
            "a cluster property on an external key store",
            xks.custom_key_store_id.clone(),
            json!({"CloudHsmClusterId": "cluster-5kq2jmzvb7tx"}),
            "ValidationException",
        ),
        (
            "a proxy URI another external key store of the Region already has",
            xks.custom_key_store_id.clone(),
            json!({"XksProxyUriEndpoint": "https://xks-twin.example.com"}),
            "XksProxyUriInUseException",
        ),
    ];
    // The twin the last row collides with, so the uniqueness rule has something to
    // collide against.
    seed(
        &engine,
        &[external_store(
            REGION,
            "xks-twin",
            "https://xks-twin.example.com",
            "/example/kms/xks/v2",
        )],
    );
    let before = reported(&router, &hsm.custom_key_store_id).await;
    for (case, custom_key_store_id, mut request, expected_code) in refusals {
        request["CustomKeyStoreId"] = json!(custom_key_store_id);
        let (status, code, _message) =
            refused(&router, REGION, "UpdateCustomKeyStore", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?}"
        );
    }
    let after = reported(&router, &hsm.custom_key_store_id).await;
    assert_eq!(
        after, before,
        "case after the refusals: a refused update changes nothing"
    );

    // Renaming a key store to the name it already has is not a conflict with itself.
    ok(
        &router,
        "UpdateCustomKeyStore",
        &json!({
            "CustomKeyStoreId": hsm.custom_key_store_id,
            "NewCustomKeyStoreName": "hsm-final",
        }),
    )
    .await;
    let entry = reported(&router, &hsm.custom_key_store_id).await;
    assert_eq!(
        entry["CustomKeyStoreName"].as_str().unwrap_or_default(),
        "hsm-final",
        "case renaming to its own name: expected the name to stand, got {entry}"
    );
}

/// The entry `DescribeCustomKeyStores` reports for one key store.
async fn reported(
    router: &smithy_server_runtime::MultiServiceRouter,
    custom_key_store_id: &str,
) -> Value {
    let answer = ok(
        router,
        "DescribeCustomKeyStores",
        &json!({"CustomKeyStoreId": custom_key_store_id}),
    )
    .await;
    answer["CustomKeyStores"][0].clone()
}

/// A custom key store backed by an CloudHSM cluster, as a create leaves it: never
/// connected.
fn cloud_hsm_store(region: &str, name: &str, cloud_hsm_cluster_id: &str) -> CustomKeyStore {
    CustomKeyStore::new(
        mint_custom_key_store_id().expect("the platform produces entropy for a key store id"),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::CloudHsm(CloudHsmCluster {
            cloud_hsm_cluster_id: cloud_hsm_cluster_id.to_string(),
            trust_anchor_certificate:
                "-----BEGIN CERTIFICATE-----\nZmlyc3Q=\n-----END CERTIFICATE-----".to_string(),
        }),
    )
}

/// A custom key store backed by an external key store proxy on a public endpoint.
fn external_store(region: &str, name: &str, uri_endpoint: &str, uri_path: &str) -> CustomKeyStore {
    CustomKeyStore::new(
        mint_custom_key_store_id().expect("the platform produces entropy for a key store id"),
        region.to_string(),
        name.to_string(),
        CustomKeyStoreBacking::External(XksProxy {
            connectivity: "PUBLIC_ENDPOINT".to_string(),
            access_key_id: "ABCDEFGHIJKLMNOPQRST".to_string(),
            uri_endpoint: uri_endpoint.to_string(),
            uri_path: uri_path.to_string(),
            vpc_endpoint_service_name: None,
            vpc_endpoint_service_owner: None,
        }),
    )
}

/// Seed key stores into the store the router was built over, in order.
fn seed(engine: &Arc<engine::InMemoryEngine>, stores: &[CustomKeyStore]) {
    let state = service_state(&**engine).expect("the KMS state store is reachable");
    let mut state = state.lock();
    for store in stores {
        state.insert_custom_key_store(store.clone());
    }
}

/// Rule for the key types that can never rotate: `RotateKeyOnDemand` refuses them with
/// the error code alone, no rotation is recorded, and the rotation readers still answer.
#[tokio::test]
async fn a_key_type_that_cannot_rotate_is_refused_and_records_no_rotation() {
    let router = router();

    // Rows: a key whose material on-demand rotation does not apply to. They differ in
    // the key spec and the usage — asymmetric encryption, asymmetric signing, elliptic
    // curve, HMAC — since "on-demand key rotation is supported only on symmetric
    // encryption KMS keys".
    let rows: Vec<(&str, Value)> = vec![
        (
            "an asymmetric signing key",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "an asymmetric encryption key",
            json!({"KeySpec": "RSA_3072", "KeyUsage": "ENCRYPT_DECRYPT"}),
        ),
        (
            "an elliptic curve key",
            json!({"KeySpec": "ECC_NIST_P256", "KeyUsage": "SIGN_VERIFY"}),
        ),
        (
            "an HMAC key",
            json!({"KeySpec": "HMAC_384", "KeyUsage": "GENERATE_VERIFY_MAC"}),
        ),
    ];
    for (case, request) in rows {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let (status, code, message) = refused(
            &router,
            REGION,
            "RotateKeyOnDemand",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException, got {code:?}"
        );
        assert!(
            message.is_empty(),
            "case {case:?}: the refusal carries the error code alone, got {message:?}"
        );

        // The rotation readers still answer for such a key: it has no rotation, which
        // is a value, not a failure.
        let status_answer = ok(
            &router,
            "GetKeyRotationStatus",
            &json!({"KeyId": key_id}),
        )
        .await;
        assert_eq!(
            status_answer["KeyRotationEnabled"],
            json!(false),
            "case {case:?}: expected KeyRotationEnabled false, got {status_answer}"
        );
        let rotations = ok(&router, "ListKeyRotations", &json!({"KeyId": key_id})).await;
        assert_eq!(
            rotations["Rotations"],
            json!([]),
            "case {case:?}: expected no rotations, got {rotations}"
        );
        assert_eq!(
            rotations["Truncated"],
            json!(false),
            "case {case:?}: a complete list is not truncated, got {rotations}"
        );

        // Asking for the key materials of a key whose material never rotates is the
        // one parameter that is not supported for it.
        let (_status, include_code, _message) = refused(
            &router,
            REGION,
            "ListKeyRotations",
            &json!({"KeyId": key_id, "IncludeKeyMaterial": "ALL_KEY_MATERIAL"}),
        )
        .await;
        assert_eq!(
            include_code, "UnsupportedOperationException",
            "case {case:?}: expected UnsupportedOperationException for ALL_KEY_MATERIAL, \
             got {include_code:?}"
        );
    }
}

/// The positive control for the rule above: a symmetric encryption key DOES rotate, and
/// each rotation becomes observable through `ListKeyRotations`.
#[tokio::test]
async fn an_on_demand_rotation_is_reported_by_the_rotation_reader() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "rotate me"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let first_material = text("setup", &metadata, "CurrentKeyMaterialId");

    let empty = ok(&router, "ListKeyRotations", &json!({"KeyId": key_id})).await;
    assert_eq!(
        empty["Rotations"],
        json!([]),
        "case before rotating: a key that has not rotated lists no rotation, got {empty}"
    );

    // Rows: how many rotations have happened. Each one appends an entry, oldest first,
    // and only the newest material is the one the key encrypts under.
    let mut expected_rotations = 0usize;
    for round in 1..=3usize {
        let case = format!("round {round}");
        rotate_on_demand(&router, &case, &key_id).await;
        expected_rotations += 1;
        let listed = ok(&router, "ListKeyRotations", &json!({"KeyId": key_id})).await;
        let entries = listed["Rotations"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            expected_rotations,
            "case round {round}: expected {expected_rotations} rotations, got {listed}"
        );
        for (index, entry) in entries.iter().enumerate() {
            assert_eq!(
                entry["RotationType"].as_str().unwrap_or_default(),
                "ON_DEMAND",
                "case round {round} entry {index}: expected RotationType ON_DEMAND, got \
                 {entry}"
            );
            assert_eq!(
                entry["KeyId"].as_str().unwrap_or_default(),
                key_arn,
                "case round {round} entry {index}: expected the entry to name the key by its \
                 ARN, got {entry}"
            );
            assert!(
                entry["RotationDate"].as_f64().unwrap_or_default() > 0.0,
                "case round {round} entry {index}: expected a rotation date, got {entry}"
            );
            let expected_state = if index + 1 == entries.len() {
                "CURRENT"
            } else {
                "NON_CURRENT"
            };
            assert_eq!(
                entry["KeyMaterialState"].as_str().unwrap_or_default(),
                expected_state,
                "case round {round} entry {index}: expected KeyMaterialState \
                 {expected_state}, got {entry}"
            );
        }
        // The material the key encrypts under now is the one the newest rotation made
        // current, which `DescribeKey` reports from the same record.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["CurrentKeyMaterialId"],
            entries[entries.len() - 1]["KeyMaterialId"],
            "case round {round}: the newest rotation's material is the current one, got \
             {described}"
        );
        assert_ne!(
            described["CurrentKeyMaterialId"].as_str().unwrap_or_default(),
            first_material,
            "case round {round}: a rotation replaces the material the key was created \
             with, got {described}"
        );
    }

    // `ALL_KEY_MATERIAL` adds the material the key was created with, which is not a
    // rotation and so carries neither a rotation date nor a rotation type.
    let all = ok(
        &router,
        "ListKeyRotations",
        &json!({"KeyId": key_id, "IncludeKeyMaterial": "ALL_KEY_MATERIAL"}),
    )
    .await;
    let entries = all["Rotations"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        entries.len(),
        expected_rotations + 1,
        "case ALL_KEY_MATERIAL: expected the first material beside the rotations, got {all}"
    );
    assert_eq!(
        entries[0]["KeyMaterialId"].as_str().unwrap_or_default(),
        first_material,
        "case ALL_KEY_MATERIAL: the first entry is the material the key was created \
         with, got {all}"
    );
    assert!(
        entries[0]["RotationDate"].is_null() && entries[0]["RotationType"].is_null(),
        "case ALL_KEY_MATERIAL: the first material is not a rotation, got {all}"
    );

    // Paging: rows differ in the page size, and a page that ends before the list does
    // is truncated and names where to resume.
    for limit in [1usize, 2, 4] {
        let page = ok(
            &router,
            "ListKeyRotations",
            &json!({"KeyId": key_id, "Limit": limit}),
        )
        .await;
        let page_entries = page["Rotations"].as_array().cloned().unwrap_or_default();
        let expected_len = limit.min(expected_rotations);
        assert_eq!(
            page_entries.len(),
            expected_len,
            "case Limit {limit}: expected {expected_len} entries, got {page}"
        );
        let truncated = expected_len < expected_rotations;
        assert_eq!(
            page["Truncated"],
            json!(truncated),
            "case Limit {limit}: expected Truncated {truncated}, got {page}"
        );
        if truncated {
            let marker = page["NextMarker"]
                .as_str()
                .unwrap_or_else(|| panic!("case Limit {limit}: expected a NextMarker in {page}"));
            let rest = ok(
                &router,
                "ListKeyRotations",
                &json!({"KeyId": key_id, "Marker": marker}),
            )
            .await;
            let rest_entries = rest["Rotations"].as_array().cloned().unwrap_or_default();
            assert_eq!(
                rest_entries.len(),
                expected_rotations - expected_len,
                "case Limit {limit}: expected the rest of the list after the marker, got \
                 {rest}"
            );
        } else {
            assert!(
                page["NextMarker"].is_null(),
                "case Limit {limit}: a complete page names nowhere to resume, got {page}"
            );
        }
    }

    // A marker that names no key material of this key resumes nowhere, which is not a
    // valid marker.
    let (_status, code, _message) = refused(
        &router,
        REGION,
        "ListKeyRotations",
        &json!({"KeyId": key_id, "Marker": "deadbeef"}),
    )
    .await;
    assert_eq!(
        code, "InvalidMarkerException",
        "case a marker from nowhere: expected InvalidMarkerException, got {code:?}"
    );
}

/// `ListKeyPolicies` reports the one policy name every key carries, and is refused for
/// exactly the identifiers the policy reader is refused for.
#[tokio::test]
async fn list_key_policies_reports_the_one_policy_name_a_key_carries() {
    let router = router();
    let account = engine::emulator_account_id();

    // Rows: the keys a policy is listed for. They differ in key spec, usage, origin and
    // whether the key is enabled — none of which changes the listing, because every KMS
    // key carries exactly one policy.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("a symmetric encryption key", json!({}), false),
        (
            "an asymmetric signing key",
            json!({"KeySpec": "RSA_2048", "KeyUsage": "SIGN_VERIFY"}),
            false,
        ),
        (
            "an HMAC key",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            false,
        ),
        (
            "a key created with no key material",
            json!({"Origin": "EXTERNAL"}),
            false,
        ),
        ("a disabled key", json!({}), true),
    ];
    for (case, request, disable) in rows {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        if disable {
            ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;
        }
        // Rows within the row: every form of identifier that names this key.
        for identifier in [key_id.clone(), key_arn.clone()] {
            let listed = ok(&router, "ListKeyPolicies", &json!({"KeyId": identifier})).await;
            assert_eq!(
                listed["PolicyNames"],
                json!(["default"]),
                "case {case:?} by {identifier}: expected the one policy name, got {listed}"
            );
            assert_eq!(
                listed["Truncated"],
                json!(false),
                "case {case:?} by {identifier}: a one-item list is not truncated, got \
                 {listed}"
            );
            assert!(
                listed["NextMarker"].is_null(),
                "case {case:?} by {identifier}: a complete list names nowhere to resume, \
                 got {listed}"
            );
            // The name the listing reports is the one `GetKeyPolicy` serves.
            let policy = ok(
                &router,
                "GetKeyPolicy",
                &json!({"KeyId": identifier, "PolicyName": "default"}),
            )
            .await;
            assert!(
                !policy["Policy"].as_str().unwrap_or_default().is_empty(),
                "case {case:?} by {identifier}: expected the named policy to be served, \
                 got {policy}"
            );
        }
    }

    // Rows: identifiers that name no key, the refusal `ListKeyPolicies` meets and the
    // refusal `GetKeyPolicy` meets. The two differ on ONE row: an ARN scoped to this
    // account that resolves to no KMS key is reported by `ListKeyPolicies` as a resource
    // that does not exist, which is the refusal the recording of that operation carries.
    let refusals: Vec<(&str, String, &str, &str)> = vec![
        (
            "an unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "NotFoundException",
            "NotFoundException",
        ),
        (
            "an ARN that is not a KMS key ARN",
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
            "NotFoundException",
            "InvalidArnException",
        ),
        (
            "an ARN whose account is not an account id",
            format!("arn:aws:kms:{REGION}:notanumericaccount:key/abc"),
            "AccessDeniedException",
            "AccessDeniedException",
        ),
    ];
    for (case, identifier, listing_code, read_code) in refusals {
        for (operation, expected_code) in
            [("ListKeyPolicies", listing_code), ("GetKeyPolicy", read_code)]
        {
            let (status, code, _message) =
                refused(&router, REGION, operation, &json!({"KeyId": identifier})).await;
            assert_eq!(
                status, 400,
                "case {case:?} on {operation}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, expected_code,
                "case {case:?} on {operation}: expected {expected_code}, got {code:?}"
            );
        }
    }
}

/// A `Marker` on the one-item policy listing resumes after the only name there is, so
/// the page it answers is the rest of the list: nothing.
#[tokio::test]
async fn a_policy_listing_resumed_after_its_only_name_is_empty() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "policy paging"})).await;
    let key_id = text("setup", &metadata, "KeyId");

    for marker in ["default", "anything"] {
        let listed = ok(
            &router,
            "ListKeyPolicies",
            &json!({"KeyId": key_id, "Marker": marker}),
        )
        .await;
        assert_eq!(
            listed["PolicyNames"],
            json!([]),
            "case marker {marker:?}: expected nothing after the only policy name, got \
             {listed}"
        );
        assert_eq!(
            listed["Truncated"],
            json!(false),
            "case marker {marker:?}: expected Truncated false, got {listed}"
        );
    }

    // And a `Limit` cannot truncate a one-item list: the modelled minimum is 1.
    for limit in [1, 1000] {
        let listed = ok(
            &router,
            "ListKeyPolicies",
            &json!({"KeyId": key_id, "Limit": limit}),
        )
        .await;
        assert_eq!(
            listed["PolicyNames"],
            json!(["default"]),
            "case Limit {limit}: expected the one policy name, got {listed}"
        );
        assert_eq!(
            listed["Truncated"],
            json!(false),
            "case Limit {limit}: expected Truncated false, got {listed}"
        );
    }
}
