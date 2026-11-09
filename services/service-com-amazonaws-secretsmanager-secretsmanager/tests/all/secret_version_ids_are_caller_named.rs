// SPDX-License-Identifier: CC0-1.0

//! Property-based test over GENERATED sequences of writes: every secret version
//! this service stores is named by a `ClientRequestToken` its caller generated.
//!
//! THE RULE: a write that creates a secret version takes that version's id from the
//! request — "this parameter specifies the unique identifier for the new version
//! [...] If you generate a raw HTTP request to the Secrets Manager service
//! endpoint, then you must generate a `ClientRequestToken` and include it in the
//! request." (`CreateSecretRequest$ClientRequestToken`, and the same sentence on
//! `PutSecretValueRequest$ClientRequestToken` and
//! `UpdateSecretRequest$ClientRequestToken`, this service's Smithy model) — so a
//! request that would create a version while naming no token is refused with
//! `InvalidRequestException` (HTTP 400, this service's Smithy model), and the
//! service never invents an id of its own.
//!
//! [`crate::secret_client_request_token`] pins that rule as a TABLE over one write
//! at a time. This suite pins it as an INVARIANT over whole generated SEQUENCES of
//! writes against one secret, because defaulting a version id is a defect whose
//! trace is what the store holds AFTERWARDS: after any sequence of accepted and
//! refused writes,
//!
//! * the ids of the versions the secret holds are EXACTLY the tokens its accepted
//!   writes carried — none staked under an id no caller chose, none a caller did
//!   choose lost;
//! * `AWSCURRENT` sits on the last of them and `AWSPREVIOUS` on the one before
//!   ("Secrets Manager automatically moves the staging label AWSCURRENT to the new
//!   version. Then it attaches the label AWSPREVIOUS to the version that AWSCURRENT
//!   was removed from.", `PutSecretValue`, this service's Smithy model), so the read
//!   that names neither a version nor a stage resolves the caller's own last value;
//! * a refused write changes none of it.
//!
//! Each case is one generated sequence, drawn from a seed this suite fixes so a
//! failure reproduces: the seed is part of every assertion message, together with
//! the sequence it generated. The generator varies what the rule generalises over —
//! the operation, whether the write carries a value and in which member, whether it
//! names a token, and the token's length across the modelled bound of
//! `ClientRequestTokenType` (32..64) — and nothing is written to disk.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::Engine as _;
use rand::{Rng, SeedableRng};
use serde_json::{json, Map, Value};
use service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::secret_client_request_token::{
    token_of_length, MISSING_TOKEN_MESSAGE, TOKEN_MAX_LEN, TOKEN_MIN_LEN,
};

/// A SigV4 header naming `secretsmanager`; the router routes on the credential
/// scope and the emulator verifies signatures only when an authenticator is
/// registered, which this harness does not do.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-west-2/secretsmanager/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=deadbeef";

/// How many generated sequences the invariant is checked over. Bounded, so the
/// suite costs a fixed amount of time.
const CASES: u64 = 24;

/// The longest generated sequence. Well under [the versions one secret holds]
/// (`VERSIONS_PER_SECRET`, this service's state), so no case depends on a
/// deprecated version being evicted.
const MAX_WRITES: usize = 5;

/// The seed the first case is generated from; case `n` is generated from
/// `GENERATOR_SEED + n`, so every case reproduces from the seed its assertions
/// name.
const GENERATOR_SEED: u64 = 0x5EC2_E700;

/// The member a write carries its value in: "You must include SecretBinary or
/// SecretString, but not both." (`PutSecretValueRequest$SecretString`, this
/// service's Smithy model.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueMember {
    SecretString,
    SecretBinary,
}

impl ValueMember {
    fn member(self) -> &'static str {
        match self {
            Self::SecretString => "SecretString",
            Self::SecretBinary => "SecretBinary",
        }
    }

    /// `value` as this member carries it on the wire: `SecretBinary` is a blob,
    /// which awsJson1_1 carries base64-encoded.
    fn on_the_wire(self, value: &str) -> Value {
        match self {
            Self::SecretString => json!(value),
            Self::SecretBinary => {
                json!(base64::engine::general_purpose::STANDARD.encode(value.as_bytes()))
            }
        }
    }
}

