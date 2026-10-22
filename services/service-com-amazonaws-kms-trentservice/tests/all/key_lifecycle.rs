// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the KMS key lifecycle and the symmetric encryption
//! round-trip, driven over the wire (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: TrentService.<Operation>`) through the same multi-service router
//! the emulator boots.
//!
//! Every assertion is about a RULE, exercised over a table of inputs rather than a
//! single call: what `DescribeKey` reports after a create, that `Decrypt` returns
//! the bytes `Encrypt` was given for any plaintext and any encryption context, which
//! page a `Marker` resumes on, and which key state refuses a cryptographic
//! operation.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_kms_trentservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region most cases sign for. Keys are regional, so a case that is about
/// regional scoping names its own.
const REGION: &str = "us-east-1";

/// A router with only this service mounted, over one shared engine — the same
/// assembly `core/router` performs, minus the other services.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a
    // front-door setting, not part of what these tests are about.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Call one operation in `region` and return its HTTP status and JSON body.
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kms.{region}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("TrentService.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEMULATOR/20260101/{region}/kms/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("the request builds");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, payload)
}

/// Call one operation in [`REGION`], requiring success.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    ok_in(router, REGION, operation, body).await
}

/// Call one operation in `region`, requiring success.
async fn ok_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, payload) = call(router, region, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {payload}"
    );
    payload
}

/// Call one operation in [`REGION`], requiring a failure, and report its status and
/// wire error code.
async fn failure(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, String) {
    failure_in(router, REGION, operation, body).await
}

/// [`failure`], in `region`.
async fn failure_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    let (status, payload) = call(router, region, operation, body).await;
    assert_ne!(
        status, 200,
        "case {operation} {body}: expected a failure, got HTTP 200 with {payload}"
    );
    (status, error_code(&payload))
}

/// The wire error code (`__type`) of an error body, without its namespace.
fn error_code(payload: &Value) -> String {
    payload["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A blob as awsJson1_1 carries it.
fn b64(bytes: &[u8]) -> String {
    aws_smithy_types::base64::encode(bytes)
}

/// The bytes behind a blob member.
fn unb64(case: &str, payload: &Value, member: &str) -> Vec<u8> {
    let encoded = payload[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected {member} in {payload}"));
    aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case}: {member} is not base64: {error}"))
}

/// Create a key and return its `KeyMetadata`.
async fn create_key(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "CreateKey", request).await["KeyMetadata"].clone()
}

/// The `KeyMetadata` `DescribeKey` reports for an identifier.
async fn describe(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "DescribeKey", &json!({ "KeyId": identifier })).await["KeyMetadata"].clone()
}

/// A member of a metadata object, as a string.
fn text(case: &str, value: &Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a string {member} in {value}"))
        .to_string()
}

/// Whether `id` has the shape of a KMS key id: a version-4 UUID (8-4-4-4-12
/// lowercase hex with the version nibble `4`) for a single-Region key, and `mrk-`
/// followed by 32 lowercase hex characters for a multi-Region one.
fn is_key_id(id: &str, multi_region: bool) -> bool {
    if multi_region {
        return id
            .strip_prefix("mrk-")
            .is_some_and(|hex| hex.len() == 32 && is_lower_hex(hex));
    }
    let groups: Vec<&str> = id.split('-').collect();
    let lengths = [8usize, 4, 4, 4, 12];
    groups.len() == lengths.len()
        && groups
            .iter()
            .zip(lengths)
            .all(|(group, len)| group.len() == len && is_lower_hex(group))
        && groups[2].starts_with('4')
}

/// Whether every character is a lowercase hexadecimal digit.
fn is_lower_hex(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Whether `haystack` contains `needle` as a contiguous run — how a test checks
/// that a ciphertext does not simply carry its plaintext.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack.len() >= needle.len()
        && haystack.windows(needle.len()).any(|window| window == needle)
}

/// The key the encryption round-trip cases use.
async fn enabled_key(router: &MultiServiceRouter) -> Value {
    create_key(router, &json!({"Description": "round trip"})).await
}

// ---------------------------------------------------------------------------
// CreateKey -> DescribeKey
// ---------------------------------------------------------------------------

