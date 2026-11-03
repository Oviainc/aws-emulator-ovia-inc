// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GenerateMac` and `VerifyMac`, driven over the wire through
//! the shared [`super::harness`].
//!
//! The rule under test: for an HMAC KMS key in a usable key state whose `KeyUsage` is
//! `GENERATE_VERIFY_MAC`, `GenerateMac` answers the RFC 2104 HMAC of the caller's
//! message under THAT key's own persisted key material with the requested MAC
//! algorithm — a tag as long as the hash the algorithm names, beside the key ARN and
//! that algorithm — and `VerifyMac` recomputes the same HMAC and accepts exactly the
//! tag the same key, algorithm and message produce, refusing every other tag with the
//! modelled `KMSInvalidMacException`; a request no such HMAC can be computed for is
//! refused with the modelled error for why, and MACing a key changes nothing a later
//! caller sees except the key's own last-usage record.
//!
//! Three suites assert it over TABLES rather than one call:
//!
//! 1. every HMAC `KeySpec`, under the MAC algorithm that spec supports, over messages
//!    spanning the modelled message bound — each tag round-tripped through `VerifyMac`
//!    and then tampered with, in the message and in the tag;
//! 2. the refusals, over every kind of key, identifier, algorithm, key state, grant
//!    token and dry run that cannot yield or verify a MAC;
//! 3. the state: one key MACs one message identically however often it is asked, two
//!    keys of a spec MAC it differently and neither tag verifies under the other, every
//!    form of identifier for one key yields the same tag, and the call leaves the key's
//!    metadata exactly as it was.

use std::collections::BTreeSet;

use aws_lc_rs::digest;
use serde_json::{json, Value};

use super::harness::{
    alias_arn, b64, call, contains, create_alias, create_key, describe, error_code, ok, refused,
    router, text, tracked_last_usage, unb64, REGION,
};

/// The longest MAC the model lets `GenerateMac` answer: `Mac` is a `CiphertextType` of
/// "len 1..6144" (`smithy_query(query="operation",operation="GenerateMac")`).
const MAX_MAC_LEN: usize = 6144;

/// One row of the MAC table: an HMAC key spec, a MAC algorithm that spec supports and
/// how long the message is.
struct MacCase {
    key_spec: &'static str,
    mac_algorithm: &'static str,
    /// How many bytes the MACed message is. `Message` is "len 1..4096"
    /// (`smithy_query(query="operation",operation="GenerateMac")`), so the rows span
    /// that bound: its smallest value, middling ones and its largest.
    message_len: usize,
}