/// One generated write against the secret under test.
#[derive(Debug, Clone)]
struct GeneratedWrite {
    /// The operation the write is made with: both of the ones that append a
    /// version to an existing secret.
    operation: &'static str,
    /// The member it carries its value in, or `None` for an update that stores no
    /// value and so creates no version to name.
    member: Option<ValueMember>,
    /// The `ClientRequestToken` it names, or `None` for the raw request that names
    /// none.
    token: Option<String>,
}

/// The sequence case `seed` generates.
fn generated_writes(seed: u64) -> Vec<GeneratedWrite> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    // The token lengths drawn from: both published bounds of
    // `ClientRequestTokenType` (`@length` min 32, max 64, this service's Smithy
    // model) and one inside them.
    let lengths = [TOKEN_MIN_LEN, 36, TOKEN_MAX_LEN];
    let count = rng.random_range(1..=MAX_WRITES);
    (0..count)
        .map(|_| {
            let operation = if rng.random_bool(0.5) {
                "PutSecretValue"
            } else {
                "UpdateSecret"
            };
            // "If you include SecretString or SecretBinary, then Secrets Manager
            // creates a new version for the secret"
            // (`UpdateSecretRequest$ClientRequestToken`): only an update can leave
            // the value out and still be a request this service serves, since
            // `PutSecretValue` exists to store one.
            let member = if operation == "UpdateSecret" && rng.random_bool(0.25) {
                None
            } else if rng.random_bool(0.5) {
                Some(ValueMember::SecretString)
            } else {
                Some(ValueMember::SecretBinary)
            };
            let token = if rng.random_bool(0.5) {
                None
            } else {
                Some(token_of_length(lengths[rng.random_range(0..lengths.len())]))
            };
            GeneratedWrite {
                operation,
                member,
                token,
            }
        })
        .collect()
}

/// A fresh emulator holding only this service, over its own engine.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Send one `awsJson1_1` request EXACTLY as the generated case states it — no token
/// is injected here, since the token is what this suite is about — and return its
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

/// Every version id the secret holds, deprecated ones included: "By default,
/// versions without staging labels aren't included" — so the listing asks for them,
/// which is what makes an id the service invented visible even once it carries no
/// label (`ListSecretVersionIdsRequest$IncludeDeprecated`, this service's Smithy
/// model).
async fn stored_version_ids(
    router: &MultiServiceRouter,
    secret_id: &str,
    case: &str,
) -> BTreeSet<String> {
    let listed = ok(
        router,
        "ListSecretVersionIds",
        &json!({"SecretId": secret_id, "IncludeDeprecated": true}),
        case,
    )
    .await;
    listed
        .get("Versions")
        .and_then(Value::as_array)
        .map(|versions| {
            versions
                .iter()
                .map(|version| text(version, "VersionId", case))
                .collect()
        })
        .unwrap_or_default()
}

/// The `VersionIdsToStages` map `DescribeSecret` reports, as version id to the
/// labels attached to it.
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

/// The labelling the rule leaves on a secret whose versions were created, in order,
/// by the tokens of `versions`: `AWSCURRENT` on the last and `AWSPREVIOUS` on the
/// one before it, every older version deprecated and so left out of the map.
fn expected_staging(versions: &[String]) -> Map<String, Value> {
    let mut expected = Map::new();
    let mut from_the_end = versions.iter().rev();
    if let Some(current) = from_the_end.next() {
        expected.insert(current.clone(), json!(["AWSCURRENT"]));
    }
    if let Some(previous) = from_the_end.next() {
        expected.insert(previous.clone(), json!(["AWSPREVIOUS"]));
    }
    expected
}