/// A created key is persisted, and `DescribeKey` reports exactly the metadata
/// `CreateKey` returned — for every identifier form that names it.
///
/// The rule is the same for each row of the table; the rows differ in what the
/// request says: nothing at all, a description (empty, long, non-ASCII), the
/// deprecated spec member, tags, a policy, and the multi-Region flag.
#[tokio::test]
async fn create_key_persists_the_metadata_describe_key_reports() {
    let router = router();
    let account = engine::emulator_account_id();
    let long_description = "d".repeat(8192);
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Principal": {"AWS": "*"}, "Action": "kms:*", "Resource": "*"}]
    })
    .to_string();

    let cases: Vec<(&str, Value, String, bool)> = vec![
        ("defaults", json!({}), String::new(), false),
        (
            "described",
            json!({"Description": "bootstrap key"}),
            "bootstrap key".to_string(),
            false,
        ),
        (
            "explicit spec and usage",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "KeyUsage": "ENCRYPT_DECRYPT"}),
            String::new(),
            false,
        ),
        (
            "deprecated spec member",
            json!({"CustomerMasterKeySpec": "SYMMETRIC_DEFAULT"}),
            String::new(),
            false,
        ),
        (
            "multi-Region primary",
            json!({"Description": "primary", "MultiRegion": true}),
            "primary".to_string(),
            true,
        ),
        (
            "tagged",
            json!({"Tags": [{"TagKey": "app", "TagValue": "demo"},
                            {"TagKey": "env", "TagValue": "test"}]}),
            String::new(),
            false,
        ),
        (
            "explicit policy",
            json!({"Policy": policy}),
            String::new(),
            false,
        ),
        (
            "description at the modelled maximum",
            json!({"Description": long_description}),
            long_description.clone(),
            false,
        ),
        (
            "non-ASCII description",
            json!({"Description": "clé ✓ 鍵"}),
            "clé ✓ 鍵".to_string(),
            false,
        ),
        (
            "explicit single-Region",
            json!({"MultiRegion": false}),
            String::new(),
            false,
        ),
    ];

    let mut seen_key_ids: Vec<String> = Vec::new();
    let mut seen_material_ids: Vec<String> = Vec::new();

    for (case, request, expected_description, expected_multi_region) in cases {
        let metadata = create_key(&router, &request).await;

        let key_id = text(case, &metadata, "KeyId");
        assert!(
            is_key_id(&key_id, expected_multi_region),
            "case {case}: expected a {} key id, got {key_id:?}",
            if expected_multi_region {
                "multi-Region (mrk-<32 hex>)"
            } else {
                "version-4 UUID"
            }
        );
        let arn = text(case, &metadata, "Arn");
        assert_eq!(
            arn,
            format!("arn:aws:kms:{REGION}:{account}:key/{key_id}"),
            "case {case}: the Arn must name the minted key id in the signed Region"
        );
        assert_eq!(
            text(case, &metadata, "AWSAccountId"),
            account,
            "case {case}: expected the emulator account id, got {metadata}"
        );
        assert_eq!(
            metadata["Enabled"],
            json!(true),
            "case {case}: a new key is enabled, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeyState"),
            "Enabled",
            "case {case}: a new key with KMS-generated material is Enabled, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "Description"),
            expected_description,
            "case {case}: the description must be the one the request carried"
        );
        assert_eq!(
            text(case, &metadata, "KeyUsage"),
            "ENCRYPT_DECRYPT",
            "case {case}: expected the default KeyUsage, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeySpec"),
            "SYMMETRIC_DEFAULT",
            "case {case}: expected the default KeySpec, got {metadata}"
        );
        assert_eq!(
            metadata["CustomerMasterKeySpec"], metadata["KeySpec"],
            "case {case}: the deprecated spec member reports the same value, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "Origin"),
            "AWS_KMS",
            "case {case}: expected the default Origin, got {metadata}"
        );
        assert_eq!(
            text(case, &metadata, "KeyManager"),
            "CUSTOMER",
            "case {case}: a key the caller created is customer managed, got {metadata}"
        );
        assert_eq!(
            metadata["EncryptionAlgorithms"],
            json!(["SYMMETRIC_DEFAULT"]),
            "case {case}: expected the symmetric algorithm list, got {metadata}"
        );
        assert_eq!(
            metadata["MultiRegion"],
            json!(expected_multi_region),
            "case {case}: expected MultiRegion {expected_multi_region}, got {metadata}"
        );
        if expected_multi_region {
            assert_eq!(
                metadata["MultiRegionConfiguration"]["MultiRegionKeyType"],
                json!("PRIMARY"),
                "case {case}: a multi-Region key created here is the primary, got {metadata}"
            );
            assert_eq!(
                metadata["MultiRegionConfiguration"]["PrimaryKey"]["Arn"], metadata["Arn"],
                "case {case}: the primary key is this key, got {metadata}"
            );
        } else {
            assert!(
                metadata["MultiRegionConfiguration"].is_null(),
                "case {case}: a single-Region key has no multi-Region configuration, got {metadata}"
            );
        }
        let material_id = text(case, &metadata, "CurrentKeyMaterialId");
        assert!(
            material_id.len() == 64 && material_id.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "case {case}: expected 64 lowercase hex characters of KeyMaterialId, got {material_id:?}"
        );
        let creation_date = metadata["CreationDate"].as_f64().unwrap_or_default();
        assert!(
            creation_date > 0.0,
            "case {case}: expected a creation date, got {metadata}"
        );

        // Every identifier form that names this key reports the identical record.
        for identifier in [key_id.clone(), arn.clone()] {
            let described = describe(&router, &identifier).await;
            assert_eq!(
                described, metadata,
                "case {case}: DescribeKey({identifier}) must report what CreateKey returned"
            );
        }

        assert!(
            !seen_key_ids.contains(&key_id),
            "case {case}: every create mints a fresh key id, but {key_id} repeated"
        );
        assert!(
            !seen_material_ids.contains(&material_id),
            "case {case}: every key gets its own key material, but {material_id} repeated"
        );
        seen_key_ids.push(key_id);
        seen_material_ids.push(material_id);
    }
}

