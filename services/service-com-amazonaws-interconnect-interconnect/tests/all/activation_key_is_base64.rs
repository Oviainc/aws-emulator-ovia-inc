// SPDX-License-Identifier: CC0-1.0

//! The `activationKey` member's OWN well-formedness, driven over the wire through the
//! shared [`crate::emulator`] harness.
//!
//! RULE: an `activationKey` is the partner portal's base64-encoded capture of what a
//! creation request chose, so every operation that takes the member judges the value's
//! ENCODING on the request alone and BEFORE it resolves the value against this store:
//! text that is not base64 is refused as invalid input (the modelled
//! `InterconnectValidationException`, HTTP 400, in one fixed sentence that reproduces
//! nothing of the `@sensitive` value), while base64 text that simply names no proposal
//! keeps the resource verdict (the modelled `ResourceNotFoundException`, HTTP 404). The
//! two operations that carry the member — the probe a client runs first
//! (`DescribeConnectionProposal`) and the acceptance that follows it
//! (`AcceptConnectionProposal`) — answer one and the same text identically, and neither
//! refusal writes anything.
//!
//! The rows differ along what the rule generalises over: WHICH WAY the text fails to be
//! base64 (a character outside the standard alphabet, a base64url-only character,
//! whitespace, padding inside the text, too much padding, a final group of a single
//! character) and at what length it does so, including a key of a single character (the
//! published `ActivationKey` minimum, `@length(min: 1, max: 2048)`), one of exactly the
//! published maximum and one character past it; and, on the well-formed side, the
//! encoding's own three padding forms, swept over every payload length from one byte to
//! twenty, padded and unpadded, plus the full alphabet's `+` and `/`, the shortest text
//! the encoding admits, the published maximum length and a key a `CreateConnection`
//! really minted.

use base64::Engine as _;
use serde_json::{json, Value};

use service_com_amazonaws_interconnect_interconnect::state::{
    activation_key_not_base64_message, proposal_not_retrievable_message,
};

use crate::emulator::{code, message, placements, text, Emulator, DEFAULT_SIGNING_REGION};

/// The error this service models for a fault in the request's own input (HTTP 400).
const VALIDATION_CODE: &str = "InterconnectValidationException";

/// The error a well-formed handle that names no resource is answered with (HTTP 404).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";

/// The published bounds of `ActivationKey`
/// (`smithy_query(query="shape",shape="ActivationKey")` -> length 1..2048), which is
/// the whole of what the member may carry whatever its text encodes.
const ACTIVATION_KEY_LENGTH: (usize, usize) = (1, 2048);

/// How many payload lengths the well-formed sweep covers: twenty bytes walks every
/// residue of three, so every one of base64's padding forms (`""`, `"="`, `"=="`)
/// appears several times over.
const SWEPT_PAYLOAD_BYTES: usize = 20;

// ---------------------------------------------------------------------------
// Driving the two operations that take the member
// ---------------------------------------------------------------------------

/// `DescribeConnectionProposal` for one key, as a `(status, body)` pair.
async fn describe(emulator: &Emulator, activation_key: &str) -> (u16, Value) {
    emulator
        .ic(
            "DescribeConnectionProposal",
            json!({ "activationKey": activation_key }),
        )
        .await
}

/// `AcceptConnectionProposal` for one key, with an attach point every member
/// constraint admits so nothing but the key can be what a refusal is about.
async fn accept(emulator: &Emulator, activation_key: &str, gateway: &str) -> (u16, Value) {
    emulator
        .ic(
            "AcceptConnectionProposal",
            json!({
                "activationKey": activation_key,
                "attachPoint": { "directConnectGateway": gateway },
            }),
        )
        .await
}

/// Both operations' answers for one key: `(operation, status, body)` each, so a case is
/// asserted on the pair rather than on one of them.
async fn both(
    emulator: &Emulator,
    activation_key: &str,
    gateway: &str,
) -> Vec<(&'static str, u16, Value)> {
    let (describe_status, describe_body) = describe(emulator, activation_key).await;
    let (accept_status, accept_body) = accept(emulator, activation_key, gateway).await;
    vec![
        ("DescribeConnectionProposal", describe_status, describe_body),
        ("AcceptConnectionProposal", accept_status, accept_body),
    ]
}

/// Assert both operations refused `activation_key` as a fault in the REQUEST.
async fn assert_refused_as_input(
    emulator: &Emulator,
    case: &str,
    activation_key: &str,
    gateway: &str,
) {
    for (operation, status, body) in both(emulator, activation_key, gateway).await {
        let case = format!("{case} / {operation}");
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for text that is not base64, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(VALIDATION_CODE),
            "case {case:?}: expected {VALIDATION_CODE}, got {:?} with {body:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            activation_key_not_base64_message(),
            "case {case:?}: expected the encoding refusal's fixed wording, got {:?}",
            message(&body)
        );
        assert!(
            !message(&body).contains(activation_key),
            "case {case:?}: expected the @sensitive activationKey not to be echoed, got {:?}",
            message(&body)
        );
    }
}

