// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateDataKey`, driven over the wire through the shared
//! [`super::harness`].
//!
//! The rule under test: the operation draws a random data key of the length the request
//! states — through `KeySpec` or through `NumberOfBytes`, never both and never neither —
//! and answers that one data key twice over: in the clear, and sealed under the current
//! material of the symmetric encryption KMS key the `KeyId` resolves to. A request that
//! does not state exactly one length is refused and nothing is generated.
//!
//! Every assertion is made over a TABLE: the rows differ in how the length is stated
//! (each `DataKeySpec` value, and byte counts across the modelled `NumberOfBytes` range
//! including both of its bounds), in the encryption context bound into the ciphertext,
//! in how the request names the key, and — for the refusals — in how many times, and
//! with what values, the length is stated.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    call, contains, create_alias, create_key, describe, error_code, is_lower_hex, ok, router, text,
    unb64, REGION,
};

/// The operation under test.
const OPERATION: &str = "GenerateDataKey";

/// The length of a `BackingKeyIdType`: 64 characters matching `^[a-f0-9]+$`
/// (`smithy_query(query="operation",operation="GenerateDataKey")`).
const KEY_MATERIAL_ID_LEN: usize = 64;

/// The members a `GenerateDataKey` answer may carry
/// (`smithy_query(query="operation",operation="GenerateDataKey")`).
/// `CiphertextForRecipient` belongs to a `Recipient` request alone, so a plain request
/// must not see it.
const RESPONSE_MEMBERS: [&str; 4] = ["CiphertextBlob", "Plaintext", "KeyId", "KeyMaterialId"];

/// The shortest data key for which two consecutive draws can be required to
/// differ.
///
/// A drawn key of fewer bytes than this collides often enough to be seen: two
/// one-byte keys are the same value once in 256 draws, so requiring one pair to
/// differ would fail runs in which nothing is wrong. Those rows assert the same
/// rule over [`SHORT_KEY_DRAWS`] draws instead. The threshold is the one the
/// ciphertext-containment assertion in this file already uses for the same
/// reason.
const MIN_UNIQUE_BY_PAIR_BYTES: usize = 8;

/// How many data keys a row shorter than [`MIN_UNIQUE_BY_PAIR_BYTES`] draws
/// before asserting that they are not all the same value.
///
/// Enough that every draw coming back equal is not something a run will see by
/// chance (for a one-byte key, 256 to the power of -23), while a key that is
/// derived from the request rather than drawn — the defect the assertion is
/// about — is the same value every time and still fails.
const SHORT_KEY_DRAWS: usize = 24;

