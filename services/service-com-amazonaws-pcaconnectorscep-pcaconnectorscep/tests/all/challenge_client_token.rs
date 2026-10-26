// SPDX-License-Identifier: CC0-1.0

//! A `CreateChallenge` retry carrying a used `ClientToken` is not exempt from the call's own
//! preconditions.
//!
//! The rule under test: an idempotency token answers with the resource the call carrying it
//! created only while that resource could still be created now — it exists and the connector
//! it lives under is in the one status that lets it serve. Otherwise the token binds nothing
//! and the retry is answered exactly as the SAME request carrying a token nobody has used
//! yet: refused where a fresh call is refused, served where a fresh call is served.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The table's rows differ along what the rule
//! generalises over: what happened to the token's challenge or to its connector between the
//! first call and the retry (nothing, the connector accepted for teardown, the teardown
//! complete, the challenge itself deleted), the shape of the token, and the CA the connector
//! was created from. The connector accepted for teardown — the row the issue was filed about
//! — is ONE row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The status a connector carries while its teardown runs, named from the service's own
/// constant rather than respelled here.
const STATUS_DELETING: &str = state::STATUS_DELETING;

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`). Rows name
/// different CAs, so no assertion can pass by echoing one recorded value.
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/deadbeef-0000-1111-2222-333344445555",
];

/// What one call answered: the HTTP status, the modelled error code (empty on success) and the
/// parsed body.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: String,
    body: Value,
}

impl Answer {
    /// One member of the `Challenge` this answer carries, or `None` when it carries none.
    fn challenge(&self, member: &str) -> Option<&str> {
        self.body["Challenge"][member].as_str()
    }
}

fn new_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's own concern and is exercised elsewhere.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    (router, engine)
}

/// Percent-encode a value for one `@httpLabel` path segment, the way an AWS SDK does, so the
/// `:` and `/` inside an ARN cannot read as path structure.
fn label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

async fn call(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    let payload = match body {
        Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
        None => bytes::Bytes::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://pca-connector-scep.{REGION}.amazonaws.com{path}"
        ))
        // The credential scope is how a handler learns the region its ARNs name.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/pca-connector-scep/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(['/', ':'])
                .next()
                .unwrap_or(value)
                .trim()
                .to_string()
        })
        .unwrap_or_default();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

async fn create_connector(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/connectors", Some(request)).await
}

async fn get_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("/connectors/{}", label(arn)), None).await
}

async fn delete_connector(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/connectors/{}", label(arn)),
        None,
    )
    .await
}

async fn create_challenge(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/challenges", Some(request)).await
}

async fn delete_challenge(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/challenges/{}", label(arn)),
        None,
    )
    .await
}

async fn get_challenge_password(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/challengePasswords/{}", label(arn)),
        None,
    )
    .await
}

/// Move the instant the clock owes `arn` its pending transition, through the service's own
/// state, so a case that is not about the transition neither sleeps for nor races
/// `TRANSITION_SPAN`. Strictly a clock move: nothing else on the record is touched.
fn set_transition_due(engine: &engine::InMemoryEngine, arn: &str, due_at: SystemTime) {
    let shared = state::service_state(engine).expect("the service's own state");
    let mut store = shared.lock();
    let connector = store
        .connectors
        .get_mut(arn)
        .unwrap_or_else(|| panic!("the connector {arn:?} that was just created"));
    let pending = connector
        .pending
        .as_mut()
        .unwrap_or_else(|| panic!("the transition the clock owes {arn:?}"));
    pending.due_at = due_at;
}

/// Keep the transition the clock owes `arn` undue for the rest of the case.
fn hold_pending(engine: &engine::InMemoryEngine, arn: &str) {
    set_transition_due(engine, arn, SystemTime::now() + Duration::from_secs(3600));
}

/// Make the transition the clock owes `arn` due, so the next call materialises it.
fn release_pending(engine: &engine::InMemoryEngine, arn: &str) {
    set_transition_due(engine, arn, SystemTime::now() - Duration::from_secs(1));
}

/// A connector that is serving, so it can mint challenge passwords.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    ca_arn: &str,
) -> String {
    let created = create_connector(router, json!({ "CertificateAuthorityArn": ca_arn })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateConnector for {ca_arn:?}, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ConnectorArn in {:?}", created.body))
        .to_string();
    release_pending(engine, &arn);
    arn
}

/// What happens to the token's challenge, or to the connector it lives under, between the
/// first `CreateChallenge` and the retry carrying the same token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Between {
    /// Nothing: the connector is still serving and still holds the challenge.
    Nothing,
    /// `DeleteConnector` is accepted and the teardown window is held open, so the connector
    /// still reports `DELETING` and still holds the challenge record.
    ConnectorDeleting,
    /// The teardown has come due: the connector and the challenges it owned are gone.
    ConnectorGone,
    /// `DeleteChallenge` removes the token's challenge; its connector keeps serving.
    ChallengeDeleted,
}

/// What the retry must be answered with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// 202 with the very challenge the token created, password included.
    SameChallenge,
    /// 202 with a challenge the token does not bind yet — the token answered for nothing, so
    /// the call was served as a first one.
    FreshChallenge,
    /// The refusal a first call with those inputs gets: a status, the modelled error code and
    /// the `Reason` the service names (`None` when the error carries none).
    Refused(u16, &'static str, Option<&'static str>),
}

/// One row of the table.
#[derive(Debug)]
struct Case {
    name: &'static str,
    /// `ClientToken` is `@length(1, 64)` with pattern `^[!-~]+$` — any printable ASCII run.
    token: &'static str,
    ca_arn: &'static str,
    between: Between,
    expected: Expected,
}

/// The whole rule, row by row.
#[tokio::test]
async fn a_client_token_answers_only_while_its_challenge_can_still_be_created() {
    let cases = [
        Case {
            name: "connector still serving",
            token: "11111111-2222-3333-4444-555555555555",
            ca_arn: CA_ARNS[0],
            between: Between::Nothing,
            expected: Expected::SameChallenge,
        },
        Case {
            name: "connector accepted for teardown",
            token: "retry-token-one",
            ca_arn: CA_ARNS[1],
            between: Between::ConnectorDeleting,
            // What the same request with an unused token gets: the connector is not serving,
            // so it cannot hand a challenge password out.
            expected: Expected::Refused(400, "ValidationException", Some("INVALID_STATE")),
        },
        Case {
            name: "connector torn down",
            token: "!~-_.+",
            ca_arn: CA_ARNS[2],
            between: Between::ConnectorGone,
            expected: Expected::Refused(404, "ResourceNotFoundException", None),
        },
        Case {
            name: "challenge deleted, connector serving",
            // The published upper bound of `ClientToken`'s `@length(1, 64)`.
            token: "t0kent0kent0kent0kent0kent0kent0kent0kent0kent0kent0kent0kent0ke",
            ca_arn: CA_ARNS[3],
            between: Between::ChallengeDeleted,
            expected: Expected::FreshChallenge,
        },
    ];

    for case in &cases {
        let name = case.name;
        assert!(
            (1..=64).contains(&case.token.len()),
            "case {name:?}: the row's token must stay inside ClientToken's modelled length, \
             got {} bytes",
            case.token.len()
        );

        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine, case.ca_arn).await;
        let request = json!({ "ConnectorArn": connector.clone(), "ClientToken": case.token });

        let created = create_challenge(&router, request.clone()).await;
        assert_eq!(
            created.status, 202,
            "case {name:?}: expected 202 from the first CreateChallenge, got {} / {:?}",
            created.status, created.body
        );
        let challenge_arn = created
            .challenge("Arn")
            .unwrap_or_else(|| panic!("case {name:?}: expected a challenge Arn in {:?}", created.body))
            .to_string();
        let password = created
            .challenge("Password")
            .unwrap_or_else(|| {
                panic!("case {name:?}: expected a challenge Password in {:?}", created.body)
            })
            .to_string();

        // Whatever the row says happens to the challenge or its connector before the retry.
        match case.between {
            Between::Nothing => {}
            Between::ConnectorDeleting | Between::ConnectorGone => {
                let deleted = delete_connector(&router, &connector).await;
                assert_eq!(
                    deleted.status, 202,
                    "case {name:?}: expected 202 from DeleteConnector, got {} / {:?}",
                    deleted.status, deleted.body
                );
                if case.between == Between::ConnectorDeleting {
                    // Pin the teardown window rather than race TRANSITION_SPAN, and check the
                    // row really is about a connector that is still reported.
                    hold_pending(&engine, &connector);
                    let reported = get_connector(&router, &connector).await;
                    assert_eq!(
                        reported.body["Connector"]["Status"],
                        json!(STATUS_DELETING),
                        "case {name:?}: the row needs the connector to report {STATUS_DELETING}, \
                         got {:?}",
                        reported.body
                    );
                } else {
                    release_pending(&engine, &connector);
                    let gone = get_connector(&router, &connector).await;
                    assert_eq!(
                        gone.status, 404,
                        "case {name:?}: the row needs the teardown to have completed, got {} / \
                         {:?}",
                        gone.status, gone.body
                    );
                }
            }
            Between::ChallengeDeleted => {
                let deleted = delete_challenge(&router, &challenge_arn).await;
                assert_eq!(
                    deleted.status, 202,
                    "case {name:?}: expected 202 from DeleteChallenge, got {} / {:?}",
                    deleted.status, deleted.body
                );
            }
        }

        // The retry: the same inputs, the same token.
        let retried = create_challenge(&router, request.clone()).await;

        match case.expected {
            Expected::SameChallenge => {
                assert_eq!(
                    (retried.status, retried.error_type.as_str()),
                    (202, ""),
                    "case {name:?}: the retry must be answered 202, got {} / {:?}",
                    retried.status,
                    retried.body
                );
                assert_eq!(
                    retried.challenge("Arn"),
                    Some(challenge_arn.as_str()),
                    "case {name:?}: the token must answer with the challenge it created, got {:?}",
                    retried.body
                );
                assert_eq!(
                    retried.challenge("Password"),
                    Some(password.as_str()),
                    "case {name:?}: the retry must carry the password the first call returned, \
                     got {:?}",
                    retried.body
                );
                // And that is still what the reader answers for that ARN.
                let read = get_challenge_password(&router, &challenge_arn).await;
                assert_eq!(
                    read.body["Password"],
                    json!(password),
                    "case {name:?}: GetChallengePassword must read back the same password, got \
                     {:?}",
                    read.body
                );
            }
            Expected::FreshChallenge => {
                assert_eq!(
                    (retried.status, retried.error_type.as_str()),
                    (202, ""),
                    "case {name:?}: the retry must be served like a first call, got {} / {:?}",
                    retried.status,
                    retried.body
                );
                let minted = retried
                    .challenge("Arn")
                    .unwrap_or_else(|| {
                        panic!("case {name:?}: expected a challenge Arn in {:?}", retried.body)
                    })
                    .to_string();
                assert_ne!(
                    minted, challenge_arn,
                    "case {name:?}: a token binding nothing must mint a new challenge, got \
                     {minted:?}"
                );
                assert_ne!(
                    retried.challenge("Password"),
                    Some(password.as_str()),
                    "case {name:?}: a newly minted challenge must carry its own password, got \
                     {:?}",
                    retried.body
                );
                // The ARN the retry answered with really resolves.
                let read = get_challenge_password(&router, &minted).await;
                assert_eq!(
                    (read.status, read.body["Password"].clone()),
                    (200, json!(retried.challenge("Password"))),
                    "case {name:?}: the challenge the retry answered with must read back, got {} \
                     / {:?}",
                    read.status,
                    read.body
                );
            }
            Expected::Refused(status, error_type, reason) => {
                assert_eq!(
                    (retried.status, retried.error_type.as_str()),
                    (status, error_type),
                    "case {name:?}: the retry must be refused like a first call, got {} / {:?}",
                    retried.status,
                    retried.body
                );
                assert_eq!(
                    retried.body["Reason"],
                    reason.map_or(Value::Null, |reason| json!(reason)),
                    "case {name:?}: expected Reason {reason:?}, got {:?}",
                    retried.body
                );
                // The refusal carries no challenge, and above all no password.
                assert_eq!(
                    retried.body["Challenge"],
                    Value::Null,
                    "case {name:?}: a refused retry must carry no Challenge, got {:?}",
                    retried.body
                );
                assert!(
                    !retried.body.to_string().contains(password.as_str()),
                    "case {name:?}: a refused retry must not hand out the challenge password, \
                     got {:?}",
                    retried.body
                );
            }
        }

        // The retry and the SAME request carrying a token nobody has used are answered the
        // same way: the token changes only WHICH challenge is answered with, never whether
        // the call is served.
        let fresh = create_challenge(
            &router,
            json!({ "ConnectorArn": connector.clone(), "ClientToken": format!("{}x", &case.token[1..]) }),
        )
        .await;
        assert_eq!(
            (retried.status, retried.error_type.as_str()),
            (fresh.status, fresh.error_type.as_str()),
            "case {name:?}: a used token and a fresh one must be answered alike, got {} / {:?} \
             against {} / {:?}",
            retried.status,
            retried.body,
            fresh.status,
            fresh.body
        );
    }
}