/// Assert both operations answered `activation_key` with the resource verdict: the text
/// is base64, so it could name a proposal — this store just holds none for it.
async fn assert_resolves_to_nothing(
    emulator: &Emulator,
    case: &str,
    activation_key: &str,
    gateway: &str,
) {
    for (operation, status, body) in both(emulator, activation_key, gateway).await {
        let case = format!("{case} / {operation}");
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for base64 text that names no proposal, got {status} with {body:?}"
        );
        assert!(
            code(&body).contains(NOT_FOUND_CODE),
            "case {case:?}: expected {NOT_FOUND_CODE}, got {:?} with {body:?}",
            code(&body)
        );
        assert_eq!(
            message(&body),
            proposal_not_retrievable_message(),
            "case {case:?}: expected the store's own not-found wording, got {:?}",
            message(&body)
        );
        assert!(
            !message(&body).contains(activation_key),
            "case {case:?}: expected the @sensitive activationKey not to be echoed, got {:?}",
            message(&body)
        );
    }
}

/// Every stored connection of the default signing Region, as `ListConnections` reports
/// it — what a refusal must leave exactly as it was.
async fn connections(emulator: &Emulator) -> Vec<Value> {
    let listed = emulator
        .ic_ok_in(DEFAULT_SIGNING_REGION, "ListConnections", json!({}))
        .await;
    listed["connections"]
        .as_array()
        .expect("connections is a list")
        .clone()
}

// ---------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------

/// Text that is NOT base64, each row breaking the encoding a different way.
///
/// `maximum` is the published `ActivationKey` maximum, so the last row carries a
/// malformed key of exactly the longest length the member admits; the single-character
/// row carries the shortest length it admits, which cannot be base64 at all.
fn malformed_keys(maximum: usize) -> Vec<(String, String)> {
    let rows: Vec<(&str, String)> = vec![
        (
            "a character outside the standard alphabet",
            "not!valid!base64!".to_string(),
        ),
        (
            "hyphens, which only the base64url alphabet carries",
            "an-activation-key-no-portal-ever-issued".to_string(),
        ),
        (
            "an underscore, which only the base64url alphabet carries",
            "QUJDREVG_w".to_string(),
        ),
        ("whitespace inside the text", "QUJD QUJD".to_string()),
        ("padding inside the text", "QU=JDREVG".to_string()),
        ("more padding than a group can carry", "QQ===".to_string()),
        (
            "a final group of a single character",
            "QUJDREVGR".to_string(),
        ),
        (
            "a single character, the published minimum length",
            "A".to_string(),
        ),
        ("nothing but padding", "====".to_string()),
        (
            "a malformed key of the maximum admitted length",
            format!("{}!", "A".repeat(maximum - 1)),
        ),
    ];
    rows.into_iter()
        .map(|(name, key)| (name.to_string(), key))
        .collect()
}

/// Base64 text that names no proposal of this store.
///
/// The sweep is the encoding's own three padding forms over every payload length from
/// one byte to [`SWEPT_PAYLOAD_BYTES`], written both with and without `=` padding —
/// neither form is what makes a key resolve, so both get the resource verdict. The
/// named rows add the shortest text the encoding admits, the published maximum length
/// and the two alphabet characters a hex-shaped key never shows (`+`, `/`).
fn well_formed_keys(minted: &str) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();

    for bytes in 1..=SWEPT_PAYLOAD_BYTES {
        let payload: Vec<u8> = (0..bytes).map(|index| (index as u8).wrapping_mul(37)).collect();
        rows.push((
            format!("the base64 of {bytes} bytes, padded"),
            base64::engine::general_purpose::STANDARD.encode(&payload),
        ));
        rows.push((
            format!("the base64 of {bytes} bytes, unpadded"),
            base64::engine::general_purpose::STANDARD_NO_PAD.encode(&payload),
        ));
    }

    // 0xFB_FF_BF encodes to `+/+/`: the two alphabet characters a hex-shaped key never
    // carries, so the whole standard alphabet is admitted and not just its alphanumerics.
    let punctuation = base64::engine::general_purpose::STANDARD_NO_PAD.encode([0xFB, 0xFF, 0xBF]);
    assert!(
        punctuation.contains('+') && punctuation.contains('/'),
        "case \"alphabet\": expected the payload to encode with + and /, got {punctuation:?}"
    );
    rows.push(("the full standard alphabet".to_string(), punctuation));

    // The longest text the member admits, the shortest the encoding admits, and a key
    // that differs from a real one only in case — base64 is case-SIGNIFICANT, so it
    // names another payload entirely.
    let (_, maximum) = ACTIVATION_KEY_LENGTH;
    rows.push((
        "the shortest base64 text the member admits (2 characters)".to_string(),
        "AA".to_string(),
    ));
    rows.push((
        format!("base64 of the maximum admitted length ({maximum})"),
        "A".repeat(maximum),
    ));
    rows.push((
        "a minted key with its case flipped".to_string(),
        minted.to_ascii_lowercase(),
    ));
    rows.push((
        "a minted key one character short".to_string(),
        minted[..minted.len() - 1].to_string(),
    ));

    rows.retain(|(_, key)| key != minted);
    rows
}

