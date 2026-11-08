// SPDX-License-Identifier: CC0-1.0

//! The REJECTION surface of `AssumeRoleForPodIdentity`: which modeled error shape each
//! class of bad request is answered with, and in which order the classes are decided.
//!
//! Three rules are pinned here, each over a TABLE of inputs rather than over one
//! recorded request:
//!
//! 1. whether the presented value is a JSON Web Token at all is decided from the VALUE,
//!    before any state is read — so a value that is not a JWT is the modeled
//!    `InvalidTokenException` (400) whether or not the cluster the URI names exists,
//!    and the key id a readable header names is then resolved through the OIDC provider
//!    of the cluster the request names, which a cluster that is not registered does not
//!    have — so that request too is answered on its token;
//! 2. every refusal of a value against a constraint the MODEL declares is this service's
//!    own `InvalidParameterException` (400) — the shape the model lists — and not the
//!    framework's `ValidationException`, which this service's model does not declare;
//! 3. such a refusal states the member and the constraint, and echoes the offending
//!    value only for a member that is not `@sensitive`: the value of a `@sensitive`
//!    member never reaches the response.
//!
//! Every test drives the real request path (`MultiServiceRouter::dispatch` over
//! `POST /clusters/{clusterName}/assume-role-for-pod-identity`), through the seeded
//! harness this crate's `assume_role_for_pod_identity` suite owns — every module of
//! `tests/all.rs` is one test binary, so the harness and its seeds are shared rather than
//! copied — so the protocol binding, the modeled constraints, the error serializer and
//! the handler are all exercised.

use service_com_amazonaws_eksauth_eksauthfrontend::state::{
    decode_jwt_segment, encode_jwt_segment, parse_service_account_token, TokenMalformation,
    TOKEN_SIGNING_ALGORITHM,
};

use crate::assume_role_for_pod_identity::{Harness, SEEDS};

/// One segment of a JWT, PADDED — base64url with the `=` RFC 7515 §2 omits, which the
/// modelled `JwtToken` pattern admits and a client may really send. The unpadded form is
/// the service's own [`encode_jwt_segment`], which these tests use everywhere else so
/// they assert the codec the service encodes with rather than a second one.
fn encode_padded_segment(bytes: impl AsRef<[u8]>) -> String {
    aws_smithy_types::base64::encode(bytes.as_ref())
        .replace('+', "-")
        .replace('/', "_")
}

/// The seeded association every row here is sent against, and the cluster that holds it:
/// the first of the shared harness's seeds.
const CLUSTER: &str = SEEDS[0].cluster;
const NAMESPACE: &str = SEEDS[0].namespace;
const SERVICE_ACCOUNT: &str = SEEDS[0].service_account;

/// A cluster name that is NOT registered, so a request naming it can only be answered
/// from the cluster lookup.
const UNREGISTERED_CLUSTER: &str = "sim2real-probe-eks-auth-cluster-1ad61135";

/// What one call answered: the status, the error shape each of restJson1's two
/// resolution points names, and the body.
#[derive(Debug)]
struct Reply {
    status: u16,
    /// `x-amzn-errortype`, which is how restJson1 resolves an error shape.
    errortype: Option<String>,
    /// The body's `__type`, which must name the same shape.
    type_name: Option<String>,
    message: String,
    body: String,
}

impl Reply {
    /// `<status> <shape>`, from the errortype header, for comparing a whole verdict in
    /// one assertion. The coral suffix a real service appends to the header
    /// (`:http://internal.amazon.com/coral/...`) names the same shape, so the shape name
    /// is what is compared.
    fn verdict(&self) -> String {
        let shape = self.errortype.as_deref().unwrap_or("<no errortype header>");
        let shape = shape.split(':').next().unwrap_or(shape);
        format!("{} {}", self.status, shape)
    }
}

/// `POST /clusters/{cluster}/assume-role-for-pod-identity` with `token`.
async fn assume(harness: &Harness, cluster: &str, token: &str) -> Reply {
    post(harness, cluster, serde_json::json!({ "token": token })).await
}