/// How a row states the length of the data key it asks for.
#[derive(Clone, Copy)]
enum Length {
    /// A `DataKeySpec` value.
    Spec(&'static str),
    /// A `NumberOfBytes` value.
    Bytes(i64),
}

/// One row of the length table: the case name, how the length is stated, and how many
/// bytes of data key that states.
struct LengthCase {
    case: &'static str,
    length: Length,
    bytes: usize,
}

/// The ways a request can state the length of its data key.
///
/// The `Spec` rows are EVERY value of `DataKeySpec`
/// (`smithy_query(query="shape",shape="DataKeySpec")`: `AES_128`, `AES_256`), with the
/// byte count each one names — "Use AES_128 to generate a 128-bit symmetric key, or
/// AES_256 to generate a 256-bit symmetric key"
/// (`smithy_query(query="doc",shape="GenerateDataKeyRequest",member="KeySpec")`). The
/// `Bytes` rows span `NumberOfBytesType`'s modelled range (1..1024): both published
/// bounds, the documented example "use the value 64 to generate a 512-bit data key",
/// and a middling value that is not a whole number of AES blocks.
const LENGTH_CASES: &[LengthCase] = &[
    LengthCase {
        case: "KeySpec AES_128",
        length: Length::Spec("AES_128"),
        bytes: 16,
    },
    LengthCase {
        case: "KeySpec AES_256",
        length: Length::Spec("AES_256"),
        bytes: 32,
    },
    LengthCase {
        case: "NumberOfBytes 1, the modelled minimum",
        length: Length::Bytes(1),
        bytes: 1,
    },
    LengthCase {
        case: "NumberOfBytes 23, not a whole number of AES blocks",
        length: Length::Bytes(23),
        bytes: 23,
    },
    LengthCase {
        case: "NumberOfBytes 64, the documented 512-bit example",
        length: Length::Bytes(64),
        bytes: 64,
    },
    LengthCase {
        case: "NumberOfBytes 1024, the modelled maximum",
        length: Length::Bytes(1024),
        bytes: 1024,
    },
];

/// An encryption-context map as awsJson1_1 carries one.
fn context(pairs: &[(&str, &str)]) -> Value {
    Value::Object(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// The encryption contexts the rows are crossed with: absent, one entry, several.
fn context_cases() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no encryption context", None),
        ("one context entry", Some(context(&[("purpose", "test")]))),
        (
            "several context entries",
            Some(context(&[("a", "1"), ("b", "2"), ("c", "3")])),
        ),
    ]
}

/// The request a row describes against `key_id`.
fn request(length: Length, key_id: &str, encryption_context: Option<&Value>) -> Value {
    let mut request = json!({"KeyId": key_id});
    match length {
        Length::Spec(spec) => request["KeySpec"] = json!(spec),
        Length::Bytes(bytes) => request["NumberOfBytes"] = json!(bytes),
    }
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    request
}

/// Assert the answer carries only the modelled members and both copies of the data key,
/// and return the plaintext bytes together with the ciphertext as the wire carries it.
fn both_copies(case: &str, answer: &Value) -> (Vec<u8>, Value) {
    let members = answer
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: expected a response object, got {answer}"));
    for member in members.keys() {
        assert!(
            RESPONSE_MEMBERS.contains(&member.as_str()),
            "case {case}: the response must carry only {RESPONSE_MEMBERS:?}, got the member \
             {member:?} in {answer}"
        );
    }
    assert!(
        answer["CiphertextBlob"].is_string(),
        "case {case}: expected a CiphertextBlob, got {answer}"
    );
    (
        unb64(case, answer, "Plaintext"),
        answer["CiphertextBlob"].clone(),
    )
}

/// The plaintext `Decrypt` recovers from `blob`, with the ciphertext's own encryption
/// context, together with the key material id it reports.
async fn decrypted(
    router: &MultiServiceRouter,
    case: &str,
    blob: &Value,
    encryption_context: Option<&Value>,
) -> (Vec<u8>, Value) {
    let mut request = json!({"CiphertextBlob": blob.clone()});
    if let Some(encryption_context) = encryption_context {
        request["EncryptionContext"] = encryption_context.clone();
    }
    let answer = ok(router, "Decrypt", &request).await;
    (
        unb64(case, &answer, "Plaintext"),
        answer["KeyMaterialId"].clone(),
    )
}

/// A data key of the requested length comes back in the clear and sealed, the sealed
/// copy opens to exactly the plaintext copy, and every call draws a new data key.
#[tokio::test]
async fn both_copies_are_the_same_data_key_of_the_requested_length() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "data keys"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    let key_arn = text("setup", &metadata, "Arn");
    let material_id = text("setup", &describe(&router, &key_id).await, "CurrentKeyMaterialId");