// ---------------------------------------------------------------------------
// The suite
// ---------------------------------------------------------------------------

/// RULE: both operations that take an `activationKey` judge the member's own base64
/// encoding before they resolve it, so text that is not base64 is the modelled input
/// refusal (400) in one fixed sentence and base64 text that names no proposal is the
/// modelled resource verdict (404) — and a key a `CreateConnection` minted is read
/// rather than refused.
#[tokio::test]
async fn an_activation_keys_encoding_is_judged_before_the_proposal_is_resolved() {
    let emulator = Emulator::new();
    let gateway = emulator.attach_point().await;
    let (environment_id, bandwidth) = placements(&emulator)
        .await
        .into_iter()
        .next()
        .expect("the catalog offers a placement");

    // One real proposal, so what is judged below is the key and not a store holding no
    // proposal at all.
    let connection = emulator
        .create_connection(
            &environment_id,
            &bandwidth,
            &gateway,
            "activation key encoding",
        )
        .await;
    let minted = text(&connection, "activationKey");
    let (minimum, maximum) = ACTIVATION_KEY_LENGTH;
    assert!(
        (minimum..=maximum).contains(&minted.len()),
        "case \"minted key\": expected a length inside the modelled bounds, got {}",
        minted.len()
    );

    // A minted key IS base64, so the rule never refuses this service's own keys: it
    // decodes, and the proposal it names describes.
    assert!(
        base64::engine::general_purpose::STANDARD
            .decode(minted.as_bytes())
            .is_ok(),
        "case \"minted key\": expected the minted activationKey to be base64, got {minted:?}"
    );
    let (status, described) = describe(&emulator, &minted).await;
    assert_eq!(
        status, 200,
        "case \"minted key\": expected the minted key to describe its proposal, got {status} with {described:?}"
    );
    assert_eq!(
        text(&described, "environmentId"),
        environment_id,
        "case \"minted key\": expected the proposal's own environmentId, got {:?}",
        described["environmentId"]
    );

    let before = connections(&emulator).await;

    // Text that is not base64: refused as input, identically by both operations.
    for (name, key) in malformed_keys(maximum) {
        assert_refused_as_input(&emulator, &name, &key, &gateway).await;
    }

    // Base64 text this store holds no proposal for: the resource verdict, unchanged.
    for (name, key) in well_formed_keys(&minted) {
        assert_resolves_to_nothing(&emulator, &name, &key, &gateway).await;
    }

    // A key one character PAST the published maximum is refused at the boundary, before
    // any handler judges its encoding — a 400, and never a 404 or a 501.
    for (name, key) in [
        ("one past the maximum, base64", "A".repeat(maximum + 1)),
        (
            "one past the maximum, not base64",
            format!("{}!", "A".repeat(maximum)),
        ),
    ] {
        for (operation, status, body) in both(&emulator, &key, &gateway).await {
            let case = format!("{name} / {operation}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a key outside the modelled length, got {status} with {body:?}"
            );
        }
    }

    // Not one of those refusals wrote anything: the store holds the same connections,
    // and the proposal they were refused beside is still acceptable.
    let after = connections(&emulator).await;
    assert_eq!(
        after.len(),
        before.len(),
        "case \"refused keys\": expected the refusals to have written nothing, got {} connections for {} before",
        after.len(),
        before.len()
    );
    let (status, accepted) = accept(&emulator, &minted, &gateway).await;
    assert_eq!(
        status, 200,
        "case \"minted key\": expected the minted key to still accept, got {status} with {accepted:?}"
    );
    assert_eq!(
        accepted["connection"]["attachPoint"],
        json!({ "directConnectGateway": gateway.clone() }),
        "case \"minted key\": expected the acceptance to settle the attach point, got {:?}",
        accepted["connection"]["attachPoint"]
    );
}
