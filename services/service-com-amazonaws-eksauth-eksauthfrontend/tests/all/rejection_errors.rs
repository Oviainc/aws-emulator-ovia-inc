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
//!    while a well-formed token against an unregistered cluster is still the modeled
//!    `ResourceNotFoundException` (404);
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
    decode_jwt_segment, encode_jwt_segment, parse_service_account_token, JwtSegment,
    TokenMalformation, REQUIRED_JOSE_HEADERS, TOKEN_SIGNING_ALGORITHM,
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
/// base64url segments whose header and claim set are JSON objects and whose header
/// carries every JOSE parameter JWS requires.
///
/// The rows differ along form — segment count, emptiness, encoding, JSON type, and each
/// required header in turn — and include the rows the wire cannot carry (a token with
/// two or four segments fails the modelled `JwtToken` pattern before the handler sees
/// it, so they are only reachable here).
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
            TokenMalformation::Encoding(JwtSegment::Header),
        ),
        (
            "a claim set that is not base64url",
            token_of(&good_header, "A", &good_signature),
            TokenMalformation::Encoding(JwtSegment::Claims),
        ),
        (
            "a signature that is not base64url",
            token_of(&good_header, &good_claims, "A"),
            TokenMalformation::Encoding(JwtSegment::Signature),
        ),
        (
            "a header that is not JSON",
            token_of(
                &encode_jwt_segment("not json at all"),
                &good_claims,
                &good_signature,
            ),
            TokenMalformation::NotJson(JwtSegment::Header),
        ),
        (
            "a header that is a JSON array",
            token_of(
                &encode_jwt_segment(r#"["alg","kid"]"#),
                &good_claims,
                &good_signature,
            ),
            TokenMalformation::NotJson(JwtSegment::Header),
        ),
        (
            "a claim set that is a JSON string",
            token_of(
                &good_header,
                &encode_jwt_segment(r#""claims""#),
                &good_signature,
            ),
            TokenMalformation::NotJson(JwtSegment::Claims),
        ),
        (
            "no kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "typ": "JWT" })),
            TokenMalformation::MissingHeader("kid"),
        ),
        (
            "an empty kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "" })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a kid header that is not a string",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": 42 })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a kid header that is JSON null",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": null })),
            TokenMalformation::InvalidHeader("kid"),
        ),
        (
            "a blank kid header",
            token_with_header(serde_json::json!({ "alg": "RS256", "kid": "   " })),
            TokenMalformation::InvalidHeader("kid"),
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
            "an alg header that is a JSON object",
            token_with_header(serde_json::json!({
                "alg": { "name": "RS256" },
                "kid": key_id(1),
            })),
            TokenMalformation::InvalidHeader("alg"),
        ),
        (
            "no alg header",
            token_with_header(serde_json::json!({ "kid": key_id(1), "typ": "JWT" })),
            TokenMalformation::MissingHeader("alg"),
        ),
        (
            "neither required header",
            token_with_header(serde_json::json!({ "typ": "JWT" })),
            TokenMalformation::MissingHeader("alg"),
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
    // The parse is about form only: it says nothing about audience, lifetime or who
    // issued the token, so a token no cluster ever issued still parses.
    assert_eq!(
        REQUIRED_JOSE_HEADERS,
        ["alg", "kid"],
        "the required JOSE headers are the ones JWS mandates"
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
        (
            "a claim set that is not a JSON object",
            token_of(
                &encode_jwt_segment(format!(
                    r#"{{"alg":"RS256","kid":"{}"}}"#,
                    harness.signing_key_id(CLUSTER)
                )),
                &encode_jwt_segment("42rubbish"),
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

    // A token that is MISSING A NAMED HEADER says which one, and one whose named header
    // holds something that is not a value for it says that instead — because those are
    // different things for the caller to fix.
    let header_cases: &[(&str, serde_json::Value, &str)] = &[
        (
            "kid absent",
            serde_json::json!({ "alg": "RS256", "typ": "JWT" }),
            "has missing header - kid.",
        ),
        (
            "alg absent",
            serde_json::json!({ "kid": key_id(1), "typ": "JWT" }),
            "has missing header - alg.",
        ),
        (
            "kid empty",
            serde_json::json!({ "alg": "RS256", "kid": "" }),
            "has invalid header - kid.",
        ),
        (
            "kid not a string",
            serde_json::json!({ "alg": "RS256", "kid": 42 }),
            "has invalid header - kid.",
        ),
        (
            "kid blank",
            serde_json::json!({ "alg": "RS256", "kid": "  " }),
            "has invalid header - kid.",
        ),
        (
            "kid null",
            serde_json::json!({ "alg": "RS256", "kid": null }),
            "has invalid header - kid.",
        ),
        (
            "kid not a key id",
            serde_json::json!({ "alg": "RS256", "kid": "my-signing-key" }),
            "has invalid header - kid.",
        ),
        (
            "alg not a string",
            serde_json::json!({ "alg": false, "kid": key_id(1) }),
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

/// The OTHER half of rule 1: deciding the token's form first does not swallow the
/// cluster lookup. A well-formed token against an unregistered cluster is still the
/// modeled 404, and against the registered cluster it is judged on what that cluster
/// issued.
#[tokio::test]
async fn a_well_formed_token_still_resolves_the_cluster() {
    let harness = Harness::seeded();
    let cluster_key_id = harness.signing_key_id(CLUSTER);

    for cluster in [UNREGISTERED_CLUSTER, "no-such-cluster", "DEMO-CLUSTER", "0"] {
        let reply = assume(&harness, cluster, &token_for_key(&cluster_key_id)).await;
        assert_eq!(
            reply.verdict(),
            "404 ResourceNotFoundException",
            "case {cluster:?}: an unregistered cluster is still not found, got {reply:?}"
        );
    }

    // Registered: the token's form is fine, so the verdict comes from the cluster's
    // record of what it issued.
    let reply = assume(&harness, CLUSTER, &token_for_key(&cluster_key_id)).await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidTokenException",
        "a token this cluster did not issue is invalid, got {reply:?}"
    );
    // A token whose `kid` is a key id this cluster does not publish is refused as
    // invalid, not reported as a missing cluster.
    let reply = assume(&harness, CLUSTER, &token_for_key(&key_id(9))).await;
    assert_eq!(
        reply.verdict(),
        "400 InvalidTokenException",
        "a token naming an unknown signing key is invalid, got {reply:?}"
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
    // the constraint, so they are answered from state (the cluster is not registered)
    // rather than refused as parameters.
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
            "404 ResourceNotFoundException",
            "case {characters} characters: a name inside the bound is looked up, got {reply:?}"
        );
    }
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
