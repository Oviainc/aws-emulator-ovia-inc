// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS and CLASSIFIES the refusals a well-formed request can meet,
//! driven end-to-end over the wire through the shared [`super::harness`].
//!
//! Five rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over:
//!
//! 1. A constraint violation on an input member the model marks `@sensitive` states the
//!    member and the bound it broke and WITHHOLDS the value; one on any other member
//!    keeps the value operand. The rows differ in the operation, the member, the kind of
//!    member (blob, string) and which bound was broken.
//! 2. A key whose `KeyUsage` does not permit the operation is refused by a sentence
//!    naming the key, that usage and the operation. The rows differ in the operation, in
//!    the usage the key carries and in its key spec.
//! 3. An algorithm the key's own key spec does not support is refused by a sentence
//!    naming the algorithm and that key spec. The rows differ in the operation, the kind
//!    of algorithm (encryption, signing, MAC) and the key spec.
//! 4. `ImportKeyMaterial`'s two `ValidTo` rules are refused SEPARATELY, each stating only
//!    the rule the request broke. The rows differ in how far outside each rule the value
//!    is, including values no clock can represent.
//! 5. An identifier that names an ALIAS is refused for its KIND by an operation whose
//!    `KeyId` takes a key id or key ARN only — before anything is resolved, so an alias
//!    that does resolve and one that names nothing are refused identically. The rows
//!    differ in the operation and in the form and resolvability of the alias.

use serde_json::{json, Value};

use super::harness::{
    b64, call, create_alias, create_key, describe, error_code, error_message, ok, refused, router,
    text, unb64, wrap, REGION,
};

/// `len` bytes of filler: what a row sends for a member whose LENGTH is what the row is
/// about.
fn filler(len: usize) -> Vec<u8> {
    vec![b'x'; len]
}

/// The `ValidationException` envelope the framework renders a single constraint
/// violation in, with `clause` as that violation.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// A key of `key_spec`/`key_usage`, as (key id, key ARN).
async fn key_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_spec: &str,
    key_usage: &str,
) -> (String, String) {
    let case = format!("{key_spec} {key_usage}");
    let metadata = create_key(
        router,
        &json!({"Description": case, "KeySpec": key_spec, "KeyUsage": key_usage}),
    )
    .await;
    (
        text(&case, &metadata, "KeyId"),
        text(&case, &metadata, "Arn"),
    )
}

/// A ciphertext of the symmetric encryption key `key_id`, as the wire carries it.
async fn ciphertext_of(
    router: &smithy_server_runtime::MultiServiceRouter,
    key_id: &str,
) -> String {
    let answer = ok(
        router,
        "Encrypt",
        &json!({"KeyId": key_id, "Plaintext": b64(b"re-encrypt me")}),
    )
    .await;
    text("ciphertext", &answer, "CiphertextBlob")
}

