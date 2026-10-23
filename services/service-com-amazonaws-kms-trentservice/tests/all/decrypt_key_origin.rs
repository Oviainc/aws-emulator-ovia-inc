// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the origin `Decrypt` reports, driven over the wire through the
//! shared [`super::harness`].
//!
//! The rule under test: a successful `Decrypt` reports where the key material that
//! opened the ciphertext comes from — the `OriginType` of the resolved key — and it is
//! the SAME value `DescribeKey` reports as `KeyMetadata.Origin` for that key, because
//! both are projections of one stored key record. A key KMS generated the material for
//! therefore reports `AWS_KMS`, the documented default origin ("The default is AWS_KMS,
//! which means that KMS creates the key material",
//! `smithy_query(query="doc",shape="CreateKeyRequest",member="Origin")`).
//!
//! The assertions are made over a TABLE whose rows differ in the plaintext (its length,
//! including each modelled bound), in the encryption context bound into the ciphertext,
//! in how the `Decrypt` request names the key (not at all, by key id, by key ARN, by
//! alias) and in which key sealed it — the member belongs to the response, not to one
//! request shape.
//!
//! One more test here is about the MODEL rather than the wire: the snapshot in
//! `smithy-model/` is kept exactly as shipped and does NOT declare this member for
//! `Decrypt` (it declares it on `DeriveSharedSecretResponse` only), which is why
//! `src/ops/decrypt.rs` declares the member schema itself; the last test pins that, so
//! a refreshed snapshot that adds the member is noticed instead of being shadowed.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{b64, create_key, describe, ok, router, text};

/// Every `OriginType` the model declares (`smithy_query(query="shape",shape="OriginType")`),
/// which is the set a reported origin must come from.
const ORIGIN_TYPE_VALUES: &[&str] = &["AWS_KMS", "EXTERNAL", "AWS_CLOUDHSM", "EXTERNAL_KEY_STORE"];

/// Seal `plaintext` under `key_id` with `context`, and return the ciphertext blob as
/// the wire carries it.
async fn encrypt(
    router: &MultiServiceRouter,
    case: &str,
    key_id: &str,
    plaintext: &[u8],
    context: Option<&Value>,
) -> Value {
    let mut request = json!({"KeyId": key_id, "Plaintext": b64(plaintext)});
    if let Some(context) = context {
        request["EncryptionContext"] = context.clone();
    }
    let answer = ok(router, "Encrypt", &request).await;
    assert!(
        answer["CiphertextBlob"].is_string(),
        "case {case}: Encrypt must answer a ciphertext, got {answer}"
    );
    answer["CiphertextBlob"].clone()
}

/// One row of the table: the case name, the plaintext, and the encryption context it
/// is sealed under.
///
/// The plaintexts span `PlaintextType`'s modelled `len 1..4096`
/// (`smithy_query(query="operation",operation="Encrypt")`): the shortest the bound
/// allows, a middling one, and the longest. The contexts span an absent one, one
/// entry, and several.
fn rows() -> Vec<(&'static str, Vec<u8>, Option<Value>)> {
    vec![
        ("one byte, no context", b"x".to_vec(), None),
        (
            "a short plaintext with one context entry",
            b"origin-probe".to_vec(),
            Some(json!({"purpose": "test"})),
        ),
        (
            "a binary plaintext with several context entries",
            (0u8..=255).collect(),
            Some(json!({"a": "1", "b": "2", "c": "3"})),
        ),
        (
            "the longest plaintext the modelled length allows",
            vec![7u8; 4096],
            None,
        ),
    ]
}

/// Every successful `Decrypt` reports the resolved key's origin, and it is exactly the
/// origin `DescribeKey` reports for that key.
#[tokio::test]
async fn decrypt_reports_the_origin_describe_key_reports_for_the_same_key() {
    let router = router();

    // Two keys, so the reported origin is read per key rather than from one global
    // answer, and an alias so the key can also be named indirectly.
    let first = create_key(&router, &json!({"Description": "first"})).await;
    let second = create_key(&router, &json!({"Description": "second"})).await;
    let first_id = text("setup", &first, "KeyId");
    let first_arn = text("setup", &first, "Arn");
    let second_id = text("setup", &second, "KeyId");
    let alias = "alias/decrypting-key";
    ok(
        &router,
        "CreateAlias",
        &json!({"AliasName": alias, "TargetKeyId": first_id}),
    )
    .await;

    for (case, plaintext, context) in rows() {
        for (key_case, sealing_key) in [("first key", &first_id), ("second key", &second_id)] {
            let blob = encrypt(&router, case, sealing_key, &plaintext, context.as_ref()).await;
            let expected_origin = text(
                key_case,
                &describe(&router, sealing_key).await,
                "Origin",
            );

            // The `KeyId` member of a `Decrypt` request is optional for a symmetric
            // ciphertext, so the rows include omitting it and naming the key in each
            // form that resolves to it.
            let mut named: Vec<(&str, Option<String>)> =
                vec![("no KeyId", None), ("by key id", Some(sealing_key.clone()))];
            if sealing_key == &first_id {
                named.push(("by key ARN", Some(first_arn.clone())));
                named.push(("by alias", Some(alias.to_string())));
            }

            for (form, key_id) in named {
                let case = format!("{case} / {key_case} / {form}");
                let mut request = json!({"CiphertextBlob": blob});
                if let Some(key_id) = key_id {
                    request["KeyId"] = json!(key_id);
                }
                if let Some(context) = &context {
                    request["EncryptionContext"] = context.clone();
                }
                let decrypted = ok(&router, "Decrypt", &request).await;

                assert_eq!(
                    decrypted["Plaintext"],
                    json!(b64(&plaintext)),
                    "case {case}: Decrypt must return the plaintext that was sealed, got \
                     {decrypted}"
                );
                let reported = decrypted["KeyOrigin"].as_str().unwrap_or_else(|| {
                    panic!("case {case}: expected a KeyOrigin member in {decrypted}")
                });
                assert_eq!(
                    reported, expected_origin,
                    "case {case}: Decrypt must report the origin DescribeKey reports \
                     ({expected_origin}), got {reported}"
                );
                assert!(
                    ORIGIN_TYPE_VALUES.contains(&reported),
                    "case {case}: the reported origin must be a modelled OriginType, got \
                     {reported}"
                );
            }
        }
    }
}

