// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `ClientRequestToken` a write that creates a secret
//! version has to carry, driven over the wire through the multi-service router.
//!
//! The rule under test: a write that creates a secret version names that version,
//! and the name is the token the CALLER generates — "If you generate a raw HTTP
//! request to the Secrets Manager service endpoint, then you must generate a
//! ClientRequestToken and include it in the request."
//! (`CreateSecretRequest$ClientRequestToken`, and the same sentence on
//! `PutSecretValueRequest$ClientRequestToken` and
//! `UpdateSecretRequest$ClientRequestToken`, this service's Smithy model.) So:
//!
//! * a write that creates a secret, or appends a version to one, and names no
//!   token is refused with `InvalidRequestException` and stores nothing;
//! * an update that stores no value creates no version, so it needs no token
//!   ("If you include SecretString or SecretBinary, then Secrets Manager creates a
//!   new version for the secret, and this parameter specifies the unique
//!   identifier for the new version",
//!   `UpdateSecretRequest$ClientRequestToken`);
//! * a write that names one stakes the version under exactly that id, which is
//!   what keeps `AWSCURRENT` on the version the caller knows about even after
//!   further token-less writes are refused.
//!
//! Every test is a TABLE over the class the rule generalises: the operation, which
//! value member carries the value, whether a value is carried at all, and the
//! length of the token (both published bounds of `ClientRequestTokenType`, 32..64,
//! and one past each).
//!
//! This suite also owns the helper every other suite in this binary builds its
//! writes with ([`with_staked_version`]), so a setup that writes a value names the
//! version it creates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Map, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 header naming `secretsmanager`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is
/// registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// The operations whose `ClientRequestToken` names the version they create: the
/// two that always create one, and the update that creates one only when it
/// stores a value.
const ALWAYS_NAMES_A_VERSION: &[&str] = &["CreateSecret", "PutSecretValue"];
const NAMES_A_VERSION_WITH_A_VALUE: &[&str] = &["UpdateSecret"];

/// The members a request carries a value in: "You must include SecretBinary or
/// SecretString, but not both." (`PutSecretValueRequest$SecretString`, this
/// service's Smithy model.)
const VALUE_MEMBERS: &[&str] = &["SecretString", "SecretBinary"];

/// The published bounds of `ClientRequestTokenType` (`@length` min 32, max 64,
/// this service's Smithy model).
pub const TOKEN_MIN_LEN: usize = 32;
pub const TOKEN_MAX_LEN: usize = 64;

/// The sentence Secrets Manager refuses a version-creating write with when the
/// request names no token.
pub const MISSING_TOKEN_MESSAGE: &str =
    "You must provide a ClientRequestToken value. We recommend a UUID-type value.";

/// Hands out a token no other call in this test binary uses, so an injected token
/// never collides with another version of the same secret.
static TOKENS: AtomicU64 = AtomicU64::new(0);

/// A `ClientRequestToken` of `length` characters, which is also the version id the
/// write that carries it stakes.
///
/// Every token this test binary hands out is unique: the serial number sits in the
/// first few characters, so even a token truncated to the shortest modelled length
/// still names a version no other call in the binary names.
pub fn token_of_length(length: usize) -> String {
    let unique = TOKENS.fetch_add(1, Ordering::Relaxed);
    let token = format!("token-{unique}-");
    let mut token: String = token.chars().take(length).collect();
    while token.len() < length {
        token.push('x');
    }
    token
}

/// A fresh token inside the modelled length bound.
pub fn fresh_token() -> String {
    token_of_length(TOKEN_MIN_LEN + 4)
}

/// `body` with the `ClientRequestToken` a version-creating write must carry, added
/// when the case named none.
///
/// A case that names its own token keeps it — that is how the suites that assert
/// on idempotency, on a token collision or on this very rule control the version
/// id — and an update that stores no value is passed through untouched, since it
/// creates no version to name.
pub fn with_staked_version(operation: &str, body: &Value) -> Value {
    let mut staked = body.clone();
    if let Some(members) = staked.as_object_mut() {
        let names_a_version = ALWAYS_NAMES_A_VERSION.contains(&operation)
            || (NAMES_A_VERSION_WITH_A_VALUE.contains(&operation)
                && VALUE_MEMBERS
                    .iter()
                    .any(|member| members.contains_key(*member)));
        if names_a_version && !members.contains_key("ClientRequestToken") {
            members.insert("ClientRequestToken".to_string(), json!(fresh_token()));
        }
    }
    staked
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request EXACTLY as the case states it — no token is
/// injected here, since the token is what this suite is about — and return its
/// status and JSON body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("secretsmanager.{operation}"))
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body is serializable"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Send one request that must succeed, and return its body.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value, case: &str) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from {operation}, got {status} with body {parsed}"
    );
    parsed
}