/// Every HMAC `KeySpec`, each with the MAC algorithm its "Key spec reference" gives it
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`, and the
/// per-spec list `DescribeKey` reports under `MacAlgorithms`): an `HMAC_<bits>` key
/// supports the one `HMAC_SHA_<bits>` algorithm over the same hash.
const MAC_CASES: &[MacCase] = &[
    MacCase {
        key_spec: "HMAC_224",
        mac_algorithm: "HMAC_SHA_224",
        message_len: 1,
    },
    MacCase {
        key_spec: "HMAC_224",
        mac_algorithm: "HMAC_SHA_224",
        message_len: 4096,
    },
    MacCase {
        key_spec: "HMAC_256",
        mac_algorithm: "HMAC_SHA_256",
        message_len: 1,
    },
    MacCase {
        key_spec: "HMAC_256",
        mac_algorithm: "HMAC_SHA_256",
        message_len: 64,
    },
    MacCase {
        key_spec: "HMAC_256",
        mac_algorithm: "HMAC_SHA_256",
        message_len: 4096,
    },
    MacCase {
        key_spec: "HMAC_384",
        mac_algorithm: "HMAC_SHA_384",
        message_len: 17,
    },
    MacCase {
        key_spec: "HMAC_384",
        mac_algorithm: "HMAC_SHA_384",
        message_len: 4096,
    },
    MacCase {
        key_spec: "HMAC_512",
        mac_algorithm: "HMAC_SHA_512",
        message_len: 1,
    },
    MacCase {
        key_spec: "HMAC_512",
        mac_algorithm: "HMAC_SHA_512",
        message_len: 300,
    },
    MacCase {
        key_spec: "HMAC_512",
        mac_algorithm: "HMAC_SHA_512",
        message_len: 4096,
    },
];

/// The hash a `MacAlgorithmSpec` is the HMAC of, which its own name gives — the four
/// values are `HMAC_SHA_224`, `HMAC_SHA_256`, `HMAC_SHA_384` and `HMAC_SHA_512`
/// (`smithy_query(query="shape",shape="MacAlgorithmSpec")`). An HMAC tag is exactly as
/// long as that hash's output (RFC 2104), which is what a `Mac` member must carry.
fn mac_hash(case: &str, mac_algorithm: &str) -> &'static digest::Algorithm {
    match mac_algorithm {
        "HMAC_SHA_224" => &digest::SHA224,
        "HMAC_SHA_256" => &digest::SHA256,
        "HMAC_SHA_384" => &digest::SHA384,
        "HMAC_SHA_512" => &digest::SHA512,
        other => panic!("case {case}: {other} is not a MAC algorithm this table covers"),
    }
}

/// The message a row MACs: `len` bytes that are neither constant nor the message of any
/// other length, so no tag can be right by accident of its input.
fn message_of(len: usize) -> Vec<u8> {
    (0..len)
        .map(|index| u8::try_from((index * 37 + len) % 251).unwrap_or_default())
        .collect()
}

/// A `GenerateMac` request over `message`.
fn mac_request(key_id: &str, mac_algorithm: &str, message: &[u8]) -> Value {
    json!({
        "KeyId": key_id,
        "Message": b64(message),
        "MacAlgorithm": mac_algorithm
    })
}

/// A `VerifyMac` request over `message` and `mac`.
fn verify_request(key_id: &str, mac_algorithm: &str, message: &[u8], mac: &[u8]) -> Value {
    json!({
        "KeyId": key_id,
        "Message": b64(message),
        "MacAlgorithm": mac_algorithm,
        "Mac": b64(mac)
    })
}

/// `bytes` with the byte at `index` changed — the smallest tampering there is.
fn flip(bytes: &[u8], index: usize) -> Vec<u8> {
    let mut tampered = bytes.to_vec();
    tampered[index] ^= 0x01;
    tampered
}

/// Create an HMAC key of `key_spec` and report its key id and key ARN.
async fn hmac_key(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key_spec: &str,
) -> (String, String) {
    let metadata = create_key(
        router,
        &json!({
            "Description": case,
            "KeySpec": key_spec,
            "KeyUsage": "GENERATE_VERIFY_MAC"
        }),
    )
    .await;
    (
        text(case, &metadata, "KeyId"),
        text(case, &metadata, "Arn"),
    )
}

/// The rule: every HMAC key spec MACs under the algorithm it supports, the tag
/// round-trips through `VerifyMac`, and nothing else verifies.
#[tokio::test]
async fn macs_every_algorithm_each_hmac_key_spec_supports() {
    let router = router();

    // One key per spec, created once: every row of that spec MACs under the same
    // persisted key material, which is what makes the rows about the message and the
    // algorithm rather than about the key.
    let mut keys: Vec<(&str, String, String)> = Vec::new();
    for case in MAC_CASES {
        if keys.iter().any(|(key_spec, ..)| *key_spec == case.key_spec) {
            continue;
        }
        let (key_id, key_arn) = hmac_key(&router, case.key_spec, case.key_spec).await;
        keys.push((case.key_spec, key_id, key_arn));
    }

    for case in MAC_CASES {
        let label = format!(
            "{} {} {} bytes",
            case.key_spec, case.mac_algorithm, case.message_len
        );
        let (_, key_id, key_arn) = keys
            .iter()
            .find(|(key_spec, ..)| *key_spec == case.key_spec)
            .unwrap_or_else(|| panic!("case {label}: the key of this spec was created"));

        let message = message_of(case.message_len);
        let request = mac_request(key_id, case.mac_algorithm, &message);
        let answered = ok(&router, "GenerateMac", &request).await;

        // The answer names the key by its key ARN — "the HMAC KMS key used in the
        // operation" — and the algorithm that produced the tag.
        let answered_key = text(&label, &answered, "KeyId");
        assert_eq!(
            &answered_key, key_arn,
            "case {label}: expected the key ARN {key_arn}, got {answered_key}"
        );
        assert_eq!(
            text(&label, &answered, "MacAlgorithm"),
            case.mac_algorithm,
            "case {label}: expected the MAC algorithm the request named, got {answered}"
        );

        // "This is the standard, raw HMAC defined in RFC 2104", so the tag is exactly
        // as long as the hash the algorithm names, and within the modelled bound.
        let mac = unb64(&label, &answered, "Mac");
        let expected_len = mac_hash(&label, case.mac_algorithm).output_len();
        assert_eq!(
            mac.len(),
            expected_len,
            "case {label}: expected a {expected_len}-byte HMAC, got {} bytes",
            mac.len()
        );
        assert!(
            !mac.is_empty() && mac.len() <= MAX_MAC_LEN,
            "case {label}: expected 1..={MAX_MAC_LEN} MAC bytes, got {}",
            mac.len()
        );
        // A tag is not a copy of what it authenticates: a message long enough not to
        // occur in it by chance must not appear in it at all.
        if case.message_len >= 16 {
            assert!(
                !contains(&mac, &message),
                "case {label}: the MAC must not carry the message it authenticates"
            );
        }

        // The HMAC is a function of the key, the algorithm and the message alone, so
        // asking again gives the same tag.
        let again = unb64(&label, &ok(&router, "GenerateMac", &request).await, "Mac");
        assert_eq!(
            mac, again,
            "case {label}: the same key, algorithm and message must give the same HMAC"
        );

        // The whole claim: the tag verifies under the same key and algorithm over the
        // same message.
        let verified = ok(
            &router,
            "VerifyMac",
            &verify_request(key_id, case.mac_algorithm, &message, &mac),
        )
        .await;
        assert_eq!(
            verified["MacValid"],
            json!(true),
            "case {label}: the tag must verify, got {verified}"
        );
        assert_eq!(
            text(&label, &verified, "KeyId"),
            *key_arn,
            "case {label}: the verification must name the key ARN, got {verified}"
        );
        assert_eq!(
            text(&label, &verified, "MacAlgorithm"),
            case.mac_algorithm,
            "case {label}: the verification must name the algorithm it used, got {verified}"
        );

        // "HMAC verification fails when the HMAC computed by using the specified
        // message, HMAC KMS key, and MAC algorithm does not match the HMAC specified in
        // the request": one byte of either the tag or the message is enough.
        let tampered: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
            ("a tampered tag", message.clone(), flip(&mac, mac.len() - 1)),
            ("a tampered message", flip(&message, 0), mac.clone()),
            (
                "a truncated tag",
                message.clone(),
                mac[..mac.len() - 1].to_vec(),
            ),
        ];
        for (what, tampered_message, tampered_mac) in tampered {
            let tamper_case = format!("{label}: {what}");
            let (status, payload) = call(
                &router,
                REGION,
                "VerifyMac",
                &verify_request(
                    key_id,
                    case.mac_algorithm,
                    &tampered_message,
                    &tampered_mac,
                ),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {tamper_case}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                "KMSInvalidMacException",
                "case {tamper_case}: expected KMSInvalidMacException, got {payload}"
            );
        }
    }
}

/// One refusal row: what makes a MAC impossible, and what KMS answers.
struct RefusalCase {
    case: &'static str,
    /// The `KeySpec` and `KeyUsage` of the key this row MACs with, or `None` for a row
    /// whose `KeyId` names no key at all.
    key: Option<(&'static str, &'static str)>,
    /// An operation called on that key before the MAC, which is what puts it in the key
    /// state the row is about.
    before: Option<&'static str>,
    /// The `KeyId` the request names, when it is not the row's own key.
    names: Option<&'static str>,
    mac_algorithm: &'static str,
    /// A grant token the request presents, for the row that is about one.
    grant_token: Option<&'static str>,
    dry_run: bool,
    /// The wire error code (`__type`) the answer carries. This service answers every
    /// client error with HTTP 400 and names the shape in `__type`.
    expected: &'static str,
}

/// Every reason a `GenerateMac` or `VerifyMac` request yields no MAC.
///
/// The usage and algorithm rows are the two halves of `InvalidKeyUsageException`: "the
/// KeyUsage value of the KMS key is incompatible with the API operation" and "the
/// encryption algorithm or signing algorithm specified for the operation is
/// incompatible with the type of key material in the KMS key (KeySpec)"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`) — for these two
/// operations, "for generating and verifying message authentication codes (MACs), the
/// KeyUsage must be GENERATE_VERIFY_MAC" and the algorithm "must be compatible with the
/// HMAC KMS key that you specify"
/// (`smithy_query(query="doc",shape="GenerateMacRequest",member="MacAlgorithm")`). The
/// key-state rows are "the KMS key that you use for this operation must be in a
/// compatible key state" (`smithy_query(query="doc",shape="GenerateMac")`).
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a symmetric encryption key",
        key: Some(("SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "an asymmetric signing key",
        key: Some(("RSA_2048", "SIGN_VERIFY")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "a key-agreement key",
        key: Some(("ECC_NIST_P256", "KEY_AGREEMENT")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "a longer hash than the HMAC key's spec supports",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_512",
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "a shorter hash than the HMAC key's spec supports",
        key: Some(("HMAC_512", "GENERATE_VERIFY_MAC")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_224",
        grant_token: None,
        dry_run: false,
        expected: "InvalidKeyUsageException",
    },
    RefusalCase {
        case: "an identifier no key answers to",
        key: None,
        before: None,
        names: Some("00000000-0000-4000-8000-000000000000"),
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "NotFoundException",
    },
    RefusalCase {
        case: "an alias that points at nothing",
        key: None,
        before: None,
        names: Some("alias/no-such-hmac-key"),
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "NotFoundException",
    },
    RefusalCase {
        case: "a disabled key",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: Some("DisableKey"),
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "DisabledException",
    },
    RefusalCase {
        case: "a key awaiting deletion",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: Some("ScheduleKeyDeletion"),
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: false,
        expected: "KMSInvalidStateException",
    },
    RefusalCase {
        case: "a grant token this service never minted",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: Some("not-a-grant-token"),
        dry_run: false,
        expected: "InvalidGrantTokenException",
    },
    RefusalCase {
        case: "a dry run",
        key: Some(("HMAC_256", "GENERATE_VERIFY_MAC")),
        before: None,
        names: None,
        mac_algorithm: "HMAC_SHA_256",
        grant_token: None,
        dry_run: true,
        expected: "DryRunOperationException",
    },
];

/// The rule, continued: a request no MAC can be computed or verified for is refused
/// with the modelled error for why, by BOTH operations, and nothing is MACed.
#[tokio::test]
async fn refuses_every_request_that_cannot_be_maced() {
    let router = router();

    // Every row names a distinct reason, so no reason is asserted twice and none is a
    // copy of another.
    let reasons: BTreeSet<&str> = REFUSAL_CASES.iter().map(|case| case.case).collect();
    assert_eq!(
        reasons.len(),
        REFUSAL_CASES.len(),
        "case table: every refusal row must name its own reason"
    );

    let message = message_of(48);
    // A tag of the length `HMAC_SHA_256` produces, for the `VerifyMac` rows: every
    // refusal here is answered BEFORE any comparison, so what the bytes are cannot
    // decide the answer.
    let mac = message_of(32);

    for case in REFUSAL_CASES {
        for operation in ["GenerateMac", "VerifyMac"] {
            let label = format!("{operation}: {}", case.case);
            let key_id = match case.key {
                Some((key_spec, key_usage)) => {
                    let metadata = create_key(
                        &router,
                        &json!({
                            "Description": label,
                            "KeySpec": key_spec,
                            "KeyUsage": key_usage
                        }),
                    )
                    .await;
                    let key_id = text(&label, &metadata, "KeyId");
                    if let Some(before) = case.before {
                        ok(&router, before, &json!({"KeyId": key_id})).await;
                    }
                    key_id
                }
                None => String::new(),
            };
            let named = case.names.unwrap_or(&key_id);

            let mut request = if operation == "GenerateMac" {
                mac_request(named, case.mac_algorithm, &message)
            } else {
                verify_request(named, case.mac_algorithm, &message, &mac)
            };
            if let Some(token) = case.grant_token {
                request["GrantTokens"] = json!([token]);
            }
            if case.dry_run {
                request["DryRun"] = json!(true);
            }

            let (status, payload) = call(&router, REGION, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {label}: expected HTTP 400, got {status} with {payload}"
            );
            assert_eq!(
                error_code(&payload),
                case.expected,
                "case {label}: expected {}, got {payload}",
                case.expected
            );
            assert!(
                payload["Mac"].is_null() && payload["MacValid"].is_null(),
                "case {label}: a refused request MACs nothing, got {payload}"
            );
        }
    }
}

/// The rule, continued: the refusal of a key whose `KeyUsage` does no MACing names
/// that key, its usage and the operation; the refusal of a MAC algorithm the key's own
/// key spec does not support names the algorithm and that key spec; and the refusal of
/// a tag that does not verify carries no message at all.
#[tokio::test]
async fn a_refusal_words_what_it_refuses() {
    let router = router();
    let message = message_of(24);

    // Rows: each HMAC key spec asked for an algorithm one of the OTHER specs supports,
    // so the wording varies in both the algorithm and the key spec.
    let mismatches: &[(&str, &str)] = &[
        ("HMAC_224", "HMAC_SHA_512"),
        ("HMAC_256", "HMAC_SHA_384"),
        ("HMAC_384", "HMAC_SHA_224"),
        ("HMAC_512", "HMAC_SHA_256"),
    ];
    for (key_spec, mac_algorithm) in mismatches {
        let (key_id, _) = hmac_key(&router, key_spec, key_spec).await;
        let expected =
            format!("Algorithm {mac_algorithm} is incompatible with key spec {key_spec}.");
        for operation in ["GenerateMac", "VerifyMac"] {
            let label = format!("{operation}: {key_spec} asked for {mac_algorithm}");
            let request = if operation == "GenerateMac" {
                mac_request(&key_id, mac_algorithm, &message)
            } else {
                verify_request(&key_id, mac_algorithm, &message, &message_of(32))
            };
            let (status, code, worded) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {label}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "InvalidKeyUsageException",
                "case {label}: expected InvalidKeyUsageException, got {code}"
            );
            assert_eq!(
                worded, expected,
                "case {label}: expected {expected:?}, got {worded:?}"
            );
        }
    }

    // Rows: every other `KeyUsage` a KMS key can carry, none of which MACs at all, so
    // the wording varies in the key, its usage and the operation that refused it.
    let other_usages: &[(&str, &str)] = &[
        ("SYMMETRIC_DEFAULT", "ENCRYPT_DECRYPT"),
        ("RSA_2048", "SIGN_VERIFY"),
        ("ECC_NIST_P256", "KEY_AGREEMENT"),
    ];
    for (key_spec, key_usage) in other_usages {
        let metadata = create_key(
            &router,
            &json!({
                "Description": key_spec,
                "KeySpec": key_spec,
                "KeyUsage": key_usage
            }),
        )
        .await;
        let key_id = text(key_spec, &metadata, "KeyId");
        let key_arn = text(key_spec, &metadata, "Arn");
        for operation in ["GenerateMac", "VerifyMac"] {
            let label = format!("{operation}: a {key_usage} key");
            let request = if operation == "GenerateMac" {
                mac_request(&key_id, "HMAC_SHA_256", &message)
            } else {
                verify_request(&key_id, "HMAC_SHA_256", &message, &message_of(32))
            };
            let expected =
                format!("{key_arn} key usage is {key_usage} which is not valid for {operation}.");
            let (status, code, worded) = refused(&router, REGION, operation, &request).await;
            assert_eq!(
                status, 400,
                "case {label}: expected HTTP 400, got {status} ({code})"
            );
            assert_eq!(
                code, "InvalidKeyUsageException",
                "case {label}: expected InvalidKeyUsageException, got {code}"
            );
            assert_eq!(
                worded, expected,
                "case {label}: expected {expected:?}, got {worded:?}"
            );
        }
    }

    // A tag that does not verify: the caller is told the verification failed and
    // nothing about which of its three inputs was not the one the tag came from.
    let (key_id, _) = hmac_key(&router, "tamper wording", "HMAC_256").await;
    let mac = unb64(
        "tamper wording",
        &ok(
            &router,
            "GenerateMac",
            &mac_request(&key_id, "HMAC_SHA_256", &message),
        )
        .await,
        "Mac",
    );
    let (status, code, worded) = refused(
        &router,
        REGION,
        "VerifyMac",
        &verify_request(&key_id, "HMAC_SHA_256", &message, &flip(&mac, 0)),
    )
    .await;
    assert_eq!(
        status, 400,
        "case tamper wording: expected HTTP 400, got {status} ({code})"
    );
    assert_eq!(
        code, "KMSInvalidMacException",
        "case tamper wording: expected KMSInvalidMacException, got {code}"
    );
    assert!(
        worded.is_empty(),
        "case tamper wording: the refusal must carry no message, got {worded:?}"
    );
}

/// The rule, continued: the MAC comes from the key's own PERSISTED material, so two
/// keys of one spec MAC one message differently and neither tag verifies under the
/// other; every form of identifier for one key yields the same tag; and the call leaves
/// everything else a later caller sees as it was.
#[tokio::test]
async fn a_mac_comes_from_the_persisted_key_material() {
    let router = router();
    let message = message_of(96);
    let other_message = message_of(97);
    let algorithm = "HMAC_SHA_256";

    let mut macs: Vec<(String, String, Vec<u8>)> = Vec::new();
    for index in 0..2u8 {
        let case = format!("key {index}");
        let (key_id, key_arn) = hmac_key(&router, &case, "HMAC_256").await;
        let before = describe(&router, &key_id).await;

        let request = mac_request(&key_id, algorithm, &message);
        let mac = unb64(&case, &ok(&router, "GenerateMac", &request).await, "Mac");

        // A different message under the same key is a different tag.
        let other = unb64(
            &case,
            &ok(
                &router,
                "GenerateMac",
                &mac_request(&key_id, algorithm, &other_message),
            )
            .await,
            "Mac",
        );
        assert_ne!(
            mac, other,
            "case {case}: two messages must not have the same HMAC under one key"
        );

        // MACing is a USE of the key: it leaves the key's own metadata as it was.
        let after = describe(&router, &key_id).await;
        assert_eq!(
            after, before,
            "case {case}: generating a MAC must not change the key's metadata"
        );

        // The use is recorded, and KMS's usage tracking produces that record a little
        // after the call: once it has, the record names this operation.
        let usage =
            tracked_last_usage(&router, &case, &key_id, "GenerateMac", &Value::Null).await;
        assert_eq!(
            usage["KeyLastUsage"]["Operation"]
                .as_str()
                .unwrap_or_default(),
            "GenerateMac",
            "case {case}: the key's last usage must name GenerateMac, got {usage}"
        );

        // Verifying is a use of the key too, and it is the later of the two.
        ok(
            &router,
            "VerifyMac",
            &verify_request(&key_id, algorithm, &message, &mac),
        )
        .await;
        let after_verify =
            tracked_last_usage(&router, &case, &key_id, "VerifyMac", &usage).await;
        assert_eq!(
            after_verify["KeyLastUsage"]["Operation"]
                .as_str()
                .unwrap_or_default(),
            "VerifyMac",
            "case {case}: the key's last usage must name VerifyMac, got {after_verify}"
        );

        macs.push((key_id, key_arn, mac));
    }

    let (first_id, _, first_mac) = &macs[0];
    let (second_id, _, second_mac) = &macs[1];
    assert_ne!(
        first_mac, second_mac,
        "two keys must not have the same HMAC over one message"
    );
    // A tag from one HMAC key does not verify under another: the key is part of what
    // the HMAC is computed from.
    for (case, key_id, other_mac) in [
        ("the first key's tag under the second key", second_id, first_mac),
        ("the second key's tag under the first key", first_id, second_mac),
    ] {
        let (status, payload) = call(
            &router,
            REGION,
            "VerifyMac",
            &verify_request(key_id, algorithm, &message, other_mac),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "KMSInvalidMacException",
            "case {case}: expected KMSInvalidMacException, got {payload}"
        );
    }

    // Every form of identifier for ONE key names the same key material, so each yields
    // the tag the key id did and reports that key's ARN; and the tag one form produced
    // verifies under another.
    let (key_id, key_arn, mac) = &macs[0];
    create_alias(&router, "identifier forms", "alias/mac-key", key_id).await;
    for identifier in [
        key_id.clone(),
        key_arn.clone(),
        "alias/mac-key".to_string(),
        alias_arn(REGION, "alias/mac-key"),
    ] {
        let case = format!("identifier {identifier}");
        let answered = ok(
            &router,
            "GenerateMac",
            &mac_request(&identifier, algorithm, &message),
        )
        .await;
        assert_eq!(
            &unb64(&case, &answered, "Mac"),
            mac,
            "case {case}: every identifier of one key must give that key's HMAC"
        );
        assert_eq!(
            text(&case, &answered, "KeyId"),
            *key_arn,
            "case {case}: the answer must name the key ARN, got {answered}"
        );

        let verified = ok(
            &router,
            "VerifyMac",
            &verify_request(&identifier, algorithm, &message, mac),
        )
        .await;
        assert_eq!(
            verified["MacValid"],
            json!(true),
            "case {case}: the tag must verify through every identifier, got {verified}"
        );
    }
}
