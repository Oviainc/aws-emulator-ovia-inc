// SPDX-License-Identifier: CC0-1.0

//! Reading a presented service account token STOPS at its JOSE header: once a value is
//! three non-empty segments, only the first one is decoded and judged, so nothing in the
//! claim set or the signature can change how `AssumeRoleForPodIdentity` answers.
//!
//! The rule has two halves, both pinned here over tables rather than over one request:
//!
//! * the segments past the header are never read, so every token sharing one header gets
//!   ONE (status, code, message) identity whatever its payload holds — a claim set inside
//!   its window, one that expired, one addressed to another audience, an empty claim
//!   object, a payload that is not a JSON object, one that is not even base64url — and no
//!   claim can select a different modelled error (nothing here is ever answered with
//!   `ExpiredTokenException`, `AccessDeniedException` or `ResourceNotFoundException`,
//!   because no `exp`, `aud` or `sub` is read);
//! * the HEADER still discriminates: dropping `kid` from the same header moves the answer
//!   to its own message, so the collapse above is a measurement of the payload layer and
//!   not of a service that answers everything the same way.
//!
//! The rows are driven over the real request path through the harness
//! `tests/all/assume_role_for_pod_identity.rs` owns, and the parse itself is exercised
//! directly over the cross product of payload and signature forms.

use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    encode_jwt_segment, parse_service_account_token, TOKEN_SIGNING_ALGORITHM,
};

use crate::assume_role_for_pod_identity::{Harness, SEEDS};

/// A seeded cluster, whose OIDC provider publishes exactly one key.
const CLUSTER: &str = SEEDS[0].cluster;

/// A cluster name inside the modelled `ClusterName` pattern and length that is NOT
/// registered, so its OIDC provider publishes no key at all and every key id a header
/// names resolves to nothing there.
const UNREGISTERED_CLUSTER: &str = "sim2realbehavclaimscluster";

/// The one message a header naming a key no provider publishes is answered with. Fixed
/// text apart from the header member name.
const UNRESOLVABLE_KID: &str = "The token included in the request has invalid header - kid.";

/// The message a header naming no key at all is answered with — the control identity the
/// header layer must keep producing.
const MISSING_KID: &str = "The token included in the request has missing header - kid.";

/// A segment base64url-encoded WITH the `=` padding RFC 7515 §2 omits — which the
/// modelled `JwtToken` pattern admits and a client may really send.
fn encode_padded_segment(bytes: impl AsRef<[u8]>) -> String {
    aws_smithy_types::base64::encode(bytes.as_ref())
        .replace('+', "-")
        .replace('/', "_")
}

/// A key id in the form a Kubernetes API server derives for every key it signs with: the
/// unpadded base64url of a 32-byte digest, 43 characters. No seeded cluster publishes it.
fn key_id(seed: u8) -> String {
    encode_jwt_segment([seed; 32])
}

/// Seconds since the epoch, for the `iat`/`exp` claims a projected token carries.
fn epoch_seconds(offset: i64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs() as i64;
    now + offset
}

/// A claim set of the shape a projected service account token carries, with the audience
/// and the validity window the row asks for.
fn claim_set(audience: &str, issued_offset: i64, expires_offset: i64) -> String {
    encode_jwt_segment(
        serde_json::json!({
            "aud": [audience],
            "iss": "https://oidc.eks.us-west-2.amazonaws.com/id/0123456789ABCDEF",
            "sub": "system:serviceaccount:default:my-sa",
            "iat": epoch_seconds(issued_offset),
            "exp": epoch_seconds(expires_offset),
        })
        .to_string(),
    )
}