/// Rule 1: a constraint violation on a `@sensitive` member withholds the value it
/// refused, and one on any other member keeps it.
///
/// Which members those are is read from the model itself rather than listed in the
/// code, so these rows are the behaviour that reading produces. `Encrypt$Plaintext` and
/// the `Message` of `Sign`, `Verify`, `GenerateMac` and `VerifyMac` all target
/// `PlaintextType`, which the model marks `@sensitive` with `length 1..4096`;
/// `KeyStorePassword` targets the `@sensitive` `KeyStorePasswordType` with
/// `length 7..32` — a string rather than a blob, and a member the cited failure never
/// touched. `Signature` (`CiphertextType`, `length 1..6144`) and `GrantTokens`
/// (`length 0..10`) are not sensitive
/// (`smithy_query(query="operation",operation="Encrypt,Sign,Verify,GenerateMac,VerifyMac")`),
/// so the scalar one keeps the value it was handed; the collection states none, which is
/// the shared renderer's own rule for a list or a map rather than this one.
#[tokio::test]
async fn a_constraint_violation_on_a_sensitive_member_withholds_the_value() {
    let router = router();
    let (symmetric, _) = key_of(&router, "SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT").await;
    let (signing, _) = key_of(&router, "RSA_2048", "SIGN_VERIFY").await;
    let (hmac, _) = key_of(&router, "HMAC_256", "GENERATE_VERIFY_MAC").await;

    // (case, operation, request, the clause the envelope must carry).
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "Encrypt past the Plaintext ceiling",
            "Encrypt",
            json!({"KeyId": symmetric, "Plaintext": b64(&filler(4097))}),
            "Value at 'plaintext' failed to satisfy constraint: Member must have length \
             less than or equal to 4096"
                .to_string(),
        ),
        (
            "Encrypt with an empty Plaintext",
            "Encrypt",
            json!({"KeyId": symmetric, "Plaintext": b64(&filler(0))}),
            "Value at 'plaintext' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "Sign past the Message ceiling",
            "Sign",
            json!({"KeyId": signing, "Message": b64(&filler(4097)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            "Value at 'message' failed to satisfy constraint: Member must have length less \
             than or equal to 4096"
                .to_string(),
        ),
        (
            "Verify with an empty Message",
            "Verify",
            json!({"KeyId": signing, "Message": b64(&filler(0)),
                   "Signature": b64(&filler(256)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            "Value at 'message' failed to satisfy constraint: Member must have length \
             greater than or equal to 1"
                .to_string(),
        ),
        (
            "GenerateMac past the Message ceiling",
            "GenerateMac",
            json!({"KeyId": hmac, "Message": b64(&filler(4097)),
                   "MacAlgorithm": "HMAC_SHA_256"}),
            "Value at 'message' failed to satisfy constraint: Member must have length less \
             than or equal to 4096"
                .to_string(),
        ),
        (
            "VerifyMac past the Message ceiling",
            "VerifyMac",
            json!({"KeyId": hmac, "Message": b64(&filler(4097)),
                   "Mac": b64(&filler(32)), "MacAlgorithm": "HMAC_SHA_256"}),
            "Value at 'message' failed to satisfy constraint: Member must have length less \
             than or equal to 4096"
                .to_string(),
        ),
        (
            "a KeyStorePassword shorter than the minimum",
            "CreateCustomKeyStore",
            json!({"CustomKeyStoreName": "withheld-password",
                   "KeyStorePassword": "short6"}),
            "Value at 'keyStorePassword' failed to satisfy constraint: Member must have \
             length greater than or equal to 7"
                .to_string(),
        ),
        (
            "a KeyStorePassword longer than the maximum",
            "CreateCustomKeyStore",
            json!({"CustomKeyStoreName": "withheld-password",
                   "KeyStorePassword": "p".repeat(33)}),
            "Value at 'keyStorePassword' failed to satisfy constraint: Member must have \
             length less than or equal to 32"
                .to_string(),
        ),
        (
            "a Signature past its ceiling, which is not a sensitive member",
            "Verify",
            json!({"KeyId": signing, "Message": b64(&filler(32)),
                   "Signature": b64(&filler(6145)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            "Value '6145 bytes' at 'signature' failed to satisfy constraint: Member must \
             have length less than or equal to 6144"
                .to_string(),
        ),
        (
            // Not a sensitive member either, and the requirement half states the
            // model's expression as the model writes it — anchors included, which is
            // what this service's recorded refusal of such a name carries.
            "an AliasName outside the modelled pattern",
            "CreateAlias",
            json!({"AliasName": "alias/bad name!", "TargetKeyId": symmetric}),
            "Value 'alias/bad name!' at 'aliasName' failed to satisfy constraint: Member \
             must satisfy regular expression pattern: ^[a-zA-Z0-9:/_-]+$"
                .to_string(),
        ),
        (
            // A LIST, not a sensitive member: a collection has no scalar rendering to
            // quote, so the shared renderer every service on this protocol uses states
            // no value for one either — the rule this row pins is that the withheld
            // value is not keyed to the member being sensitive alone.
            "more GrantTokens than the ceiling, a collection rather than a value",
            "Encrypt",
            json!({"KeyId": symmetric, "Plaintext": b64(b"short"),
                   "GrantTokens": vec!["token"; 11]}),
            "Value at 'grantTokens' failed to satisfy constraint: Member must have length \
             less than or equal to 10"
                .to_string(),
        ),
    ];

    for (case, operation, request, clause) in rows {
        let (status, code, message) = refused(&router, REGION, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case}: expected HTTP 400 ValidationException, got {status} {code}"
        );
        assert_eq!(
            message,
            one_violation(&clause),
            "case {case}: expected {:?}, got {message:?}",
            one_violation(&clause)
        );
    }

    // The at-limit leg: a `Plaintext` of exactly the ceiling is not a violation at all.
    let answer = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": symmetric, "Plaintext": b64(&filler(4096))}),
    )
    .await;
    assert!(
        answer["CiphertextBlob"].is_string(),
        "case a Plaintext of exactly 4096 bytes: expected a ciphertext, got {answer}"
    );
}

/// Rule 2: a key whose `KeyUsage` does not permit the operation is refused
/// `InvalidKeyUsageException` by a sentence naming that key, its usage and the
/// operation — and nothing else.
#[tokio::test]
async fn an_incompatible_key_usage_names_the_key_its_usage_and_the_operation() {
    let router = router();
    let (symmetric, symmetric_arn) = key_of(&router, "SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT").await;
    let (signing, signing_arn) = key_of(&router, "RSA_2048", "SIGN_VERIFY").await;
    let (hmac, hmac_arn) = key_of(&router, "HMAC_256", "GENERATE_VERIFY_MAC").await;
    let (agreement, agreement_arn) = key_of(&router, "ECC_NIST_P256", "KEY_AGREEMENT").await;
    let blob = ciphertext_of(&router, &symmetric).await;

    // (case, operation, request, the key ARN the refusal names, the usage it names).
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "Sign with a symmetric encryption key",
            "Sign",
            json!({"KeyId": symmetric, "Message": b64(&filler(32)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            &symmetric_arn,
            "ENCRYPT_DECRYPT",
        ),
        (
            "Sign with an HMAC key",
            "Sign",
            json!({"KeyId": hmac, "Message": b64(&filler(32)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
        (
            "Sign with a key-agreement key",
            "Sign",
            json!({"KeyId": agreement, "Message": b64(&filler(32)),
                   "SigningAlgorithm": "ECDSA_SHA_256"}),
            &agreement_arn,
            "KEY_AGREEMENT",
        ),
        (
            "Verify with a symmetric encryption key",
            "Verify",
            json!({"KeyId": symmetric, "Message": b64(&filler(32)),
                   "Signature": b64(&filler(256)),
                   "SigningAlgorithm": "RSASSA_PKCS1_V1_5_SHA_256"}),
            &symmetric_arn,
            "ENCRYPT_DECRYPT",
        ),
        (
            "Encrypt with a signing key",
            "Encrypt",
            json!({"KeyId": signing, "Plaintext": b64(b"secret")}),
            &signing_arn,
            "SIGN_VERIFY",
        ),
        (
            "Encrypt with an HMAC key",
            "Encrypt",
            json!({"KeyId": hmac, "Plaintext": b64(b"secret")}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
        (
            "Decrypt with a signing key",
            "Decrypt",
            json!({"KeyId": signing, "CiphertextBlob": b64(&filler(64))}),
            &signing_arn,
            "SIGN_VERIFY",
        ),
        (
            "GenerateDataKey with a signing key",
            "GenerateDataKey",
            json!({"KeyId": signing, "KeySpec": "AES_256"}),
            &signing_arn,
            "SIGN_VERIFY",
        ),
        (
            "GenerateDataKeyWithoutPlaintext with an HMAC key",
            "GenerateDataKeyWithoutPlaintext",
            json!({"KeyId": hmac, "KeySpec": "AES_256"}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
        (
            "GenerateDataKeyPair with a signing key",
            "GenerateDataKeyPair",
            json!({"KeyId": signing, "KeyPairSpec": "ECC_NIST_P256"}),
            &signing_arn,
            "SIGN_VERIFY",
        ),
        (
            "GenerateDataKeyPairWithoutPlaintext with an HMAC key",
            "GenerateDataKeyPairWithoutPlaintext",
            json!({"KeyId": hmac, "KeyPairSpec": "ECC_NIST_P256"}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
        (
            "GetPublicKey with an HMAC key",
            "GetPublicKey",
            json!({"KeyId": hmac}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
        (
            "ReEncrypt into a signing key",
            "ReEncrypt",
            json!({"CiphertextBlob": blob, "DestinationKeyId": signing}),
            &signing_arn,
            "SIGN_VERIFY",
        ),
        (
            "ReEncrypt into an HMAC key",
            "ReEncrypt",
            json!({"CiphertextBlob": blob, "DestinationKeyId": hmac}),
            &hmac_arn,
            "GENERATE_VERIFY_MAC",
        ),
    ];

    for (case, operation, request, key_arn, key_usage) in rows {
        let expected = format!("{key_arn} key usage is {key_usage} which is not valid for {operation}.");
        let (status, code, message) = refused(&router, REGION, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidKeyUsageException"),
            "case {case}: expected HTTP 400 InvalidKeyUsageException, got {status} {code} \
             ({message:?})"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// Rule 3: an algorithm the key's own key spec does not support is refused
/// `InvalidKeyUsageException` by a sentence naming the algorithm and that key spec.
#[tokio::test]
async fn an_incompatible_algorithm_names_the_algorithm_and_the_key_spec() {
    let router = router();
    let (symmetric, _) = key_of(&router, "SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT").await;
    let (p384, _) = key_of(&router, "ECC_NIST_P384", "SIGN_VERIFY").await;
    let (p256, _) = key_of(&router, "ECC_NIST_P256", "SIGN_VERIFY").await;
    let (rsa, _) = key_of(&router, "RSA_2048", "SIGN_VERIFY").await;
    let (hmac, _) = key_of(&router, "HMAC_256", "GENERATE_VERIFY_MAC").await;
    let blob = ciphertext_of(&router, &symmetric).await;

    // (case, operation, request, the algorithm the refusal names, the key spec it
    // names). The rows span the three kinds of algorithm a KMS key spec admits.
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "a shorter ECDSA hash than the curve takes",
            "Sign",
            json!({"KeyId": p384, "Message": b64(&filler(32)), "MessageType": "RAW",
                   "SigningAlgorithm": "ECDSA_SHA_256"}),
            "ECDSA_SHA_256",
            "ECC_NIST_P384",
        ),
        (
            "a longer ECDSA hash than the curve takes",
            "Sign",
            json!({"KeyId": p256, "Message": b64(&filler(32)), "MessageType": "RAW",
                   "SigningAlgorithm": "ECDSA_SHA_384"}),
            "ECDSA_SHA_384",
            "ECC_NIST_P256",
        ),
        (
            "an elliptic-curve algorithm on an RSA key",
            "Sign",
            json!({"KeyId": rsa, "Message": b64(&filler(32)), "MessageType": "RAW",
                   "SigningAlgorithm": "ECDSA_SHA_256"}),
            "ECDSA_SHA_256",
            "RSA_2048",
        ),
        (
            "Verify under an algorithm the curve does not take",
            "Verify",
            json!({"KeyId": p384, "Message": b64(&filler(32)), "MessageType": "RAW",
                   "Signature": b64(&filler(96)), "SigningAlgorithm": "ECDSA_SHA_256"}),
            "ECDSA_SHA_256",
            "ECC_NIST_P384",
        ),
        (
            "an asymmetric encryption algorithm on a symmetric key",
            "Encrypt",
            json!({"KeyId": symmetric, "Plaintext": b64(b"secret"),
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            "RSAES_OAEP_SHA_256",
            "SYMMETRIC_DEFAULT",
        ),
        (
            "Decrypt under an algorithm the sealing key spec does not take",
            "Decrypt",
            json!({"CiphertextBlob": blob, "KeyId": symmetric,
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_1"}),
            "RSAES_OAEP_SHA_1",
            "SYMMETRIC_DEFAULT",
        ),
        (
            "ReEncrypt into a symmetric key under an asymmetric algorithm",
            "ReEncrypt",
            json!({"CiphertextBlob": blob, "DestinationKeyId": symmetric,
                   "DestinationEncryptionAlgorithm": "SM2PKE"}),
            "SM2PKE",
            "SYMMETRIC_DEFAULT",
        ),
        (
            "a MAC algorithm of another HMAC key spec",
            "GenerateMac",
            json!({"KeyId": hmac, "Message": b64(&filler(32)),
                   "MacAlgorithm": "HMAC_SHA_512"}),
            "HMAC_SHA_512",
            "HMAC_256",
        ),
    ];

    for (case, operation, request, algorithm, key_spec) in rows {
        let expected = format!("Algorithm {algorithm} is incompatible with key spec {key_spec}.");
        let (status, code, message) = refused(&router, REGION, operation, &request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidKeyUsageException"),
            "case {case}: expected HTTP 400 InvalidKeyUsageException, got {status} {code} \
             ({message:?})"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }

    // The positive control: the algorithm the key spec DOES support signs.
    let answer = ok(
        &router,
        "Sign",
        &json!({"KeyId": p384, "Message": b64(&filler(32)), "MessageType": "RAW",
                "SigningAlgorithm": "ECDSA_SHA_384"}),
    )
    .await;
    assert!(
        answer["Signature"].is_string(),
        "case the algorithm the key spec supports: expected a signature, got {answer}"
    );
}

/// `days` from now, as an awsJson1_1 timestamp (epoch seconds) — how the wire carries a
/// `ValidTo`.
fn in_days(days: f64) -> f64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    (now + days * 86_400.0).trunc()
}

/// Rule 4: `ImportKeyMaterial`'s `ValidTo` carries two rules — the value "must be a
/// future date and time" and "the maximum value is 365 days from the request date"
/// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`) — and
/// each refusal states only the rule the request broke, naming no timestamp.
#[tokio::test]
async fn the_valid_to_refusals_each_state_one_rule() {
    let router = router();
    let metadata = create_key(&router, &json!({"Origin": "EXTERNAL"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let parameters = ok(
        &router,
        "GetParametersForImport",
        &json!({"KeyId": key_id, "WrappingAlgorithm": "RSAES_OAEP_SHA_256",
                "WrappingKeySpec": "RSA_2048"}),
    )
    .await;
    let public_key = unb64("setup", &parameters, "PublicKey");
    let import_token = text("setup", &parameters, "ImportToken");
    let material = filler(32);
    let wrapped = wrap("setup", &public_key, "RSAES_OAEP_SHA_256", &material);

    let not_future = "ValidTo must be in the future";
    let past_window = "ValidTo must be at most 365 days from the request date";

    // (case, the ValidTo the row sends, the one rule its refusal states).
    let rows: Vec<(&str, f64, &str)> = vec![
        ("a day in the past", in_days(-1.0), not_future),
        ("a second in the past", in_days(-1.0 / 86_400.0), not_future),
        ("the epoch itself", 0.0, not_future),
        ("a timestamp before the epoch", -9.0e18, not_future),
        ("a day past the window", in_days(366.0), past_window),
        ("ten years ahead", in_days(3_650.0), past_window),
        ("a timestamp no clock can represent", 9.0e18, past_window),
    ];

    for (case, valid_to, expected) in rows {
        let (status, code, message) = refused(
            &router,
            REGION,
            "ImportKeyMaterial",
            &json!({"KeyId": key_id, "ImportToken": import_token,
                    "EncryptedKeyMaterial": b64(&wrapped),
                    "ExpirationModel": "KEY_MATERIAL_EXPIRES", "ValidTo": valid_to}),
        )
        .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case}: expected HTTP 400 ValidationException, got {status} {code} \
             ({message:?})"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
        assert_eq!(
            text(case, &describe(&router, &key_id).await, "KeyState"),
            "PendingImport",
            "case {case}: a refused import leaves the key waiting"
        );
    }

    // The same (token, wrapped material) pair, with a ValidTo inside BOTH rules: the
    // rows above were refused for their timestamp alone.
    ok(
        &router,
        "ImportKeyMaterial",
        &json!({"KeyId": key_id, "ImportToken": import_token,
                "EncryptedKeyMaterial": b64(&wrapped),
                "ExpirationModel": "KEY_MATERIAL_EXPIRES", "ValidTo": in_days(30.0)}),
    )
    .await;
    assert_eq!(
        text(
            "an expiry inside both rules",
            &describe(&router, &key_id).await,
            "KeyState"
        ),
        "Enabled",
        "case an expiry inside both rules: the import makes the key usable"
    );
}

/// Rule 5: an operation whose `KeyId` takes a key id or key ARN only refuses an
/// identifier that names an ALIAS for its KIND — `InvalidArnException` with the fixed
/// sentence — and does so BEFORE it resolves anything, so an alias naming a key the
/// operation could have served and one naming nothing are refused identically.
#[tokio::test]
async fn an_alias_identifier_is_refused_for_its_kind_before_anything_is_resolved() {
    let router = router();
    let account = engine::emulator_account_id();
    let expected = "Key Aliases are not supported for this operation.";

    // A multi-Region primary key with a replica, so the set is one `ReplicateKey` and
    // `UpdatePrimaryRegion` would otherwise serve through the alias below.
    let primary = text(
        "setup",
        &create_key(&router, &json!({"MultiRegion": true})).await,
        "KeyId",
    );
    ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": primary, "ReplicaRegion": "eu-west-2"}),
    )
    .await;
    create_alias(&router, "setup", "alias/replicable", &primary).await;

    // A key awaiting imported key material, which the import operations would serve.
    let importable = text(
        "setup",
        &create_key(&router, &json!({"Origin": "EXTERNAL"})).await,
        "KeyId",
    );
    create_alias(&router, "setup", "alias/importable", &importable).await;

    // The alias forms a `KeyId` member accepts for an alias — a bare name and an alias
    // ARN — each both for an alias this endpoint HOLDS and for one it does not.
    let aliases: Vec<(&str, String)> = vec![
        ("a bare alias name that resolves", "alias/replicable".to_string()),
        (
            "an alias ARN that resolves",
            format!("arn:aws:kms:{REGION}:{account}:alias/replicable"),
        ),
        ("a bare alias name that resolves to nothing", "alias/absent".to_string()),
        (
            "an alias ARN that resolves to nothing",
            format!("arn:aws:kms:{REGION}:{account}:alias/absent"),
        ),
    ];

    // (operation, the rest of its request).
    let operations: Vec<(&str, Value)> = vec![
        ("ReplicateKey", json!({"ReplicaRegion": "ap-south-1"})),
        ("UpdatePrimaryRegion", json!({"PrimaryRegion": "eu-west-2"})),
        ("GetKeyLastUsage", json!({})),
        (
            "GetParametersForImport",
            json!({"WrappingAlgorithm": "RSAES_OAEP_SHA_256", "WrappingKeySpec": "RSA_2048"}),
        ),
        (
            "ImportKeyMaterial",
            json!({"ImportToken": b64(&filler(32)),
                   "EncryptedKeyMaterial": b64(&filler(256)),
                   "ExpirationModel": "KEY_MATERIAL_DOES_NOT_EXPIRE"}),
        ),
        ("DeleteImportedKeyMaterial", json!({})),
    ];

    for (operation, rest) in operations {
        for (form, identifier) in &aliases {
            let mut request = rest.clone();
            request["KeyId"] = json!(identifier);
            let case = format!("{operation}: {form}");
            let (status, code, message) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidArnException"),
                "case {case}: expected HTTP 400 InvalidArnException, got {status} {code} \
                 ({message:?})"
            );
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // The positive control: the same operation, the same key, named by a KEY ARN —
    // which reaches the business rule and is served.
    let key_arn = text("control", &describe(&router, &primary).await, "Arn");
    let answer = ok(
        &router,
        "ReplicateKey",
        &json!({"KeyId": key_arn, "ReplicaRegion": "ap-south-1"}),
    )
    .await;
    assert_eq!(
        answer["ReplicaKeyMetadata"]["KeyId"],
        json!(primary),
        "case a key ARN of the primary key: expected the replica of that key, got {answer}"
    );
}

/// Rule 5, continued on `CreateAlias`'s own `AliasName`: each of the three rules the
/// member's documentation states carries its OWN error code, and the name is judged
/// BEFORE the target key is resolved — so a request whose target names no key at all is
/// still refused for its name.
#[tokio::test]
async fn an_alias_name_rule_is_judged_before_the_target_key() {
    let router = router();
    let absent = "00000000-0000-0000-0000-000000000000";

    // (case, the AliasName, the error code, the sentence).
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "a name with no alias/ prefix",
            "behavioural-noprefix",
            "ValidationException",
            "Alias must start with the prefix \"alias/\". Please see \
             https://docs.aws.amazon.com/kms/latest/developerguide/kms-alias.html",
        ),
        (
            "a name under the reserved namespace",
            "alias/aws/dynamodb",
            "NotAuthorizedException",
            "The alias name for a customer managed CMK cannot begin with alias/aws/.",
        ),
        (
            "the prefix with no name after it",
            "alias/",
            "InvalidAliasNameException",
            "Alias must have a name after the prefix \"alias/\".",
        ),
    ];

    for (case, alias_name, expected_code, expected_message) in rows {
        let (status, payload) = call(
            &router,
            REGION,
            "CreateAlias",
            &json!({"AliasName": alias_name, "TargetKeyId": absent}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            expected_code,
            "case {case}: expected {expected_code}, got {payload}"
        );
        assert_eq!(
            error_message(&payload),
            expected_message,
            "case {case}: expected {expected_message:?}, got {payload}"
        );
    }
}