    for row in LENGTH_CASES {
        for (context_case, encryption_context) in context_cases() {
            let case = format!("{} / {context_case}", row.case);
            let body = request(row.length, &key_id, encryption_context.as_ref());

            let first = ok(&router, OPERATION, &body).await;
            let (plaintext, blob) = both_copies(&case, &first);
            assert_eq!(
                plaintext.len(),
                row.bytes,
                "case {case}: expected a {}-byte data key, got {} bytes",
                row.bytes,
                plaintext.len()
            );
            assert_eq!(
                first["KeyId"],
                json!(key_arn),
                "case {case}: the answer must name the sealing key by its ARN, got {first}"
            );
            let reported_material = text(&case, &first, "KeyMaterialId");
            assert_eq!(
                reported_material, material_id,
                "case {case}: the answer must report the key material that sealed the data \
                 key ({material_id}), got {reported_material}"
            );
            assert!(
                reported_material.len() == KEY_MATERIAL_ID_LEN && is_lower_hex(&reported_material),
                "case {case}: a KeyMaterialId is {KEY_MATERIAL_ID_LEN} lowercase hex \
                 characters, got {reported_material:?}"
            );

            // The two copies are ONE data key: the sealed one opens to the plaintext one.
            let (opened, decrypt_material) =
                decrypted(&router, &case, &blob, encryption_context.as_ref()).await;
            assert_eq!(
                opened, plaintext,
                "case {case}: the sealed copy must open to the data key that was returned \
                 in the clear"
            );
            assert_eq!(
                decrypt_material,
                json!(material_id),
                "case {case}: Decrypt must report the material that sealed the data key, \
                 got {decrypt_material}"
            );

            // The ciphertext is a ciphertext: it does not simply carry the data key. A
            // very short data key can appear in a blob's own random bytes by chance, so
            // this is asserted on the rows where it cannot.
            let blob_bytes = unb64(&case, &first, "CiphertextBlob");
            if plaintext.len() >= 8 {
                assert!(
                    !contains(&blob_bytes, &plaintext),
                    "case {case}: the ciphertext must not carry the data key in the clear"
                );
            }

            // "The bytes in the plaintext key are random": a second call on the same
            // request draws a different data key, not the same one re-sealed.
            let second = ok(&router, OPERATION, &body).await;
            let (other, other_blob) = both_copies(&case, &second);
            assert_eq!(
                other.len(),
                row.bytes,
                "case {case}: every call must answer a data key of the requested length, \
                 got {} bytes",
                other.len()
            );
            if row.bytes >= MIN_UNIQUE_BY_PAIR_BYTES {
                assert_ne!(
                    plaintext, other,
                    "case {case}: two calls must draw two different data keys"
                );
            } else {
                // A key this short can repeat by chance, so the rule is asserted over a
                // sample of draws rather than over one pair.
                let mut drawn = vec![plaintext.clone(), other.clone()];
                while drawn.len() < SHORT_KEY_DRAWS {
                    let again = ok(&router, OPERATION, &body).await;
                    drawn.push(both_copies(&case, &again).0);
                }
                let distinct: std::collections::BTreeSet<&Vec<u8>> = drawn.iter().collect();
                assert!(
                    distinct.len() > 1,
                    "case {case}: {SHORT_KEY_DRAWS} calls must not all draw the same data key, \
                     got {drawn:?}"
                );
            }
            assert_ne!(
                blob, other_blob,
                "case {case}: two calls must answer two different ciphertexts"
            );

            // The encryption context is authenticated, so the sealed copy does not open
            // under any other context.
            let wrong = match &encryption_context {
                // A ciphertext sealed with no context is not openable under one.
                None => Some(context(&[("unexpected", "context")])),
                // A ciphertext sealed WITH a context is not openable without it.
                Some(_) => None,
            };
            let mut probe = json!({"CiphertextBlob": blob.clone()});
            if let Some(wrong) = &wrong {
                probe["EncryptionContext"] = wrong.clone();
            }
            let (status, payload) = call(&router, REGION, "Decrypt", &probe).await;
            assert_eq!(
                (status, error_code(&payload).as_str()),
                (400, "InvalidCiphertextException"),
                "case {case}: a data key sealed under one encryption context must not open \
                 under another, got {status} with {payload}"
            );
        }
    }
}