#[tokio::test]
async fn every_stored_version_is_named_by_a_token_its_caller_generated() {
    for case_seed in GENERATOR_SEED..GENERATOR_SEED + CASES {
        let writes = generated_writes(case_seed);
        let case = format!("seed {case_seed}, writes {writes:?}");
        let router = emulator();
        let name = format!("generated/{case_seed}");

        // The secret the sequence writes to is created with a token of the
        // caller's own, so the sequence starts from a version the caller named.
        let created_token = token_of_length(TOKEN_MIN_LEN);
        let created = ok(
            &router,
            "CreateSecret",
            &json!({
                "Name": name.clone(),
                "SecretString": "the-created-value",
                "ClientRequestToken": created_token.clone(),
            }),
            &case,
        )
        .await;
        assert_eq!(
            text(&created, "VersionId", &case),
            created_token,
            "case {case:?}: expected the create to stake the token it named, got {created}"
        );

        // What the rule says the store holds: the versions the accepted writes
        // named, in the order they were accepted, and the value the last of them
        // stored.
        let mut versions = vec![created_token.clone()];
        let mut current_value = ("the-created-value".to_string(), ValueMember::SecretString);

        for (index, write) in writes.iter().enumerate() {
            let value = format!("generated-value-{index}");
            let mut body = Map::new();
            body.insert("SecretId".to_string(), json!(name.clone()));
            match write.member {
                Some(member) => {
                    body.insert(member.member().to_string(), member.on_the_wire(&value));
                }
                // An update that stores no value still has to change something:
                // "Modifies the details of a secret" (`UpdateSecret`, this
                // service's Smithy model).
                None => {
                    body.insert("Description".to_string(), json!(format!("step {index}")));
                }
            }
            if let Some(token) = write.token.as_ref() {
                body.insert("ClientRequestToken".to_string(), json!(token.clone()));
            }
            let step = format!("{case}, step {index}");
            let (status, parsed) = call(&router, write.operation, &Value::Object(body)).await;

            match (write.member, write.token.as_ref()) {
                // A write that would create a version and names no token is
                // refused, and stakes nothing.
                (Some(_), None) => {
                    assert_eq!(
                        status, 400,
                        "case {step:?}: expected 400 from a token-less {}, got {status} with \
                         body {parsed}",
                        write.operation
                    );
                    let (code, message) = refusal(&parsed);
                    assert_eq!(
                        code, "InvalidRequestException",
                        "case {step:?}: expected InvalidRequestException, got {parsed}"
                    );
                    assert_eq!(
                        message, MISSING_TOKEN_MESSAGE,
                        "case {step:?}: expected the missing-token sentence, got {message:?}"
                    );
                }
                // A write that names a token stakes the version under exactly it.
                (Some(member), Some(token)) => {
                    assert_eq!(
                        status, 200,
                        "case {step:?}: expected 200 from {} with a token, got {status} with \
                         body {parsed}",
                        write.operation
                    );
                    assert_eq!(
                        text(&parsed, "VersionId", &step),
                        *token,
                        "case {step:?}: expected the token to become the version id, got {parsed}"
                    );
                    versions.push(token.clone());
                    current_value = (value, member);
                }
                // An update that stores no value creates no version, so it needs
                // no token and reports none.
                (None, _) => {
                    assert_eq!(
                        status, 200,
                        "case {step:?}: expected 200 from a metadata-only update, got {status} \
                         with body {parsed}"
                    );
                    assert!(
                        parsed.get("VersionId").is_none(),
                        "case {step:?}: expected no VersionId from an update that stored no \
                         value, got {parsed}"
                    );
                }
            }
        }

        // The invariant: the ids the secret holds are exactly the tokens its
        // accepted writes carried — no id the service invented, none the caller
        // chose missing.
        let stored = stored_version_ids(&router, &name, &case).await;
        let named: BTreeSet<String> = versions.iter().cloned().collect();
        assert_eq!(
            stored, named,
            "case {case:?}: expected the stored version ids to be exactly the tokens the \
             accepted writes named, got {stored:?}"
        );

        // AWSCURRENT is on the last version a caller named, AWSPREVIOUS on the one
        // before it, and every refused write left that labelling alone.
        let staged = staging_map(&router, &name, &case).await;
        assert_eq!(
            staged,
            expected_staging(&versions),
            "case {case:?}: expected AWSCURRENT on the last version its caller named, got \
             {staged:?}"
        );

        // So the read that names neither a version nor a stage resolves that
        // version, and the value its write stored.
        let read = ok(
            &router,
            "GetSecretValue",
            &json!({"SecretId": name.clone()}),
            &case,
        )
        .await;
        let current = versions
            .last()
            .expect("the created version is always named")
            .clone();
        assert_eq!(
            text(&read, "VersionId", &case),
            current,
            "case {case:?}: expected the last version its caller named to be current, got {read}"
        );
        let (value, member) = current_value;
        assert_eq!(
            read.get(member.member()),
            Some(&member.on_the_wire(&value)),
            "case {case:?}: expected the value of the last accepted write back in {}, got {read}",
            member.member()
        );
    }
}