/// Every form a second segment can take, named. The rows differ along what the rule
/// generalises over: the claims' validity window, their audience, the JSON type of the
/// payload, whether it is JSON at all, and whether it is even base64url.
fn payload_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "a claim set inside its validity window",
            claim_set("pods.eks.amazonaws.com", -60, 3600),
        ),
        (
            "the same claim set, expired an hour ago",
            claim_set("pods.eks.amazonaws.com", -7200, -3600),
        ),
        (
            "the same claim set, not valid until an hour from now",
            claim_set("pods.eks.amazonaws.com", 3600, 7200),
        ),
        (
            "a claim set addressed to another audience",
            claim_set("sts.amazonaws.com", -60, 3600),
        ),
        ("an empty claim object", encode_jwt_segment("{}")),
        (
            "a claim set whose exp is not a number",
            encode_jwt_segment(r#"{"exp":"soon","aud":[]}"#),
        ),
        (
            "a payload that is a JSON array",
            encode_jwt_segment(r#"["aud","exp"]"#),
        ),
        (
            "a payload that is a JSON string",
            encode_jwt_segment(r#""claims""#),
        ),
        ("a payload that is a JSON number", encode_jwt_segment("42")),
        (
            "a payload that is not JSON at all",
            encode_jwt_segment([0x8f, 0x01, 0xff, 0x7e, 0x00]),
        ),
        (
            "a payload that is text but not JSON",
            encode_jwt_segment("42rubbish"),
        ),
        // A segment the modelled `JwtToken` pattern admits that is not a whole base64url
        // group, so it decodes to nothing: still not the header, so still not read.
        ("a payload that is not base64url", "A".to_string()),
        (
            // Padding the modelled pattern admits too.
            "a padded payload",
            encode_padded_segment("{}"),
        ),
    ]
}

/// Every form a third segment can take, named: the signature is past the header too.
fn signature_rows() -> Vec<(&'static str, String)> {
    vec![
        (
            "a signature of signature length",
            encode_jwt_segment([7_u8; 32]),
        ),
        ("a one-byte signature", encode_jwt_segment([0_u8; 1])),
        ("a signature that is not base64url", "A".to_string()),
        ("a padded signature", encode_padded_segment([3_u8; 31])),
    ]
}

/// A token whose JOSE header is `header` and whose remaining segments are exactly
/// `payload` and `signature`.
fn token_of(header: &serde_json::Value, payload: &str, signature: &str) -> String {
    format!(
        "{}.{payload}.{signature}",
        encode_jwt_segment(header.to_string())
    )
}

/// What one call answered: the status, the shape `x-amzn-errortype` names and the message.
#[derive(Debug, PartialEq, Eq)]
struct Identity {
    status: u16,
    code: String,
    message: String,
}

/// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`.
async fn identity_of(harness: &Harness, cluster: &str, token: &str) -> Identity {
    let (status, errortype, raw) = harness
        .send(cluster, serde_json::json!({ "token": token }))
        .await;
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    let code = parsed["__type"]
        .as_str()
        .or(errortype.as_deref())
        .unwrap_or("<no __type>");
    let code = code.split(':').next().unwrap_or(code).to_string();
    Identity {
        status,
        code,
        message: parsed["message"].as_str().unwrap_or_default().to_string(),
    }
}

/// THE RULE, over the wire: one header, every payload — one identity. A header naming a
/// key no provider publishes is answered on that key id whatever the payload holds, so
/// the payload cannot be the reason a token was refused.
#[tokio::test]
async fn the_payload_cannot_change_how_a_token_is_refused() {
    let harness = Harness::seeded();
    let header = serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "typ": "JWT",
        "kid": key_id(11),
    });
    let signature = encode_jwt_segment([7_u8; 32]);
    let expected = Identity {
        status: 400,
        code: "InvalidTokenException".to_string(),
        message: UNRESOLVABLE_KID.to_string(),
    };

    // Over a registered cluster and over one that is not: neither publishes this key, so
    // the answer is the same for both, and the payload changes nothing for either. Every
    // row is compared against the SAME expected identity, which is what "the payloads
    // collapse onto one identity" means.
    for cluster in [UNREGISTERED_CLUSTER, CLUSTER, "Prod2"] {
        for (case, payload) in payload_rows() {
            let token = token_of(&header, &payload, &signature);
            let actual = identity_of(&harness, cluster, &token).await;
            assert_eq!(
                actual, expected,
                "case {case:?} against cluster {cluster:?}: expected {expected:?}, got \
                 {actual:?}"
            );
        }
    }
}

/// The same rule for the third segment: the signature is past the header too, so no form
/// of it changes the identity either.
#[tokio::test]
async fn the_signature_segment_cannot_change_how_a_token_is_refused() {
    let harness = Harness::seeded();
    let header = serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": key_id(12) });
    let payload = claim_set("pods.eks.amazonaws.com", -60, 3600);
    let expected = Identity {
        status: 400,
        code: "InvalidTokenException".to_string(),
        message: UNRESOLVABLE_KID.to_string(),
    };

    for (case, signature) in signature_rows() {
        let token = token_of(&header, &payload, &signature);
        let actual = identity_of(&harness, UNREGISTERED_CLUSTER, &token).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
    }
}