/// Whatever identifier names the key — key id, key ARN or alias — the answer reports
/// that key's ARN and the material of THAT key, read back through `DescribeKey`.
#[tokio::test]
async fn the_answer_names_the_resolved_key_by_arn_whatever_identifier_named_it() {
    let router = router();

    // Two keys, so the answer is read per key rather than from one global value, and an
    // alias so each can also be named indirectly.
    let mut keys = Vec::new();
    for (case, description) in [("first key", "first"), ("second key", "second")] {
        let metadata = create_key(&router, &json!({"Description": description})).await;
        let key_id = text(case, &metadata, "KeyId");
        let key_arn = text(case, &metadata, "Arn");
        let alias = format!("alias/generated-data-key-{description}");
        create_alias(&router, case, &alias, &key_id).await;
        let material_id = text(case, &describe(&router, &key_id).await, "CurrentKeyMaterialId");
        keys.push((case, key_id, key_arn, alias, material_id));
    }

    for (key_case, key_id, key_arn, alias, material_id) in &keys {
        for (form, identifier) in [
            ("by key id", key_id.as_str()),
            ("by key ARN", key_arn.as_str()),
            ("by alias", alias.as_str()),
        ] {
            for row in LENGTH_CASES {
                let case = format!("{key_case} / {form} / {}", row.case);
                let answer = ok(&router, OPERATION, &request(row.length, identifier, None)).await;
                let (plaintext, blob) = both_copies(&case, &answer);
                assert_eq!(
                    answer["KeyId"],
                    json!(key_arn),
                    "case {case}: the answer must name the resolved key by its ARN, got \
                     {answer}"
                );
                assert_eq!(
                    answer["KeyMaterialId"],
                    json!(material_id),
                    "case {case}: the answer must report that key's current material, got \
                     {answer}"
                );

                // The ciphertext is a ciphertext OF that key: `Decrypt` opens it, reports
                // the same key, and returns the same data key.
                let opened = ok(&router, "Decrypt", &json!({"CiphertextBlob": blob})).await;
                assert_eq!(
                    opened["KeyId"],
                    json!(key_arn),
                    "case {case}: Decrypt must report the key that sealed the data key, got \
                     {opened}"
                );
                assert_eq!(
                    unb64(&case, &opened, "Plaintext"),
                    plaintext,
                    "case {case}: the sealed copy must open to the data key returned in the \
                     clear"
                );
            }
        }
    }
}

/// How a row of the exclusivity table states the length of its data key, and whether
/// that is a request KMS serves.
struct ExclusivityCase {
    case: &'static str,
    /// The members the request carries beside `KeyId`.
    length: Value,
    /// The number of bytes the request asks for, or `None` where it is refused because
    /// it does not state exactly one length.
    bytes: Option<usize>,
}

/// The exclusivity rule, row by row: exactly one of `KeySpec` and `NumberOfBytes` says
/// how long the data key is, so a request stating the length twice — even twice with the
/// same length — or not at all does not say how long it is and is refused. The served
/// rows are the boundary: the same request with one of the two members alone succeeds.
fn exclusivity_cases() -> Vec<ExclusivityCase> {
    vec![
        ExclusivityCase {
            case: "KeySpec alone",
            length: json!({"KeySpec": "AES_128"}),
            bytes: Some(16),
        },
        ExclusivityCase {
            case: "NumberOfBytes alone",
            length: json!({"NumberOfBytes": 16}),
            bytes: Some(16),
        },
        ExclusivityCase {
            case: "neither KeySpec nor NumberOfBytes",
            length: json!({}),
            bytes: None,
        },
        ExclusivityCase {
            case: "both, stating two different lengths",
            length: json!({"KeySpec": "AES_128", "NumberOfBytes": 1024}),
            bytes: None,
        },
        ExclusivityCase {
            case: "both, stating the same length twice",
            length: json!({"KeySpec": "AES_128", "NumberOfBytes": 16}),
            bytes: None,
        },
        ExclusivityCase {
            case: "both, with AES_256",
            length: json!({"KeySpec": "AES_256", "NumberOfBytes": 32}),
            bytes: None,
        },
        // `NumberOfBytesType` is `@range(min: 1, max: 1024)`
        // (`smithy_query(query="shape",shape="NumberOfBytesType")`), so the rows walk
        // each published bound and one past it.
        ExclusivityCase {
            case: "NumberOfBytes 1, the published minimum",
            length: json!({"NumberOfBytes": 1}),
            bytes: Some(1),
        },
        ExclusivityCase {
            case: "NumberOfBytes 0, one below the published minimum",
            length: json!({"NumberOfBytes": 0}),
            bytes: None,
        },
        ExclusivityCase {
            case: "NumberOfBytes 1024, the published maximum",
            length: json!({"NumberOfBytes": 1024}),
            bytes: Some(1024),
        },
        ExclusivityCase {
            case: "NumberOfBytes 1025, one above the published maximum",
            length: json!({"NumberOfBytes": 1025}),
            bytes: None,
        },
        // `DataKeySpec` is an enum of `AES_128` and `AES_256`
        // (`smithy_query(query="shape",shape="DataKeySpec")`), so no other value names a
        // length either.
        ExclusivityCase {
            case: "a KeySpec that is not a DataKeySpec value",
            length: json!({"KeySpec": "AES_512"}),
            bytes: None,
        },
    ]
}