/// The origin of a key KMS made the material for is the documented default, `AWS_KMS`
/// — and it is reported for the key the CIPHERTEXT names, not for whichever key was
/// created last.
#[tokio::test]
async fn a_kms_generated_key_reports_the_documented_default_origin() {
    let router = router();

    // Rows: the `CreateKey` request, which varies the key's description and its
    // multi-Region-ness. Every one of them asks KMS for the key material, so every one
    // of them has the default origin.
    let requests: Vec<(&str, Value)> = vec![
        ("default symmetric key", json!({"Description": "default"})),
        ("no members at all", json!({})),
        (
            "an explicit AWS_KMS origin",
            json!({"Origin": "AWS_KMS", "Description": "explicit"}),
        ),
        ("a multi-Region key", json!({"MultiRegion": true})),
    ];
    for (case, request) in requests {
        let metadata = create_key(&router, &request).await;
        let key_id = text(case, &metadata, "KeyId");
        assert_eq!(
            text(case, &metadata, "Origin"),
            "AWS_KMS",
            "case {case}: a key KMS generated the material for has the default origin"
        );

        let blob = encrypt(&router, case, &key_id, b"default origin", None).await;
        let decrypted = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
        assert_eq!(
            decrypted["KeyOrigin"],
            json!("AWS_KMS"),
            "case {case}: Decrypt must report AWS_KMS for a KMS-generated key, got \
             {decrypted}"
        );
        assert_eq!(
            decrypted["KeyId"],
            json!(text(case, &metadata, "Arn")),
            "case {case}: Decrypt must report the key that opened the ciphertext, got \
             {decrypted}"
        );
    }
}

/// The model snapshot is untouched: it does NOT declare `DecryptResponse$KeyOrigin`,
/// which is why `src/ops/decrypt.rs` declares that member itself — and it still
/// declares the member on `DeriveSharedSecretResponse`, which is where the wording and
/// the `OriginType` target come from.
///
/// This pins the REASON the member is written locally. If a refreshed snapshot ever
/// declares it on `DecryptResponse`, this test fails, and the local declaration should
/// give way to the modelled member rather than the two drifting apart.
#[test]
fn the_model_snapshot_does_not_declare_the_decrypt_key_origin_member() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("smithy-model/com-amazonaws-kms-trentservice.json");
    let model: Value = serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the model snapshot is readable"),
    )
    .expect("the model snapshot is JSON");
    let shapes = &model["shapes"];

    // `DecryptResponse` carries exactly its five published members.
    let members = shapes["com.amazonaws.kms#DecryptResponse"]["members"]
        .as_object()
        .expect("DecryptResponse declares members");
    let mut declared: Vec<&str> = members.keys().map(String::as_str).collect();
    declared.sort_unstable();
    assert_eq!(
        declared,
        vec![
            "CiphertextForRecipient",
            "EncryptionAlgorithm",
            "KeyId",
            "KeyMaterialId",
            "Plaintext",
        ],
        "the snapshot must be as shipped: DecryptResponse declares its five published \
         members and no KeyOrigin, got {declared:?}"
    );

    // The shape that DOES declare the member, and the enum both would target.
    assert_eq!(
        shapes["com.amazonaws.kms#DeriveSharedSecretResponse"]["members"]["KeyOrigin"]["target"],
        json!("com.amazonaws.kms#OriginType"),
        "the published KeyOrigin member must still be declared as it was"
    );
    let mut origins: Vec<&str> = shapes["com.amazonaws.kms#OriginType"]["members"]
        .as_object()
        .expect("OriginType declares members")
        .keys()
        .map(String::as_str)
        .collect();
    origins.sort_unstable();
    let mut published = ORIGIN_TYPE_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        origins, published,
        "OriginType must keep exactly its published values, got {origins:?}"
    );
}