/// The same call with a caller-built body — for an absent member, or a member of the
/// wrong size — read as the whole error envelope.
async fn post(harness: &Harness, cluster: &str, body: serde_json::Value) -> Reply {
    let (status, errortype, raw) = harness.send(cluster, body).await;
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::Value::Null);
    Reply {
        status,
        errortype,
        type_name: parsed["__type"].as_str().map(str::to_string),
        message: parsed["message"].as_str().unwrap_or_default().to_string(),
        body: raw,
    }
}

/// A token whose three segments are exactly `header`, `claims` and `signature`.
fn token_of(header: &str, claims: &str, signature: &str) -> String {
    format!("{header}.{claims}.{signature}")
}

/// A token carrying `header` as its JOSE header, with a well-formed claim set and
/// signature — so the only thing a rejection can be about is the header.
fn token_with_header(header: serde_json::Value) -> String {
    token_of(
        &encode_jwt_segment(header.to_string()),
        &encode_jwt_segment(r#"{"sub":"system:serviceaccount:default:my-sa"}"#),
        &encode_jwt_segment([0_u8; 32]),
    )
}

/// A key id in the form a Kubernetes API server derives for every key it signs with:
/// the base64url of a 32-byte digest, unpadded — 43 characters. `seed` makes two of them
/// different values.
fn key_id(seed: u8) -> String {
    encode_jwt_segment([seed; 32])
}

/// A structurally perfect JWT naming the key `key_id`: three base64url segments, a header
/// naming an algorithm and that key, and a JSON claim set. Nothing issued it, so only a
/// cluster's record of what it issued can refuse it — which is why the sibling suite
/// reaches for this one too.
pub(crate) fn token_for_key(key_id: &str) -> String {
    token_with_header(serde_json::json!({
        "alg": TOKEN_SIGNING_ALGORITHM,
        "kid": key_id,
        "typ": "JWT",
    }))
}

/// RULE 1, over the parse itself: a value is a JWT only if it is three non-empty
/// segments whose FIRST one is base64url of a JSON object carrying every JOSE parameter
/// JWS requires. What the other two segments hold is not part of the read — see
/// `token_claims_are_never_read.rs` for that half of the rule.
///
/// The rows differ along form — segment count, emptiness, the header's encoding and JSON
/// type, and each required header in turn — and include the rows the wire cannot carry (a
/// token with two or four segments fails the modelled `JwtToken` pattern before the
/// handler sees it, so they are only reachable here).
#[test]
fn only_three_json_segments_with_the_required_headers_parse_as_a_token() {
    let good_claims = encode_jwt_segment(r#"{"sub":"system:serviceaccount:default:my-sa"}"#);
    let good_signature = encode_jwt_segment([0_u8; 32]);
    let good_header = encode_jwt_segment(format!(
        r#"{{"alg":"RS256","kid":"{}"}}"#,
        key_id(1)
    ));

    // (case, token, expected malformation)
    let malformed: &[(&str, String, TokenMalformation)] = &[
        (
            // Longer than any token could be: not read at all, however well formed the
            // rest of it is.
            "a token past the length this service reads",
            token_of(
                &good_header,
                &encode_jwt_segment(format!(r#"{{"sub":"{}"}}"#, "s".repeat(32 * 1024))),
                &good_signature,
            ),
            TokenMalformation::Length,
        ),
        (
            "one segment",
            good_header.clone(),
            TokenMalformation::Segments,
        ),
        (
            "two segments",
            format!("{good_header}.{good_claims}"),
            TokenMalformation::Segments,
        ),
        (
            "four segments",
            format!("{good_header}.{good_claims}.{good_signature}.{good_signature}"),
            TokenMalformation::Segments,
        ),
        (
            "an empty header segment",
            token_of("", &good_claims, &good_signature),
            TokenMalformation::Segments,
        ),
        (
            "an empty signature segment",
            token_of(&good_header, &good_claims, ""),
            TokenMalformation::Segments,
        ),
        (
            "a header that is not base64url",
            token_of("A", &good_claims, &good_signature),
            TokenMalformation::HeaderEncoding,
        ),
        (
            "a header that is not JSON",
            token_of(
                &encode_jwt_segment("not json at all"),
                &good_claims,
                &good_signature,
            ),
            TokenMalformation::HeaderNotJson,
        ),
        (
            "a header that is a JSON array",
            token_of(
                &encode_jwt_segment(r#"["alg","kid"]"#),
                &good_claims,
                &good_signature,
            ),
            TokenMalformation::HeaderNotJson,
        ),
        (
            // Absent, `null`, empty and blank all name no key, so all four are the same
            // report: the header is missing.
            "no kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "typ": "JWT" })),
            TokenMalformation::MissingHeader("kid"),
        ),
        (
            "an empty kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "" })),
            TokenMalformation::MissingHeader("kid"),
        ),
        (
            "a kid header that is JSON null",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": null })),
            TokenMalformation::MissingHeader("kid"),
        ),
        (
            "a blank kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "   " })),
            TokenMalformation::MissingHeader("kid"),
        ),
        (
            // A member no key id could ever be: the header object does not read as a
            // JOSE header at all, so the value did not parse.
            "a kid header that is not a string",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": 42 })),
            TokenMalformation::NotJoseHeader,
        ),
        (
            "a kid that is one character short of a key id",
            token_with_header(serde_json::json!({
                "alg": "RS256",
                "kid": key_id(3)[1..].to_string(),
            })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a kid that is one character longer than a key id",
            token_with_header(serde_json::json!({
                "alg": "RS256",
                "kid": format!("{}A", key_id(3)),
            })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a kid of the right length outside the base64url alphabet",
            token_with_header(serde_json::json!({
                "alg": "RS256",
                "kid": format!("{}.", &key_id(3)[1..]),
            })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a kid that is a plain name rather than a key id",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "my-signing-key" })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            // A JOSE header names its algorithm as a string, so one holding anything
            // else is not a JOSE header: the value did not parse.
            "an alg header that is a JSON object",
            token_with_header(serde_json::json!({
                "alg": { "name": "RS256" },
                "kid": key_id(1),
            })),
            TokenMalformation::NotJoseHeader,
        ),
        (
            "no alg header",
            token_with_header(serde_json::json!({ "kid": key_id(1), "typ": "JWT" })),
            TokenMalformation::NotJoseHeader,
        ),
        (
            "neither header",
            token_with_header(serde_json::json!({ "typ": "JWT" })),
            TokenMalformation::NotJoseHeader,
        ),
        (
            // A readable header naming an algorithm no cluster signs with: the shallower
            // layers are past, so this one is reported.
            "an alg header naming another algorithm",
            token_with_header(serde_json::json!({ "alg": "HS256", "kid": key_id(1) })),
            TokenMalformation::InvalidHeader("alg"),
        ),
    ];

    for case in malformed {
        let (name, token, expected) = case;
        let parsed = parse_service_account_token(token);
        assert_eq!(
            parsed,
            Err(*expected),
            "case {name:?}: expected {expected:?}, got {parsed:?}"
        );
    }

    // The same three values whose SECOND or THIRD segment is unreadable are tokens: the
    // read stops at the header, so the segments past it only have to be there. What a
    // caller is answered for each is pinned in `token_claims_are_never_read.rs`.
    let past_the_header: &[(&str, String)] = &[
        (
            "a claim set that is not base64url",
            token_of(&good_header, "A", &good_signature),
        ),
        (
            "a claim set that is a JSON string",
            token_of(
                &good_header,
                &encode_jwt_segment(r#""claims""#),
                &good_signature,
            ),
        ),
        (
            "a signature that is not base64url",
            token_of(&good_header, &good_claims, "A"),
        ),
    ];
    for (name, token) in past_the_header {
        let parsed = parse_service_account_token(token);
        assert_eq!(
            parsed.as_ref().map(|headers| headers.key_id.clone()),
            Ok(key_id(1)),
            "case {name:?}: a segment past the header is not read, so the token parses and \
             reports the key it named, got {parsed:?}"
        );
    }

    // And the form that IS a token: every required header is reported back.
    let well_formed = key_id(7);
    let headers = parse_service_account_token(&token_for_key(&well_formed))
        .expect("a three-segment JWT with alg and a key id parses");
    assert_eq!(
        (headers.algorithm.as_str(), headers.key_id.as_str()),
        (TOKEN_SIGNING_ALGORITHM, well_formed.as_str()),
        "the parsed header must report what the token named, got {headers:?}"
    );
    // A padded segment is a token a client really sends, and the modelled pattern admits
    // `=`, so it parses too.
    let padded = format!(
        "{}.{}.{}",
        encode_padded_segment(format!(r#"{{"alg":"RS256","kid":"{}"}}"#, key_id(8))),
        encode_padded_segment(r#"{"sub":"x"}"#),
        encode_padded_segment([1_u8; 31]),
    );
    assert!(
        parse_service_account_token(&padded).is_ok(),
        "a padded token must parse, got {:?}",
        parse_service_account_token(&padded)
    );
}

/// RULE 1, over the wire: a value that is not a JWT is the modeled
/// `InvalidTokenException` (400) whatever cluster the request names — registered or
/// not — so a broken token is never reported as a missing cluster.
#[tokio::test]
async fn a_token_that_is_not_a_jwt_is_invalid_for_every_cluster_name() {
    let harness = Harness::seeded();
    let cluster_key_id = harness.signing_key_id(CLUSTER);

    // Each row is a value the modelled `JwtToken` pattern ADMITS (three base64url
    // segments) that is nevertheless not a JWT, so each reaches the handler.
    let cases: &[(&str, String)] = &[
        (
            "no kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "typ": "JWT" })),
        ),
        (
            "no alg header",
            token_with_header(serde_json::json!({ "kid": cluster_key_id, "typ": "JWT" })),
        ),
        (
            "an empty kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "" })),
        ),
        (
            "a kid that is not a key id",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "my-signing-key" })),
        ),
        (
            "a header that is not JSON",
            token_of(
                &encode_jwt_segment("not json at all"),
                &encode_jwt_segment(r#"{"sub":"x"}"#),
                &encode_jwt_segment([0_u8; 32]),
            ),
        ),
        (
            "a header that is not base64url",
            token_of(
                "A",
                &encode_jwt_segment(r#"{"sub":"x"}"#),
                &encode_jwt_segment([0_u8; 32]),
            ),
        ),
    ];

    for cluster in [CLUSTER, UNREGISTERED_CLUSTER, "Prod2", "0"] {
        for case in cases {
            let (name, token) = case;
            let reply = assume(&harness, cluster, token).await;
            assert_eq!(
                reply.verdict(),
                "400 InvalidTokenException",
                "case {name:?} against cluster {cluster:?}: expected the modeled 400, got \
                 {reply:?}"
            );
            assert_eq!(
                reply.type_name.as_deref(),
                Some("InvalidTokenException"),
                "case {name:?} against cluster {cluster:?}: the body must name the same shape \
                 as the header, got {reply:?}"
            );
            // The token is `@sensitive`: no part of it may be echoed back.
            assert!(
                !reply.body.contains(token.as_str()),
                "case {name:?} against cluster {cluster:?}: the token value was echoed into \
                 {}",
                reply.body
            );
        }
    }

    // Each layer of reading the header has its own message, because each names a
    // different thing for the caller to fix: a header that does not read as a JOSE
    // header did not parse, a header naming an algorithm or a key this service cannot
    // use is invalid, and one naming nothing at all is missing.
    let header_cases: &[(&str, serde_json::Value, &str)] = &[
        (
            "kid absent",
            serde_json::json!({ "alg": "RS256", "typ": "JWT" }),
            "has missing header - kid.",
        ),
        (
            "alg absent",
            serde_json::json!({ "kid": key_id(1), "typ": "JWT" }),
            "Failed to parse the token included in the request.",
        ),
        (
            "kid empty",
            serde_json::json!({ "alg": "RS256", "kid": "" }),
            "has missing header - kid.",
        ),
        (
            "kid not a string",
            serde_json::json!({ "alg": "RS256", "kid": 42 }),
            "Failed to parse the token included in the request.",
        ),
        (
            "kid blank",
            serde_json::json!({ "alg": "RS256", "kid": "  " }),
            "has missing header - kid.",
        ),
        (
            "kid null",
            serde_json::json!({ "alg": "RS256", "kid": null }),
            "has missing header - kid.",
        ),
        (
            "kid not a key id",
            serde_json::json!({ "alg": "RS256", "kid": "my-signing-key" }),
            "has invalid header - kid.",
        ),
        (
            "alg not a string",
            serde_json::json!({ "alg": false, "kid": key_id(1) }),
            "Failed to parse the token included in the request.",
        ),
        (
            "alg naming another algorithm",
            serde_json::json!({ "alg": "HS256", "kid": key_id(1) }),
            "has invalid header - alg.",
        ),
    ];
    for case in header_cases {
        let (name, header, expected) = case;
        let token = token_with_header(header.clone());
        let reply = assume(&harness, UNREGISTERED_CLUSTER, &token).await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidTokenException",
            "case {name:?}: expected the modeled 400, got {reply:?}"
        );
        assert!(
            reply.message.contains(expected),
            "case {name:?}: the message must state {expected:?}, got {:?}",
            reply.message
        );
    }
}

/// The OTHER half of rule 1: deciding the token's form first does not swallow the state
/// the request is judged against. A readable header's `kid` is resolved through the OIDC
/// provider of the cluster the request NAMES, so a key id that cluster does not publish
/// — including every key id against a cluster that is not registered, which publishes
/// none — is the invalid-`kid` refusal, and the cluster the key DOES resolve against then
/// judges the token on its own record of what it issued.
#[tokio::test]
async fn a_readable_header_is_resolved_against_the_named_cluster() {
    let harness = Harness::seeded();
    let cluster_key_id = harness.signing_key_id(CLUSTER);

    for cluster in [UNREGISTERED_CLUSTER, "no-such-cluster", "DEMO-CLUSTER", "0"] {
        let reply = assume(&harness, cluster, &token_for_key(&cluster_key_id)).await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidTokenException",
            "case {cluster:?}: an unregistered cluster publishes no key the header could \
             name, got {reply:?}"
        );
        assert!(
            reply.message.contains("has invalid header - kid."),
            "case {cluster:?}: the refusal must name the header that resolves to no key, \
             got {:?}",
            reply.message
        );
        // Whether that cluster exists is not reported to a caller holding none of its
        // key material. (Checked on the names long enough that the comparison is about
        // the name rather than about one of its characters.)
        if cluster.len() > 1 {
            assert!(
                !reply.body.contains(cluster),
                "case {cluster:?}: the cluster name reached the response {}",
                reply.body
            );
        }
    }

    // Registered, and the header names the key it publishes: the token's form is fine, so
    // the verdict comes from the cluster's record of what it issued.
    let reply = assume(&harness, CLUSTER, &token_for_key(&cluster_key_id)).await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidTokenException",
        "a token this cluster did not issue is invalid, got {reply:?}"
    );
    assert!(
        !reply.message.contains("header"),
        "a token whose header reads and resolves is not refused on its header, got {:?}",
        reply.message
    );
    // A token whose `kid` is a key id this cluster does not publish is refused on that
    // header, not reported as a missing cluster.
    let reply = assume(&harness, CLUSTER, &token_for_key(&key_id(9))).await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidTokenException",
        "a token naming an unknown signing key is invalid, got {reply:?}"
    );
    assert!(
        reply.message.contains("has invalid header - kid."),
        "a key id the cluster does not publish is the invalid-kid refusal, got {:?}",
        reply.message
    );
    // And the token the cluster DID issue is exchanged.
    let token = harness.token(&SEEDS[0]);
    let reply = assume(&harness, CLUSTER, &token).await;
    assert_eq!(
        reply.status, 200,
        "the issued token must be exchanged, got {reply:?}"
    );
}

/// One request refused against a constraint the model declares.
#[derive(Debug)]
struct BadRequest<'a> {
    /// What makes this row different from the others.
    name: &'a str,
    /// The `clusterName` http label the request carries.
    cluster: &'a str,
    /// The request body.
    body: serde_json::Value,
    /// The member the stated clause must name.
    member: &'a str,
    /// How many clauses the envelope states.
    clauses: usize,
    /// A value that must NOT appear anywhere in the response, because the member
    /// carrying it is `@sensitive`.
    secret: Option<&'a str>,
}

/// RULES 2 and 3: every refusal against a constraint the model declares is this
/// service's own `InvalidParameterException` (400), states the member and the
/// constraint, and echoes the offending value only when the member is not `@sensitive`.
///
/// The rows span the constrained input members (`token`: `@length` min 1, a `@pattern`,
/// `@required`, `@sensitive`; `clusterName`: `@length` 1..100, a `@pattern`,
/// `@required`, `@httpLabel`) and the constraint classes each carries, including both
/// published bounds of the cluster-name length.
#[tokio::test]
async fn a_constraint_refusal_is_the_modeled_invalid_parameter_failure() {
    let harness = Harness::seeded();
    let valid_token = harness.token(&SEEDS[0]);

    let long_name = "c".repeat(101);
    let cases: &[BadRequest] = &[
        BadRequest {
            name: "a token that is not in the JwtToken shape",
            cluster: CLUSTER,
            body: serde_json::json!({ "token": "this-is-not-a-jwt-token" }),
            member: "token",
            clauses: 1,
            secret: Some("this-is-not-a-jwt-token"),
        },
        BadRequest {
            name: "a token with two segments",
            cluster: CLUSTER,
            body: serde_json::json!({ "token": "aaaa.bbbb" }),
            member: "token",
            clauses: 1,
            secret: Some("aaaa.bbbb"),
        },
        BadRequest {
            name: "a token with a character outside the pattern",
            cluster: CLUSTER,
            body: serde_json::json!({ "token": "aaaa.bbbb.cc*cc" }),
            member: "token",
            clauses: 1,
            secret: Some("aaaa.bbbb.cc*cc"),
        },
        BadRequest {
            // Empty breaks BOTH of this member's constraints, so the envelope states
            // both clauses.
            name: "an empty token",
            cluster: CLUSTER,
            body: serde_json::json!({ "token": "" }),
            member: "token",
            clauses: 2,
            secret: None,
        },
        BadRequest {
            name: "no token at all",
            cluster: CLUSTER,
            body: serde_json::json!({}),
            member: "token",
            clauses: 1,
            secret: None,
        },
        BadRequest {
            name: "a cluster name outside the pattern",
            cluster: "-cluster",
            body: serde_json::json!({ "token": valid_token }),
            member: "clusterName",
            clauses: 1,
            secret: None,
        },
        BadRequest {
            name: "a cluster name past the length bound",
            cluster: &long_name,
            body: serde_json::json!({ "token": valid_token }),
            member: "clusterName",
            clauses: 1,
            secret: None,
        },
    ];

    for case in cases {
        let &BadRequest {
            name,
            cluster,
            ref body,
            member,
            clauses,
            secret,
        } = case;
        let reply = post(&harness, cluster, body.clone()).await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidParameterException",
            "case {name:?}: expected the modeled parameter failure, got {reply:?}"
        );
        assert_eq!(
            reply.type_name.as_deref(),
            Some("InvalidParameterException"),
            "case {name:?}: the body must name the same shape as the header, got {reply:?}"
        );
        let envelope = if clauses == 1 {
            "1 validation error detected: ".to_string()
        } else {
            format!("{clauses} validation errors detected: ")
        };
        assert!(
            reply.message.starts_with(&envelope)
                && reply.message.contains(&format!("at '{member}'"))
                && reply.message.contains("failed to satisfy constraint: "),
            "case {name:?}: the message must state {envelope:?} with the member and the \
             constraint, got {:?}",
            reply.message
        );
        if let Some(secret) = secret {
            assert!(
                !reply.body.contains(secret),
                "case {name:?}: the value of the @sensitive member reached the response {}",
                reply.body
            );
        }
    }

    // The value of a member that is NOT @sensitive is stated, because that is what makes
    // the clause actionable — the suppression above is about sensitivity, not a blanket
    // rule.
    let reply = post(
        &harness,
        "-cluster",
        serde_json::json!({ "token": harness.token(&SEEDS[0]) }),
    )
    .await;
    assert!(
        reply.message.contains("'-cluster'"),
        "a non-sensitive member's value must be stated, got {:?}",
        reply.message
    );

    // Two bad members are two clauses in one envelope, and the sensitive one still
    // carries no value.
    let reply = post(
        &harness,
        "-cluster",
        serde_json::json!({ "token": "this-is-not-a-jwt-token" }),
    )
    .await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidParameterException",
        "two bad members are still the modeled parameter failure, got {reply:?}"
    );
    assert!(
        reply.message.starts_with("2 validation errors detected: ")
            && reply.message.contains("at 'token'")
            && reply.message.contains("at 'clusterName'"),
        "both members must be stated, got {:?}",
        reply.message
    );
    assert!(
        !reply.body.contains("this-is-not-a-jwt-token"),
        "the @sensitive value reached the response {}",
        reply.body
    );

    // A @pattern clause states the expression UNANCHORED: the pattern applies to the
    // whole value, so the clause carries the expression between the model's `^` and `$`.
    // The two patterns come from
    // `smithy_query(query="shape", shape="JwtToken,ClusterName")`:
    // `^[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+$` and
    // `^[0-9A-Za-z][A-Za-z0-9\-_]*$`.
    let patterns: &[(&str, &str, serde_json::Value, &str)] = &[
        (
            "token",
            CLUSTER,
            serde_json::json!({ "token": "this-is-not-a-jwt-token" }),
            r"[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+",
        ),
        (
            "clusterName",
            "-cluster",
            serde_json::json!({ "token": harness.token(&SEEDS[0]) }),
            r"[0-9A-Za-z][A-Za-z0-9\-_]*",
        ),
    ];
    for case in patterns {
        let (member, cluster, body, pattern) = case;
        let reply = post(&harness, cluster, body.clone()).await;
        assert!(
            reply
                .message
                .contains(&format!("regular expression pattern: {pattern}")),
            "case {member:?}: the clause must state the unanchored pattern {pattern:?}, got \
             {:?}",
            reply.message
        );
        assert!(
            !reply.message.contains('^') && !reply.message.contains('$'),
            "case {member:?}: the whole-value anchors must not be stated, got {:?}",
            reply.message
        );
    }

    // The published bounds of the cluster-name length: 1 and 100 characters are inside
    // the constraint, so they are answered from state (no cluster is registered under
    // either name, so neither publishes the key the token's header names) rather than
    // refused as parameters.
    for characters in [1_usize, 100] {
        let cluster = "c".repeat(characters);
        let reply = post(
            &harness,
            &cluster,
            serde_json::json!({ "token": harness.token(&SEEDS[0]) }),
        )
        .await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidTokenException",
            "case {characters} characters: a name inside the bound is looked up, got {reply:?}"
        );
    }
}

/// RULE: a clause about the `@sensitive` `token` states the member and the requirement
/// and NO value — `Value at 'token' …` — which is not the `Value null at 'token' …` an
/// ABSENT member earns: the request DID carry a token, and the two facts are worded
/// differently.
///
/// Asserted as the WHOLE message, because the difference between the two is one word:
/// a clause that merely "names the member and the constraint" is satisfied by either.
/// The rows span the classes the member's own constraints can break — its `@pattern`
/// and its `@length` minimum (`smithy_query(query="shape", shape="JwtToken")`) — and
/// the absent case beside them.
#[tokio::test]
async fn a_withheld_tokens_clause_is_not_the_clause_an_absent_token_earns() {
    let harness = Harness::seeded();

    // (case, the body, the requirement the clause states)
    let withheld: &[(&str, serde_json::Value, &str)] = &[
        (
            "a token outside its pattern",
            serde_json::json!({ "token": "this-is-not-a-jwt-token" }),
            r"Member must satisfy regular expression pattern: [A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+\.[A-Za-z0-9-_=]+",
        ),
        (
            "an empty token, under its length minimum",
            serde_json::json!({ "token": "" }),
            "Member must have length greater than or equal to 1",
        ),
    ];
    for (case, body, requirement) in withheld {
        let reply = post(&harness, CLUSTER, body.clone()).await;
        assert_eq!(
            reply.verdict(),
            "400 InvalidParameterException",
            "case {case:?}: expected the modeled parameter failure, got {reply:?}"
        );
        // Every clause of the refusal opens with the withheld subject: an empty token
        // breaks both of the member's constraints, so the envelope carries two.
        for clause in reply.message.split("; ") {
            let opens = clause.starts_with("Value at 'token' ")
                || clause.contains("detected: Value at 'token' ");
            assert!(
                opens,
                "case {case:?}: every clause must open with the withheld subject, not the \
                 null an absent member earns, got {:?}",
                reply.message
            );
        }
        assert!(
            reply.message.contains(requirement),
            "case {case:?}: the clause must state the requirement {requirement:?}, got {:?}",
            reply.message
        );
    }

    // The member ABSENT: nothing was sent, so the clause states the null it was.
    let reply = post(&harness, CLUSTER, serde_json::json!({})).await;
    assert_eq!(
        reply.message,
        "1 validation error detected: Value null at 'token' failed to satisfy constraint: \
         Member must not be null",
        "case \"an absent token\": expected the null clause, got {:?}",
        reply.message
    );
}

/// The tokens a cluster issues are JSON Web Tokens of its own signing key, which is what
/// makes the parse above a check on the same thing the real service checks: the header
/// names the cluster's algorithm and key id, and the claim set names the service account
/// and the cluster's OIDC issuer.
#[tokio::test]
async fn an_issued_token_is_a_jwt_signed_by_the_cluster_key() {
    let harness = Harness::seeded();
    let token = harness.token(&SEEDS[0]);

    let segments: Vec<&str> = token.split('.').collect();
    assert_eq!(
        segments.len(),
        3,
        "an issued token must have three segments, got {token:?}"
    );

    let header: serde_json::Value = serde_json::from_slice(
        &decode_jwt_segment(segments[0]).expect("the header segment is base64url"),
    )
    .expect("the header segment is JSON");
    assert_eq!(
        (
            header["alg"].as_str(),
            header["kid"].as_str(),
            header["typ"].as_str()
        ),
        (
            Some(TOKEN_SIGNING_ALGORITHM),
            Some(harness.signing_key_id(CLUSTER).as_str()),
            Some("JWT")
        ),
        "the header must name the cluster's algorithm and key, got {header}"
    );

    let claims: serde_json::Value = serde_json::from_slice(
        &decode_jwt_segment(segments[1]).expect("the claim segment is base64url"),
    )
    .expect("the claim segment is JSON");
    assert_eq!(
        claims["sub"].as_str(),
        Some(format!("system:serviceaccount:{NAMESPACE}:{SERVICE_ACCOUNT}").as_str()),
        "the claims must name the service account, got {claims}"
    );
    assert_eq!(
        claims["iss"].as_str(),
        Some(harness.oidc_issuer(CLUSTER).as_str()),
        "the claims must name the cluster's OIDC issuer, got {claims}"
    );

    // Two tokens of the same service account are two distinct values, so one record can
    // never stand for another.
    assert_ne!(
        harness.token(&SEEDS[0]),
        harness.token(&SEEDS[0]),
        "each issued token must be its own value"
    );
}