/// A request states the length of its data key exactly once, through `KeySpec` or
/// through `NumberOfBytes`: one of them is served, and any other way of stating the
/// length is refused with `ValidationException` — with nothing generated and no use
/// recorded against the key.
#[tokio::test]
async fn a_request_states_exactly_one_length_or_is_refused() {
    let router = router();
    let metadata = create_key(&router, &json!({"Description": "length exclusivity"})).await;
    let key_id = text("setup", &metadata, "KeyId");
    // The refused rows name a key of their own, which nothing ever generates a data key
    // under: a key no successful call has used holds no usage record, so what
    // `GetKeyLastUsage` reports for it is the empty record whenever it is asked.
    let refused_key = create_key(&router, &json!({"Description": "nothing generated"})).await;
    let refused_key_id = text("setup", &refused_key, "KeyId");

    let fresh = ok(
        &router,
        "GetKeyLastUsage",
        &json!({"KeyId": refused_key_id}),
    )
    .await;
    assert_eq!(
        fresh["KeyLastUsage"],
        json!({}),
        "case setup: a key with no tracked usage record reports an empty KeyLastUsage, got \
         {fresh}"
    );

    let mut served = 0usize;
    for row in exclusivity_cases() {
        // A row that is refused is refused for the lengths it states, so it names the
        // key no call succeeds on; a row that is served names the key the data key is
        // generated under.
        let named_key = if row.bytes.is_some() {
            &key_id
        } else {
            &refused_key_id
        };
        let mut body = json!({"KeyId": named_key});
        for (member, value) in row
            .length
            .as_object()
            .expect("every exclusivity row states its members as an object")
        {
            body[member] = value.clone();
        }
        let (status, payload) = call(&router, REGION, OPERATION, &body).await;

        match row.bytes {
            Some(bytes) => {
                assert_eq!(
                    status, 200,
                    "case {:?}: a request stating exactly one length is served, got {status} \
                     with {payload}",
                    row.case
                );
                let (plaintext, _) = both_copies(row.case, &payload);
                assert_eq!(
                    plaintext.len(),
                    bytes,
                    "case {:?}: expected a {bytes}-byte data key, got {} bytes",
                    row.case,
                    plaintext.len()
                );
                served += 1;
            }
            None => {
                assert_eq!(
                    (status, error_code(&payload).as_str()),
                    (400, "ValidationException"),
                    "case {:?}: a request that does not state exactly one length must be \
                     refused, got {status} with {payload}",
                    row.case
                );
                assert!(
                    payload.get("Plaintext").is_none() && payload.get("CiphertextBlob").is_none(),
                    "case {:?}: a refusal must carry no data key, got {payload}",
                    row.case
                );
                // Nothing was generated, so nothing used the key the request named: it
                // still holds no usage record, and `GetKeyLastUsage` still reports the
                // empty one.
                let after = ok(
                    &router,
                    "GetKeyLastUsage",
                    &json!({"KeyId": refused_key_id}),
                )
                .await;
                assert_eq!(
                    after, fresh,
                    "case {:?}: a refused request must not record a use of the key, got \
                     {after} after {fresh}",
                    row.case
                );
            }
        }
    }
    assert!(
        served > 0,
        "case table: the table must include requests KMS serves, so the refusals are not \
         the only rows"
    );
}