/// `CreateKey` refuses a key it cannot mint, and mints nothing when it does: every
/// refusal names a modelled error, and `ListKeys` still reports only the keys that
/// succeeded.
#[tokio::test]
async fn create_key_refuses_what_it_cannot_mint() {
    let router = router();

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "asymmetric key spec",
            json!({"KeySpec": "RSA_2048"}),
            "UnsupportedOperationException",
        ),
        (
            "HMAC key spec",
            json!({"KeySpec": "HMAC_256", "KeyUsage": "GENERATE_VERIFY_MAC"}),
            "UnsupportedOperationException",
        ),
        (
            "signing key usage",
            json!({"KeyUsage": "SIGN_VERIFY"}),
            "UnsupportedOperationException",
        ),
        (
            "key agreement key usage",
            json!({"KeyUsage": "KEY_AGREEMENT"}),
            "UnsupportedOperationException",
        ),
        (
            "imported key material",
            json!({"Origin": "EXTERNAL"}),
            "UnsupportedOperationException",
        ),
        (
            "external key store id",
            json!({"CustomKeyStoreId": "cks-1234567890abcdef0"}),
            "CustomKeyStoreNotFoundException",
        ),
        (
            "external key id",
            json!({"XksKeyId": "external-key-1"}),
            "UnsupportedOperationException",
        ),
        (
            "spec members disagree",
            json!({"KeySpec": "SYMMETRIC_DEFAULT", "CustomerMasterKeySpec": "HMAC_256"}),
            "ValidationException",
        ),
        (
            "policy is not JSON",
            json!({"Policy": "not a policy"}),
            "MalformedPolicyDocumentException",
        ),
        (
            "policy is not an object",
            json!({"Policy": "[]"}),
            "MalformedPolicyDocumentException",
        ),
        (
            "policy has no statement",
            json!({"Policy": "{\"Version\":\"2012-10-17\"}"}),
            "MalformedPolicyDocumentException",
        ),
        (
            "policy statement is empty",
            json!({"Policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}"}),
            "MalformedPolicyDocumentException",
        ),
        (
            "duplicate tag key",
            json!({"Tags": [{"TagKey": "app", "TagValue": "one"},
                            {"TagKey": "app", "TagValue": "two"}]}),
            "TagException",
        ),
    ];

    // One key that WILL be created, so the "nothing was minted" assertion has
    // something to compare against.
    let kept = create_key(&router, &json!({"Description": "kept"})).await;

    for (case, request, expected_code) in cases {
        let (status, code) = failure(&router, "CreateKey", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400 for a refused create, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }

    let listed = ok(&router, "ListKeys", &json!({})).await;
    assert_eq!(
        listed["Keys"],
        json!([{"KeyId": kept["KeyId"], "KeyArn": kept["Arn"]}]),
        "a refused create must leave no key behind, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// Encrypt -> Decrypt
// ---------------------------------------------------------------------------

/// `Decrypt` returns the exact bytes `Encrypt` was given, for any plaintext within
/// the modelled bounds and any encryption context, and the ciphertext is opaque: it
/// never carries the plaintext.
///
/// The rows differ in the plaintext (size from one byte to the modelled 4096-byte
/// maximum, ASCII, arbitrary binary, non-ASCII text) and in the encryption context
/// (absent, empty, one entry, several).
#[tokio::test]
async fn encrypt_and_decrypt_round_trip_every_plaintext_and_context() {
    let router = router();
    let metadata = enabled_key(&router).await;
    let arn = metadata["Arn"].as_str().expect("the key has an ARN").to_string();

    let binary: Vec<u8> = (0..=255u8).cycle().take(1024).collect();
    let cases: Vec<(&str, Vec<u8>, Value)> = vec![
        ("one byte, no context", vec![0x00], Value::Null),
        ("ascii, no context", b"hello kms".to_vec(), Value::Null),
        (
            "ascii, empty context",
            b"hello kms".to_vec(),
            json!({}),
        ),
        (
            "ascii, one context entry",
            b"hello kms".to_vec(),
            json!({"app": "demo"}),
        ),
        (
            "ascii, three context entries",
            b"hello kms".to_vec(),
            json!({"app": "demo", "env": "test", "tenant": "acme"}),
        ),
        (
            "arbitrary binary",
            binary.clone(),
            json!({"app": "demo"}),
        ),
        (
            "plaintext at the modelled maximum",
            vec![0x5au8; 4096],
            json!({"size": "4096"}),
        ),
        (
            "non-ASCII text",
            "clé ✓ 鍵".as_bytes().to_vec(),
            json!({"клуч": "значение"}),
        ),
    ];

    for (case, plaintext, context) in cases {
        let mut request = json!({"KeyId": arn, "Plaintext": b64(&plaintext)});
        if !context.is_null() {
            request["EncryptionContext"] = context.clone();
        }
        let encrypted = ok(&router, "Encrypt", &request).await;

        assert_eq!(
            encrypted["KeyId"].as_deref_or_empty(),
            arn,
            "case {case}: Encrypt answers with the key ARN, got {encrypted}"
        );
        assert_eq!(
            encrypted["EncryptionAlgorithm"], json!("SYMMETRIC_DEFAULT"),
            "case {case}: expected the symmetric algorithm, got {encrypted}"
        );
        let blob = unb64(case, &encrypted, "CiphertextBlob");
        assert_ne!(
            blob, plaintext,
            "case {case}: the ciphertext must not be the plaintext"
        );
        // A plaintext of a few bytes can occur inside a ciphertext by chance (a
        // single byte does so about once in 256 positions), so the containment
        // check applies to the rows where a coincidence is negligible — eight
        // bytes put it below 2^-64 per position.
        if plaintext.len() >= 8 {
            assert!(
                !contains(&blob, &plaintext),
                "case {case}: the ciphertext must not carry the plaintext, got {} bytes",
                blob.len()
            );
        }
        assert!(
            blob.len() <= 6144,
            "case {case}: the ciphertext must fit CiphertextType's 6144-byte bound, got {}",
            blob.len()
        );

        // A second seal of the same input is a different ciphertext, and both open.
        let again = ok(&router, "Encrypt", &request).await;
        let blob_again = unb64(case, &again, "CiphertextBlob");
        assert_ne!(
            blob, blob_again,
            "case {case}: two encryptions of one plaintext must not produce one ciphertext"
        );

        for (which, ciphertext) in [("first", &blob), ("second", &blob_again)] {
            let mut decrypt = json!({"CiphertextBlob": b64(ciphertext)});
            if !context.is_null() {
                decrypt["EncryptionContext"] = context.clone();
            }
            let decrypted = ok(&router, "Decrypt", &decrypt).await;
            assert_eq!(
                unb64(case, &decrypted, "Plaintext"),
                plaintext,
                "case {case} ({which} ciphertext): Decrypt must return the original plaintext"
            );
            assert_eq!(
                decrypted["KeyId"], encrypted["KeyId"],
                "case {case} ({which} ciphertext): Decrypt names the key that sealed it, got {decrypted}"
            );
            assert_eq!(
                decrypted["EncryptionAlgorithm"], json!("SYMMETRIC_DEFAULT"),
                "case {case} ({which} ciphertext): expected the symmetric algorithm, got {decrypted}"
            );
            assert_eq!(
                decrypted["KeyMaterialId"], metadata["CurrentKeyMaterialId"],
                "case {case} ({which} ciphertext): the key material is the key's current one, got {decrypted}"
            );
        }

        // Naming the key that sealed it is accepted, in either identifier form.
        for identifier in [
            metadata["KeyId"].as_str().unwrap_or_default().to_string(),
            arn.clone(),
        ] {
            let mut decrypt = json!({"CiphertextBlob": b64(&blob), "KeyId": identifier});
            if !context.is_null() {
                decrypt["EncryptionContext"] = context.clone();
            }
            let decrypted = ok(&router, "Decrypt", &decrypt).await;
            assert_eq!(
                unb64(case, &decrypted, "Plaintext"),
                plaintext,
                "case {case}: Decrypt with the sealing key named must still round-trip"
            );
        }
    }
}

/// `Decrypt` refuses a ciphertext it cannot authenticate: one that is not an
/// envelope, one that has been altered, or one presented under an encryption context
/// other than the one it was sealed with.
#[tokio::test]
async fn decrypt_refuses_a_ciphertext_it_cannot_authenticate() {
    let router = router();
    let metadata = enabled_key(&router).await;
    let arn = metadata["Arn"].as_str().expect("the key has an ARN").to_string();
    let plaintext = b"hello kms".to_vec();
    let context = json!({"app": "demo"});

    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": arn, "Plaintext": b64(&plaintext), "EncryptionContext": context}),
    )
    .await;
    let blob = unb64("sealed", &sealed, "CiphertextBlob");

    // Rows differ in HOW the ciphertext fails to authenticate.
    let mut truncated = blob.clone();
    truncated.truncate(blob.len() / 2);
    let mut flipped_tag = blob.clone();
    let last = flipped_tag.len() - 1;
    flipped_tag[last] ^= 0x01;
    let mut appended = blob.clone();
    appended.push(0x00);
    let dropped_version = blob[1..].to_vec();

    let blob_cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated envelope", truncated),
        ("flipped authentication tag", flipped_tag),
        ("extra trailing byte", appended),
        ("version byte dropped", dropped_version),
        ("the plaintext as a ciphertext", plaintext.clone()),
        ("zero bytes", vec![0u8; 64]),
    ];
    for (case, ciphertext) in blob_cases {
        let (status, code) = failure(
            &router,
            "Decrypt",
            &json!({"CiphertextBlob": b64(&ciphertext), "EncryptionContext": context}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidCiphertextException",
            "case {case}: expected InvalidCiphertextException, got {code}"
        );
    }

    // Rows differ in HOW the encryption context differs from the sealed one.
    let context_cases: Vec<(&str, Value)> = vec![
        ("absent context", Value::Null),
        ("empty context", json!({})),
        ("different value", json!({"app": "other"})),
        ("different key", json!({"application": "demo"})),
        ("extra entry", json!({"app": "demo", "env": "test"})),
    ];
    for (case, wrong_context) in context_cases {
        let mut request = json!({"CiphertextBlob": b64(&blob)});
        if !wrong_context.is_null() {
            request["EncryptionContext"] = wrong_context;
        }
        let (status, code) = failure(&router, "Decrypt", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidCiphertextException",
            "case {case}: an encryption context that differs from the sealed one must be refused, got {code}"
        );
    }

    // A `KeyId` that names a key other than the one which sealed the ciphertext.
    let other = create_key(&router, &json!({"Description": "other"})).await;
    for identifier in [
        other["KeyId"].as_str().unwrap_or_default().to_string(),
        other["Arn"].as_str().unwrap_or_default().to_string(),
    ] {
        let (status, code) = failure(
            &router,
            "Decrypt",
            &json!({
                "CiphertextBlob": b64(&blob),
                "EncryptionContext": context,
                "KeyId": identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case key {identifier} did not seal it: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "IncorrectKeyException",
            "case key {identifier} did not seal it: expected IncorrectKeyException, got {code}"
        );
    }

    // A `KeyId` that names no key at all is the refusal every other operation
    // answers for one, not "the wrong key".
    let account = engine::emulator_account_id();
    let absent: Vec<(&str, String)> = vec![
        (
            "unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("unknown alias", "alias/no-such-alias".to_string()),
        (
            "malformed ARN",
            format!("arn:aws:kms:{REGION}:{account}:thing/abc"),
        ),
    ];
    for (case, identifier) in absent {
        let (status, code) = failure(
            &router,
            "Decrypt",
            &json!({
                "CiphertextBlob": b64(&blob),
                "EncryptionContext": context,
                "KeyId": identifier,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case}: expected NotFoundException, got {code}"
        );
    }

    // And a ciphertext this service never issued at all.
    let (status, code) = failure(&router, "Decrypt", &json!({})).await;
    assert_eq!(
        status, 400,
        "case no ciphertext: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "ValidationException",
        "case no ciphertext: expected ValidationException, got {code}"
    );
}

// ---------------------------------------------------------------------------
// ListKeys
// ---------------------------------------------------------------------------

/// `ListKeys` reports one entry per persisted key in creation order, truncates at
/// `Limit`, and resumes exactly after the last entry of the previous page.
///
/// The rows are the page sizes: one key at a time, a size that divides the total, a
/// size that does not, the total itself, and more than the total.
#[tokio::test]
async fn list_keys_pages_over_every_persisted_key_in_creation_order() {
    let router = router();

    let mut expected: Vec<Value> = Vec::new();
    for index in 0..5 {
        let metadata = create_key(&router, &json!({"Description": format!("key-{index}")})).await;
        expected.push(json!({"KeyId": metadata["KeyId"], "KeyArn": metadata["Arn"]}));
    }

    // With no Limit the whole list comes back in one untruncated page.
    let all = ok(&router, "ListKeys", &json!({})).await;
    assert_eq!(
        all["Keys"],
        json!(expected),
        "case no limit: expected every key in creation order, got {all}"
    );
    assert_eq!(
        all["Truncated"],
        json!(false),
        "case no limit: a complete page is not truncated, got {all}"
    );
    assert!(
        all["NextMarker"].is_null(),
        "case no limit: an untruncated page carries no marker, got {all}"
    );

    for limit in [1usize, 2, 3, 5, 7] {
        let case = format!("limit {limit}");
        let mut collected: Vec<Value> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({"Limit": limit});
            if let Some(marker) = marker.as_ref() {
                request["Marker"] = json!(marker);
            }
            let page = ok(&router, "ListKeys", &request).await;
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: pagination must terminate, but took {pages} pages"
            );

            let entries = page["Keys"].as_array().cloned().unwrap_or_default();
            assert!(
                !entries.is_empty() && entries.len() <= limit,
                "case {case}: a page holds between one and Limit entries, got {page}"
            );
            collected.extend(entries);

            let truncated = page["Truncated"] == json!(true);
            assert_eq!(
                truncated,
                collected.len() < expected.len(),
                "case {case}: Truncated must say whether keys remain, got {page}"
            );
            if !truncated {
                assert!(
                    page["NextMarker"].is_null(),
                    "case {case}: the last page carries no marker, got {page}"
                );
                break;
            }
            let next = page["NextMarker"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: a truncated page carries a marker: {page}"))
                .to_string();
            assert!(
                !next.is_empty() && next.len() <= 1024,
                "case {case}: the marker must satisfy MarkerType's 1..1024 bound, got {next:?}"
            );
            marker = Some(next);
        }

        assert_eq!(
            collected,
            expected,
            "case {case}: paging must visit every key exactly once, in creation order"
        );
    }

    // A marker that is not one this service issued is refused.
    let unknown_key = "keys/1/00000000-0000-0000-0000-000000000000";
    let marker_cases: Vec<(&str, String)> = vec![
        ("not hex", "not-a-marker".to_string()),
        ("odd-length hex", "abc".to_string()),
        ("hex of foreign text", hex_of("marker from elsewhere")),
        ("hex of an unknown key id", hex_of(unknown_key)),
        ("hex of another list's token", hex_of("aliases/1/alias/demo")),
    ];
    for (case, marker) in marker_cases {
        let (status, code) = failure(&router, "ListKeys", &json!({"Marker": marker})).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "InvalidMarkerException",
            "case {case}: expected InvalidMarkerException, got {code}"
        );
    }
}

/// The hex encoding of `text`, which is the form a `ListKeys` marker takes — through
/// the same `hex` crate the implementation's `encode_marker` uses.
fn hex_of(text: &str) -> String {
    hex::encode(text)
}

// ---------------------------------------------------------------------------
// Key state
// ---------------------------------------------------------------------------

/// A key's state governs whether it can be used in a cryptographic operation:
/// `Enabled` works, `Disabled` answers `DisabledException`, and `PendingDeletion`
/// answers `KMSInvalidStateException` — and `DescribeKey` reports the state the
/// transition left behind.
#[tokio::test]
async fn key_state_governs_cryptographic_operations() {
    let router = router();
    let metadata = enabled_key(&router).await;
    let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();
    let arn = metadata["Arn"].as_str().expect("an ARN").to_string();
    let plaintext = b"hello kms".to_vec();

    let encrypt = json!({"KeyId": key_id, "Plaintext": b64(&plaintext)});
    let sealed = ok(&router, "Encrypt", &encrypt).await;
    let blob = b64(&unb64("enabled", &sealed, "CiphertextBlob"));

    // Disabled: both cryptographic operations are refused, with the key state the
    // store now holds.
    ok(&router, "DisableKey", &json!({"KeyId": key_id})).await;
    let disabled = describe(&router, &key_id).await;
    assert_eq!(
        disabled["KeyState"],
        json!("Disabled"),
        "case disabled: DescribeKey must report the new state, got {disabled}"
    );
    assert_eq!(
        disabled["Enabled"],
        json!(false),
        "case disabled: a disabled key is not enabled, got {disabled}"
    );
    for (case, operation, body) in [
        ("disabled encrypt", "Encrypt", encrypt.clone()),
        (
            "disabled decrypt",
            "Decrypt",
            json!({"CiphertextBlob": blob}),
        ),
    ] {
        let (status, code) = failure(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "DisabledException",
            "case {case}: expected DisabledException, got {code}"
        );
    }

    // Enabled again: the same calls succeed, and the ciphertext sealed before the
    // key was disabled still opens.
    ok(&router, "EnableKey", &json!({"KeyId": key_id})).await;
    let reenabled = describe(&router, &key_id).await;
    assert_eq!(
        reenabled["KeyState"],
        json!("Enabled"),
        "case re-enabled: expected Enabled, got {reenabled}"
    );
    let decrypted = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
    assert_eq!(
        unb64("re-enabled", &decrypted, "Plaintext"),
        plaintext,
        "case re-enabled: the ciphertext sealed earlier must still open"
    );
    assert_eq!(
        decrypted["KeyId"].as_str().unwrap_or_default(),
        arn,
        "case re-enabled: Decrypt names the sealing key's ARN, got {decrypted}"
    );

    // Pending deletion: the terminal state of this slice.
    let scheduled = ok(
        &router,
        "ScheduleKeyDeletion",
        &json!({"KeyId": key_id, "PendingWindowInDays": 7}),
    )
    .await;
    assert_eq!(
        scheduled["KeyState"],
        json!("PendingDeletion"),
        "case scheduled: expected PendingDeletion, got {scheduled}"
    );
    for (case, operation, body) in [
        ("pending-deletion encrypt", "Encrypt", encrypt.clone()),
        (
            "pending-deletion decrypt",
            "Decrypt",
            json!({"CiphertextBlob": blob}),
        ),
        (
            "pending-deletion disable",
            "DisableKey",
            json!({"KeyId": key_id}),
        ),
        (
            "pending-deletion enable",
            "EnableKey",
            json!({"KeyId": key_id}),
        ),
        (
            "second schedule",
            "ScheduleKeyDeletion",
            json!({"KeyId": key_id}),
        ),
    ] {
        let (status, code) = failure(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "KMSInvalidStateException",
            "case {case}: expected KMSInvalidStateException, got {code}"
        );
    }
}

/// `ScheduleKeyDeletion` sets the deletion date to the documented waiting period
/// after the call, defaults that period to 30 days, and refuses a period outside
/// the documented 7..30 window.
#[tokio::test]
async fn schedule_key_deletion_honours_the_documented_waiting_period() {
    let router = router();

    // Rows: the waiting period the request names, and the one it gets.
    let accepted: Vec<(&str, Value, i64)> = vec![
        ("the default waiting period", json!({}), 30),
        ("seven days", json!({"PendingWindowInDays": 7}), 7),
        ("twenty days", json!({"PendingWindowInDays": 20}), 20),
        ("the documented maximum", json!({"PendingWindowInDays": 30}), 30),
    ];
    for (case, extra, expected_days) in accepted {
        let metadata = create_key(&router, &json!({"Description": case})).await;
        let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();

        let mut request = json!({"KeyId": key_id});
        if let Some(window) = extra.get("PendingWindowInDays") {
            request["PendingWindowInDays"] = window.clone();
        }
        let before = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("the clock is after the epoch")
            .as_secs_f64();
        let scheduled = ok(&router, "ScheduleKeyDeletion", &request).await;

        assert_eq!(
            scheduled["KeyId"], metadata["Arn"],
            "case {case}: the answer names the key ARN, got {scheduled}"
        );
        assert_eq!(
            scheduled["PendingWindowInDays"],
            json!(expected_days),
            "case {case}: expected a {expected_days}-day waiting period, got {scheduled}"
        );
        let deletion_date = scheduled["DeletionDate"].as_f64().unwrap_or_default();
        #[allow(clippy::cast_precision_loss)]
        let expected_seconds = (expected_days * 24 * 60 * 60) as f64;
        let elapsed = deletion_date - before;
        assert!(
            (elapsed - expected_seconds).abs() < 120.0,
            "case {case}: expected a deletion date {expected_seconds}s after the call, \
             got {elapsed}s"
        );

        // The store, not the response, is what a later read reports.
        let described = describe(&router, &key_id).await;
        assert_eq!(
            described["KeyState"],
            json!("PendingDeletion"),
            "case {case}: expected PendingDeletion, got {described}"
        );
        assert_eq!(
            described["Enabled"],
            json!(false),
            "case {case}: a key pending deletion is not enabled, got {described}"
        );
        assert_eq!(
            described["DeletionDate"], scheduled["DeletionDate"],
            "case {case}: the stored deletion date is the one the call returned, got {described}"
        );
    }

    // Rows outside the documented window, each refused before anything changes.
    let metadata = create_key(&router, &json!({"Description": "untouched"})).await;
    let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();
    for days in [1, 6, 31, 365] {
        let (status, code) = failure(
            &router,
            "ScheduleKeyDeletion",
            &json!({"KeyId": key_id, "PendingWindowInDays": days}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {days} days: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "ValidationException",
            "case {days} days: expected ValidationException, got {code}"
        );
    }
    // And rows outside the modelled range, which the boundary refuses.
    for days in [0, 366] {
        let (status, code) = failure(
            &router,
            "ScheduleKeyDeletion",
            &json!({"KeyId": key_id, "PendingWindowInDays": days}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {days} days: expected HTTP 400, got {status} ({code})"
        );
    }
    assert_eq!(
        describe(&router, &key_id).await,
        metadata,
        "a refused ScheduleKeyDeletion must leave the key exactly as it was"
    );
}

// ---------------------------------------------------------------------------
// Key identifiers
// ---------------------------------------------------------------------------

/// An identifier that names no key is `NotFoundException`; an identifier that is an
/// ARN but not a KMS key or alias ARN is `InvalidArnException` on the operations
/// that model it.
#[tokio::test]
async fn unknown_and_malformed_key_identifiers_are_refused() {
    let router = router();
    let account = engine::emulator_account_id();
    let metadata = enabled_key(&router).await;
    let key_id = metadata["KeyId"].as_str().expect("a key id").to_string();

    // Well-formed identifiers that name nothing here.
    let not_found: Vec<(&str, String)> = vec![
        (
            "unknown key id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        (
            "unknown key ARN",
            format!("arn:aws:kms:{REGION}:{account}:key/00000000-0000-0000-0000-000000000000"),
        ),
        ("unknown alias", "alias/no-such-alias".to_string()),
        (
            "unknown alias ARN",
            format!("arn:aws:kms:{REGION}:{account}:alias/no-such-alias"),
        ),
        (
            "this key's ARN in another Region",
            format!("arn:aws:kms:eu-west-1:{account}:key/{key_id}"),
        ),
        (
            "this key's ARN in another account",
            format!("arn:aws:kms:{REGION}:999999999999:key/{key_id}"),
        ),
    ];
    for (case, identifier) in not_found {
        for operation in ["DescribeKey", "ScheduleKeyDeletion", "DisableKey", "EnableKey"] {
            let (status, code) =
                failure(&router, operation, &json!({"KeyId": identifier})).await;
            assert_eq!(
                status, 400,
                "case {case} on {operation}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "NotFoundException",
                "case {case} on {operation}: expected NotFoundException, got {code}"
            );
        }
        let (status, code) = failure(
            &router,
            "Encrypt",
            &json!({"KeyId": identifier, "Plaintext": b64(b"hello kms")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} on Encrypt: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case} on Encrypt: expected NotFoundException, got {code}"
        );
    }

    // ARNs that are not KMS key or alias ARNs at all.
    let malformed: Vec<(&str, String)> = vec![
        (
            "unknown resource type",
            format!("arn:aws:kms:{REGION}:{account}:thing/{key_id}"),
        ),
        ("another service", "arn:aws:s3:::my-bucket".to_string()),
        (
            "empty key id",
            format!("arn:aws:kms:{REGION}:{account}:key/"),
        ),
        (
            "no Region",
            format!("arn:aws:kms::{account}:key/{key_id}"),
        ),
        ("too few segments", "arn:aws:kms".to_string()),
        ("nothing but the prefix", "arn:".to_string()),
    ];
    for (case, identifier) in malformed {
        for operation in ["DescribeKey", "ScheduleKeyDeletion", "DisableKey", "EnableKey"] {
            let (status, code) =
                failure(&router, operation, &json!({"KeyId": identifier})).await;
            assert_eq!(
                status, 400,
                "case {case} on {operation}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "InvalidArnException",
                "case {case} on {operation}: expected InvalidArnException, got {code}"
            );
        }
        // `Encrypt` models no InvalidArnException, so it reports the same refusal
        // it reports for an identifier that names nothing.
        let (status, code) = failure(
            &router,
            "Encrypt",
            &json!({"KeyId": identifier, "Plaintext": b64(b"hello kms")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case} on Encrypt: expected HTTP 400, got {status} ({code})"
        );
        assert_eq!(
            code, "NotFoundException",
            "case {case} on Encrypt: expected NotFoundException, got {code}"
        );
    }
}

/// A KMS key is a regional resource: the endpoint that created it is the only one
/// that lists, describes or uses it.
#[tokio::test]
async fn keys_are_scoped_to_the_region_that_created_them() {
    let router = router();

    // Rows: a Region, and the keys created through its endpoint.
    let regions = ["us-east-1", "us-west-2", "eu-west-1"];
    let mut created: Vec<(&str, Vec<Value>)> = Vec::new();
    for (index, region) in regions.iter().enumerate() {
        let mut keys = Vec::new();
        for n in 0..=index {
            let metadata = ok_in(
                &router,
                region,
                "CreateKey",
                &json!({"Description": format!("{region}-{n}")}),
            )
            .await["KeyMetadata"]
                .clone();
            assert_eq!(
                metadata["Arn"].as_str().unwrap_or_default(),
                format!(
                    "arn:aws:kms:{region}:{}:key/{}",
                    engine::emulator_account_id(),
                    metadata["KeyId"].as_str().unwrap_or_default()
                ),
                "case {region}: the ARN carries the Region the request was signed for"
            );
            keys.push(metadata);
        }
        created.push((region, keys));
    }

    for (region, keys) in &created {
        let listed = ok_in(&router, region, "ListKeys", &json!({})).await;
        let expected: Vec<Value> = keys
            .iter()
            .map(|metadata| json!({"KeyId": metadata["KeyId"], "KeyArn": metadata["Arn"]}))
            .collect();
        assert_eq!(
            listed["Keys"],
            json!(expected),
            "case {region}: ListKeys reports only the keys of this Region, got {listed}"
        );

        // A key of another Region is not visible here.
        for (other_region, other_keys) in &created {
            if other_region == region {
                continue;
            }
            for metadata in other_keys {
                let key_id = metadata["KeyId"].as_str().unwrap_or_default();
                let (status, code) =
                    failure_in(&router, region, "DescribeKey", &json!({"KeyId": key_id})).await;
                assert_eq!(
                    status, 400,
                    "case {other_region}'s key seen from {region}: expected HTTP 400, \
                     got {status} ({code})"
                );
                assert_eq!(
                    code, "NotFoundException",
                    "case {other_region}'s key seen from {region}: expected NotFoundException, \
                     got {code}"
                );
            }
        }
    }
}

/// `Encrypt` and `Decrypt` refuse what they cannot honour — a grant token nothing
/// issued, an algorithm the key spec does not support, a dry run, and a `Recipient`
/// whose attestation cannot be verified — and a refused call changes nothing.
#[tokio::test]
async fn cryptographic_operations_refuse_what_they_cannot_honour() {
    let router = router();
    let metadata = enabled_key(&router).await;
    let arn = metadata["Arn"].as_str().expect("an ARN").to_string();
    let plaintext = b"hello kms".to_vec();
    let sealed = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": arn, "Plaintext": b64(&plaintext)}),
    )
    .await;
    let blob = b64(&unb64("sealed", &sealed, "CiphertextBlob"));

    let cases: Vec<(&str, &str, Value, u16, &str)> = vec![
        (
            "encrypt with a grant token",
            "Encrypt",
            json!({"KeyId": arn, "Plaintext": b64(&plaintext), "GrantTokens": ["token"]}),
            400,
            "InvalidGrantTokenException",
        ),
        (
            "decrypt with a grant token",
            "Decrypt",
            json!({"CiphertextBlob": blob, "GrantTokens": ["token"]}),
            400,
            "InvalidGrantTokenException",
        ),
        (
            "encrypt with an asymmetric algorithm",
            "Encrypt",
            json!({"KeyId": arn, "Plaintext": b64(&plaintext),
                   "EncryptionAlgorithm": "RSAES_OAEP_SHA_256"}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "decrypt with an asymmetric algorithm",
            "Decrypt",
            json!({"CiphertextBlob": blob, "EncryptionAlgorithm": "SM2PKE"}),
            400,
            "InvalidKeyUsageException",
        ),
        (
            "encrypt as a dry run",
            "Encrypt",
            json!({"KeyId": arn, "Plaintext": b64(&plaintext), "DryRun": true}),
            400,
            "DryRunOperationException",
        ),
        (
            "decrypt as a dry run",
            "Decrypt",
            json!({"CiphertextBlob": blob, "DryRun": true}),
            400,
            "DryRunOperationException",
        ),
        (
            "decrypt for an enclave recipient",
            "Decrypt",
            json!({"CiphertextBlob": blob,
                   "Recipient": {"KeyEncryptionAlgorithm": "RSAES_OAEP_SHA_256",
                                 "AttestationDocument": b64(b"not an attestation")}}),
            501,
            "NotImplemented",
        ),
    ];
    for (case, operation, body, expected_status, expected_code) in cases {
        let (status, code) = failure(&router, operation, &body).await;
        assert_eq!(
            status, expected_status,
            "case {case}: expected HTTP {expected_status}, got {status} ({code})"
        );
        assert_eq!(
            code, expected_code,
            "case {case}: expected {expected_code}, got {code}"
        );
    }

    // `DryRun: false` is not a dry run, and none of the refusals above disturbed
    // the key or the ciphertext.
    let again = ok(
        &router,
        "Encrypt",
        &json!({"KeyId": arn, "Plaintext": b64(&plaintext), "DryRun": false}),
    )
    .await;
    assert_eq!(
        again["KeyId"].as_deref_or_empty(),
        arn,
        "case DryRun false: the call must be performed, got {again}"
    );
    let decrypted = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
    assert_eq!(
        unb64("after refusals", &decrypted, "Plaintext"),
        plaintext,
        "case after refusals: the ciphertext sealed before them must still open"
    );
    assert_eq!(
        describe(&router, arn.as_str()).await,
        metadata,
        "case after refusals: a refused cryptographic operation must not change the key"
    );
}

/// The store is bounded: `CreateKey` refuses past the key count and the per-key tag
/// quota with the modelled `LimitExceededException`, and persists nothing when it
/// refuses.
#[tokio::test]
async fn the_key_store_is_bounded() {
    use service_com_amazonaws_kms_trentservice::state::{MAX_KEYS, MAX_TAGS_PER_KEY};

    let router = router();

    // The tag quota, which bounds what one request can make the store retain.
    let tag_cases: Vec<(&str, usize, bool)> = vec![
        ("at the tag quota", MAX_TAGS_PER_KEY, true),
        ("one tag past the quota", MAX_TAGS_PER_KEY + 1, false),
        ("far past the quota", MAX_TAGS_PER_KEY * 10, false),
    ];
    for (case, count, accepted) in tag_cases {
        let tags: Vec<Value> = (0..count)
            .map(|n| json!({"TagKey": format!("key-{n}"), "TagValue": format!("value-{n}")}))
            .collect();
        let request = json!({"Tags": tags});
        if accepted {
            let metadata = create_key(&router, &request).await;
            assert!(
                metadata["KeyId"].is_string(),
                "case {case}: expected a key, got {metadata}"
            );
        } else {
            let (status, code) = failure(&router, "CreateKey", &request).await;
            assert_eq!(
                status, 400,
                "case {case}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "LimitExceededException",
                "case {case}: expected LimitExceededException, got {code}"
            );
        }
    }

    // The key count. One key already exists from the accepted tag row above.
    let mut created = 1;
    while created < MAX_KEYS {
        create_key(&router, &json!({})).await;
        created += 1;
    }
    let (status, code) = failure(&router, "CreateKey", &json!({})).await;
    assert_eq!(
        status, 400,
        "case one key past the bound: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "LimitExceededException",
        "case one key past the bound: expected LimitExceededException, got {code}"
    );

    let listed = ok(&router, "ListKeys", &json!({"Limit": 1000})).await;
    assert_eq!(
        listed["Keys"].as_array().map(Vec::len),
        Some(MAX_KEYS),
        "case at the bound: the store holds exactly {MAX_KEYS} keys, got {}",
        listed["Keys"].as_array().map_or(0, Vec::len)
    );
    assert_eq!(
        listed["Truncated"],
        json!(false),
        "case at the bound: a refused create adds nothing to page over, got {}",
        listed["Truncated"]
    );
}

/// Helper trait: a string member, or the empty string — so an assertion can compare
/// a member against an owned `String` without unwrapping first.
trait AsStrOrEmpty {
    fn as_deref_or_empty(&self) -> &str;
}

impl AsStrOrEmpty for Value {
    fn as_deref_or_empty(&self) -> &str {
        self.as_str().unwrap_or_default()
    }
}