/// The `__type` and `Message` of an error body, as the awsJson error envelope
/// carries them.
fn refusal(parsed: &Value) -> (String, String) {
    let code = parsed
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string();
    let message = parsed
        .get("Message")
        .or_else(|| parsed.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// A required string member of a response body.
fn text(parsed: &Value, member: &str, case: &str) -> String {
    parsed
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no {member} in {parsed}"))
        .to_string()
}

/// The `VersionIdsToStages` map `DescribeSecret` reports, as version id to the
/// labels attached to it; empty when the secret holds no labelled version, which
/// is how the response omits the member.
async fn staging_map(
    router: &MultiServiceRouter,
    secret_id: &str,
    case: &str,
) -> Map<String, Value> {
    let described = ok(router, "DescribeSecret", &json!({"SecretId": secret_id}), case).await;
    described
        .get("VersionIdsToStages")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The value a write of `value` carries under `member`: `SecretBinary` is base64
/// on the wire ("The binary data to encrypt and store in the new version of the
/// secret", `PutSecretValueRequest$SecretBinary`).
fn value_member(member: &str, value: &str) -> Value {
    match member {
        "SecretBinary" => json!(base64::engine::general_purpose::STANDARD.encode(value.as_bytes())),
        _ => json!(value),
    }
}

#[tokio::test]
async fn a_write_that_creates_a_version_with_no_client_request_token_is_refused() {
    // The rule: a write that creates a version names it, and the name is the token
    // the caller generates, so a raw request that names none is refused with
    // `InvalidRequestException` (HTTP 400, this service's Smithy model) and stores
    // nothing. The rows run over every write that creates a version and over both
    // members it can carry the value in — including a create that carries NO value:
    // a create stakes the secret itself, and its token is what a retry replays, so
    // it is required whether or not an initial version comes with it.
    let rows: &[(&str, Option<&str>)] = &[
        ("CreateSecret", Some("SecretString")),
        ("CreateSecret", Some("SecretBinary")),
        ("CreateSecret", None),
        ("PutSecretValue", Some("SecretString")),
        ("PutSecretValue", Some("SecretBinary")),
        ("UpdateSecret", Some("SecretString")),
        ("UpdateSecret", Some("SecretBinary")),
    ];
    for (operation, member) in rows {
        let case = format!(
            "{operation} carrying {} and no ClientRequestToken",
            member.unwrap_or("no value member")
        );
        let router = emulator();
        let name = format!("token-less/{operation}-{}", member.unwrap_or("valueless"));
        // The secret the write is addressed to already exists, created with a
        // token of its own, so the refusal under test is about the write and
        // never about the secret.
        let owner_token = fresh_token();
        if *operation != "CreateSecret" {
            ok(
                &router,
                "CreateSecret",
                &json!({
                    "Name": name.clone(),
                    "SecretString": "the-first-value",
                    "ClientRequestToken": owner_token.clone(),
                }),
                &case,
            )
            .await;
        }

        let mut body = Map::new();
        if *operation == "CreateSecret" {
            body.insert("Name".to_string(), json!(name.clone()));
            body.insert("Description".to_string(), json!("a create with no token"));
        } else {
            body.insert("SecretId".to_string(), json!(name.clone()));
        }
        if let Some(member) = member {
            body.insert((*member).to_string(), value_member(member, "a-new-value"));
        }
        let (status, parsed) = call(&router, operation, &Value::Object(body)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with body {parsed}"
        );
        let (code, message) = refusal(&parsed);
        assert_eq!(
            code, "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {parsed}"
        );
        assert_eq!(
            message, MISSING_TOKEN_MESSAGE,
            "case {case:?}: expected the missing-token sentence, got {message:?}"
        );

        // Nothing was stored: a refused create leaves no secret at all, and a
        // refused write to an existing secret leaves it holding only the
        // version its owner staked, still current.
        if *operation == "CreateSecret" {
            let (status, parsed) =
                call(&router, "DescribeSecret", &json!({"SecretId": name.clone()})).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the refused create to store nothing, got {status} \
                 with body {parsed}"
            );
            let (code, _) = refusal(&parsed);
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {parsed}"
            );
        } else {
            let staged = staging_map(&router, &name, &case).await;
            assert_eq!(
                staged,
                Map::from_iter([(owner_token.clone(), json!(["AWSCURRENT"]))]),
                "case {case:?}: expected only the owner's version, still current, got {staged:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_that_creates_no_version_needs_no_client_request_token() {
    // `UpdateSecret`'s own member conditions the requirement on a version being
    // created: "If you include SecretString or SecretBinary, then Secrets Manager
    // creates a new version for the secret, and this parameter specifies the unique
    // identifier for the new version."
    // (`UpdateSecretRequest$ClientRequestToken`.) An update that stores no value
    // creates no version to name, so it is accepted with no token — and reports no
    // `VersionId`, since none was created. The rows run over the metadata members
    // such an update can name.
    let router = emulator();
    let token = fresh_token();
    ok(
        &router,
        "CreateSecret",
        &json!({
            "Name": "metadata-only",
            "SecretString": "the-only-value",
            "ClientRequestToken": token.clone(),
        }),
        "the secret the updates are addressed to",
    )
    .await;

    for (case, members) in [
        ("only a description", json!({"Description": "post-mortem"})),
        ("only a KMS key", json!({"KmsKeyId": ""})),
        (
            "a description and a type",
            json!({"Description": "both", "Type": "a-partner"}),
        ),
    ] {
        let mut body = members
            .as_object()
            .expect("the members are a JSON object")
            .clone();
        body.insert("SecretId".to_string(), json!("metadata-only"));
        let updated = ok(&router, "UpdateSecret", &Value::Object(body), case).await;
        assert!(
            updated.get("VersionId").is_none(),
            "case {case:?}: expected no VersionId from a metadata-only update, got {updated}"
        );
    }

    // The secret still holds exactly the one version its create named, so no
    // metadata update staked a version under an id the caller never chose.
    let staged = staging_map(&router, "metadata-only", "after the metadata updates").await;
    assert_eq!(
        staged,
        Map::from_iter([(token.clone(), json!(["AWSCURRENT"]))]),
        "expected only the created version, still current, got {staged:?}"
    );
}

#[tokio::test]
async fn the_version_a_write_names_is_the_one_that_holds_awscurrent() {
    // A write that names a token stakes the new version under exactly that id and
    // "automatically moves the staging label AWSCURRENT to this version"
    // (`PutSecretValue`, this service's Smithy model), leaving AWSPREVIOUS on the
    // version it was taken from. The rows run over the writing operation, over the
    // value member, and over the token's length at both published bounds of
    // `ClientRequestTokenType` (32..64).
    for operation in ["PutSecretValue", "UpdateSecret"] {
        for member in VALUE_MEMBERS {
            for length in [TOKEN_MIN_LEN, 36, TOKEN_MAX_LEN] {
                let case =
                    format!("{operation} carrying {member} and a token of {length} characters");
                let router = emulator();
                let name = format!("staked/{operation}-{member}-{length}");
                let first = token_of_length(length);
                ok(
                    &router,
                    "CreateSecret",
                    &json!({
                        "Name": name.clone(),
                        "SecretString": "the-first-value",
                        "ClientRequestToken": first.clone(),
                    }),
                    &case,
                )
                .await;

                let second = token_of_length(length);
                let mut body = Map::new();
                body.insert("SecretId".to_string(), json!(name.clone()));
                body.insert(
                    (*member).to_string(),
                    value_member(member, "the-second-value"),
                );
                body.insert("ClientRequestToken".to_string(), json!(second.clone()));
                let written = ok(&router, operation, &Value::Object(body), &case).await;
                assert_eq!(
                    text(&written, "VersionId", &case),
                    second,
                    "case {case:?}: expected the token to become the version id, got {written}"
                );

                let staged = staging_map(&router, &name, &case).await;
                assert_eq!(
                    staged,
                    Map::from_iter([
                        (second.clone(), json!(["AWSCURRENT"])),
                        (first.clone(), json!(["AWSPREVIOUS"])),
                    ]),
                    "case {case:?}: expected AWSCURRENT on the version this write named, got \
                     {staged:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_token_outside_the_modelled_length_is_refused_and_stores_nothing() {
    // `ClientRequestTokenType` is `@length` 32..64 (this service's Smithy model),
    // so a token one character past either bound is refused — at each bound it is
    // accepted, which the test above covers. The rows run over both operations that
    // append a version and over both bounds, one character outside each.
    for operation in ["PutSecretValue", "UpdateSecret"] {
        for length in [TOKEN_MIN_LEN - 1, TOKEN_MAX_LEN + 1] {
            let case = format!("{operation} carrying a token of {length} characters");
            let router = emulator();
            let name = format!("too-long/{operation}-{length}");
            let owner = fresh_token();
            ok(
                &router,
                "CreateSecret",
                &json!({
                    "Name": name.clone(),
                    "SecretString": "the-first-value",
                    "ClientRequestToken": owner.clone(),
                }),
                &case,
            )
            .await;

            let (status, parsed) = call(
                &router,
                operation,
                &json!({
                    "SecretId": name.clone(),
                    "SecretString": "the-second-value",
                    "ClientRequestToken": token_of_length(length),
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a token outside 32..64, got {status} with body \
                 {parsed}"
            );
            let staged = staging_map(&router, &name, &case).await;
            assert_eq!(
                staged,
                Map::from_iter([(owner.clone(), json!(["AWSCURRENT"]))]),
                "case {case:?}: expected the refused write to store nothing, got {staged:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_caller_s_own_version_stays_current_when_token_less_writes_are_refused() {
    // The user-visible consequence of the rule: a client that creates a version
    // with its OWN token, and whose later token-less writes are refused, still
    // finds that version holding AWSCURRENT — the label `GetSecretValue` resolves
    // when the caller names neither a version nor a stage — and finds no version it
    // never chose. The rows run over the sequence of refused writes and over the
    // member each one carried its value in.
    for (case, refused) in [
        ("one refused PutSecretValue", vec![("PutSecretValue", "SecretString")]),
        ("one refused UpdateSecret", vec![("UpdateSecret", "SecretString")]),
        (
            "a refused put then a refused update",
            vec![
                ("PutSecretValue", "SecretString"),
                ("UpdateSecret", "SecretString"),
            ],
        ),
        (
            "refused binary writes",
            vec![
                ("PutSecretValue", "SecretBinary"),
                ("UpdateSecret", "SecretBinary"),
            ],
        ),
        (
            "every token-less write, both members",
            vec![
                ("PutSecretValue", "SecretString"),
                ("PutSecretValue", "SecretBinary"),
                ("UpdateSecret", "SecretString"),
                ("UpdateSecret", "SecretBinary"),
            ],
        ),
    ] {
        let router = emulator();
        let name = "the-caller-s-own";
        let token = fresh_token();
        let created = ok(
            &router,
            "CreateSecret",
            &json!({
                "Name": name,
                "SecretString": "the-caller-s-value",
                "ClientRequestToken": token.clone(),
            }),
            case,
        )
        .await;
        assert_eq!(
            text(&created, "VersionId", case),
            token,
            "case {case:?}: expected the caller's token to be the version id, got {created}"
        );

        for (index, (operation, member)) in refused.iter().enumerate() {
            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name));
            body.insert(
                (*member).to_string(),
                value_member(member, &format!("a-value-{index}")),
            );
            let (status, parsed) = call(&router, operation, &Value::Object(body)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected {operation} with no token to be refused, got {status} \
                 with body {parsed}"
            );
        }

        // Exactly one version exists, it is the one the caller named, and it still
        // holds AWSCURRENT.
        let staged = staging_map(&router, name, case).await;
        assert_eq!(
            staged,
            Map::from_iter([(token.clone(), json!(["AWSCURRENT"]))]),
            "case {case:?}: expected only the caller's version, staged AWSCURRENT, got {staged:?}"
        );

        // So the read that names neither a version nor a stage resolves the
        // caller's value, not one a refused write would have stored.
        let read = ok(
            &router,
            "GetSecretValue",
            &json!({"SecretId": name}),
            case,
        )
        .await;
        assert_eq!(
            text(&read, "VersionId", case),
            token,
            "case {case:?}: expected the caller's version to be current, got {read}"
        );
        assert_eq!(
            text(&read, "SecretString", case),
            "the-caller-s-value",
            "case {case:?}: expected the caller's own value back, got {read}"
        );
    }
}