/// No claim selects a modelled error: because `exp`, `aud` and `sub` are never read, the
/// expired-token, access-denied and not-found shapes are reachable by no payload.
#[tokio::test]
async fn no_payload_selects_another_modelled_error() {
    let harness = Harness::seeded();
    let header = serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": key_id(13) });
    let signature = encode_jwt_segment([5_u8; 32]);

    for cluster in [UNREGISTERED_CLUSTER, CLUSTER] {
        for (case, payload) in payload_rows() {
            let token = token_of(&header, &payload, &signature);
            let actual = identity_of(&harness, cluster, &token).await;
            assert_eq!(
                actual.code, "InvalidTokenException",
                "case {case:?} against cluster {cluster:?}: no payload may select another \
                 modelled shape, got {actual:?}"
            );
        }
    }
}

/// The CONTROL: the header layer must still discriminate. The same payloads under a
/// header that names NO key answer the missing-header message instead, an identity
/// distinct from the collapsed one — so the collapse above is a property of the payload
/// layer, not of a service that answers everything alike.
#[tokio::test]
async fn the_header_still_discriminates_under_every_payload() {
    let harness = Harness::seeded();
    let signature = encode_jwt_segment([7_u8; 32]);

    // (case, header, the message that header is answered with)
    let headers: &[(&str, serde_json::Value, &str)] = &[
        (
            "no kid at all",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "typ": "JWT" }),
            MISSING_KID,
        ),
        (
            "a blank kid",
            serde_json::json!({ "alg": TOKEN_SIGNING_ALGORITHM, "kid": "  " }),
            MISSING_KID,
        ),
        (
            "another signature algorithm",
            serde_json::json!({ "alg": "HS256", "kid": key_id(14) }),
            "The token included in the request has invalid header - alg.",
        ),
        (
            "a header that is not a JOSE header",
            serde_json::json!({ "typ": "JWT", "kid": key_id(14) }),
            "Failed to parse the token included in the request.",
        ),
    ];

    for (header_case, header, expected) in headers {
        for (payload_case, payload) in payload_rows() {
            let token = token_of(header, &payload, &signature);
            let actual = identity_of(&harness, UNREGISTERED_CLUSTER, &token).await;
            assert_eq!(
                (actual.status, actual.code.as_str(), actual.message.as_str()),
                (400, "InvalidTokenException", *expected),
                "header {header_case:?} with payload {payload_case:?}: expected the \
                 header's own identity {expected:?}, got {actual:?}"
            );
            assert_ne!(
                actual.message, UNRESOLVABLE_KID,
                "header {header_case:?} with payload {payload_case:?}: the header layer \
                 must still discriminate, got {actual:?}"
            );
        }
    }
}

/// The same invariant at the parse itself, over the whole cross product of payload and
/// signature forms: reading a token reports the header it named and nothing else, so the
/// result is byte-identical for every pair of trailing segments.
#[test]
fn the_parse_depends_only_on_the_header_segment() {
    let header = serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "kid": key_id(15),
        "typ": "JWT",
    });
    let reference = parse_service_account_token(&token_of(
        &header,
        &claim_set("pods.eks.amazonaws.com", -60, 3600),
        &encode_jwt_segment([7_u8; 32]),
    ));
    assert!(
        reference.is_ok(),
        "a three-segment token with a readable header parses, got {reference:?}"
    );

    for (payload_case, payload) in payload_rows() {
        for (signature_case, signature) in signature_rows() {
            let parsed = parse_service_account_token(&token_of(&header, &payload, &signature));
            assert_eq!(
                parsed, reference,
                "payload {payload_case:?} with signature {signature_case:?}: the parse \
                 must report the same header as {reference:?}, got {parsed:?}"
            );
        }
    }

    // An EMPTY trailing segment is still not a token: the three segments have to be
    // there, which is the one thing about them that is checked.
    for empty in [
        format!(
            "{}..{}",
            encode_jwt_segment(header.to_string()),
            encode_jwt_segment([7_u8; 32])
        ),
        format!(
            "{}.{}.",
            encode_jwt_segment(header.to_string()),
            encode_jwt_segment("{}")
        ),
    ] {
        let parsed = parse_service_account_token(&empty);
        assert!(
            parsed.is_err(),
            "a token with an empty segment is not three segments, got {parsed:?}"
        );
    }
}
