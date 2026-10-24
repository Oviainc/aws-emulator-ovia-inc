// SPDX-License-Identifier: CC0-1.0

//! `DeleteChallenge` answers for the record its ARN names, not for the call being retried.
//!
//! The rule under test: an operation of this service whose input identifies a resource by its
//! ARN answers `ResourceNotFoundException` (HTTP 404) — carrying that ARN as `ResourceId` and
//! the kind of resource it names as `ResourceType` — whenever no such record is stored at the
//! instant of the call, and succeeds only when one is. `@idempotent` says a retry is safe to
//! repeat, which constrains the EFFECT of the second call (it removes the same record and
//! nothing else), not the status of a call about a record that is not there.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The table's rows differ along what the rule
//! generalises over: HOW the ARN came to hold no record (never created, already deleted, the
//! owning connector torn down, a connector that never existed), the region and partition in
//! the ARN, the challenge id, and how many challenges the owning connector holds. The repeat
//! delete the issue was filed about is ONE row.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::state;

/// A region that is NOT the emulator default, so an ARN built from the request's own SigV4
/// credential scope is distinguishable from a hardcoded one.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// The `ResourceType` a challenge's `ResourceNotFoundException` names, from the service's own
/// vocabulary rather than respelled here.
const RESOURCE_TYPE_CHALLENGE: &str = state::RESOURCE_TYPE_CHALLENGE;

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
        "case {ca_arn:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ConnectorArn in {:?}", created.body))
        .to_string();
    release_pending(engine, &arn);
    arn
}

/// Mint one challenge password on `connector_arn` and answer its ARN.
async fn new_challenge(router: &MultiServiceRouter, connector_arn: &str) -> String {
    let created = create_challenge(router, json!({ "ConnectorArn": connector_arn })).await;
    assert_eq!(
        created.status, 202,
        "expected 202 from CreateChallenge on {connector_arn:?}, got {} / {:?}",
        created.status, created.body
    );
    created.body["Challenge"]["Arn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a challenge Arn in {:?}", created.body))
        .to_string()
}

/// A challenge ARN of exactly the form `ChallengeArn`'s own pattern requires
/// (`^arn:aws(-[a-z]+)*:pca-connector-scep:<region>:<account>:connector/<uuid>/challenge/<uuid>$`),
/// built from the parts a row varies.
fn challenge_arn(partition: &str, region: &str, connector_id: &str, challenge_id: &str) -> String {
    format!(
        "arn:{partition}:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}\
         /challenge/{challenge_id}"
    )
}

/// How the ARN under test came to hold no challenge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Missing {
    /// Nothing was ever created under the ARN.
    NeverCreated,
    /// A challenge was created under a live connector and then deleted.
    AlreadyDeleted,
    /// The challenge existed and its owning connector's teardown took it.
    ConnectorTornDown,
}

/// One row: a well-formed challenge ARN that holds no record, and how it got that way.
#[derive(Debug)]
struct MissingCase {
    name: &'static str,
    how: Missing,
    /// The partition in the ARN. The pattern allows `aws` and any `aws-<suffix>` form, so a
    /// row can vary it; only rows that create a record have to use the emulator's own.
    partition: &'static str,
    /// The region in the ARN, likewise.
    region: &'static str,
    /// The connector uuid for the rows that build the ARN by hand.
    connector_id: &'static str,
    /// The challenge uuid for those rows.
    challenge_id: &'static str,
    /// The CA the row's connector is created from, for the rows that create one.
    ca_arn: &'static str,
}

/// The whole rule, row by row: every well-formed challenge ARN that holds no record is a 404
/// `ResourceNotFoundException` naming that ARN and the `Challenge` resource type.
#[tokio::test]
async fn delete_challenge_refuses_an_arn_that_holds_no_challenge() {
    let cases = [
        MissingCase {
            name: "never created, under a connector that exists",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "22222222-3333-4444-5555-666666666666",
            ca_arn: CA_ARNS[0],
        },
        MissingCase {
            name: "never created, under a connector that never existed",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "11111111-1111-1111-1111-111111111111",
            challenge_id: "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
            ca_arn: CA_ARNS[1],
        },
        MissingCase {
            name: "never created, another partition",
            how: Missing::NeverCreated,
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
            challenge_id: "99999999-8888-7777-6666-555544443333",
            ca_arn: CA_ARNS[2],
        },
        MissingCase {
            name: "never created, another region",
            how: Missing::NeverCreated,
            partition: "aws",
            region: "ap-southeast-2",
            connector_id: "deadbeef-0000-1111-2222-333344445555",
            challenge_id: "abcdef01-2345-6789-abcd-ef0123456789",
            ca_arn: CA_ARNS[3],
        },
        MissingCase {
            name: "already deleted, so the delete is a repeat",
            how: Missing::AlreadyDeleted,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "",
            ca_arn: CA_ARNS[1],
        },
        MissingCase {
            name: "the owning connector's teardown took it",
            how: Missing::ConnectorTornDown,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "",
            ca_arn: CA_ARNS[2],
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine, case.ca_arn).await;

        let arn = match case.how {
            Missing::NeverCreated => {
                // Either under the connector that was just created, or under one the row names
                // that was never created at all.
                let connector_id = if case.connector_id.is_empty() {
                    connector
                        .rsplit('/')
                        .next()
                        .expect("the connector id in its own ARN")
                        .to_string()
                } else {
                    case.connector_id.to_string()
                };
                challenge_arn(
                    case.partition,
                    case.region,
                    &connector_id,
                    case.challenge_id,
                )
            }
            Missing::AlreadyDeleted => {
                let arn = new_challenge(&router, &connector).await;
                let first = delete_challenge(&router, &arn).await;
                assert_eq!(
                    (first.status, first.error_type.as_str()),
                    (202, ""),
                    "case {name:?}: the first DeleteChallenge must be accepted, got {} / {:?}",
                    first.status,
                    first.body
                );
                arn
            }
            Missing::ConnectorTornDown => {
                let arn = new_challenge(&router, &connector).await;
                let accepted = delete_connector(&router, &connector).await;
                assert_eq!(
                    accepted.status, 202,
                    "case {name:?}: expected 202 from DeleteConnector, got {} / {:?}",
                    accepted.status, accepted.body
                );
                // Let the clock make the teardown due: only then is the challenge gone.
                release_pending(&engine, &connector);
                let read = get_challenge_password(&router, &arn).await;
                assert_eq!(
                    read.status, 404,
                    "case {name:?}: the teardown must take the challenge with it, got {} / {:?}",
                    read.status, read.body
                );
                arn
            }
        };

        let answer = delete_challenge(&router, &arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?} on {arn:?}: expected 404 ResourceNotFoundException, got {} / {:?}",
            answer.status,
            answer.body
        );
        assert_eq!(
            answer.body["ResourceId"],
            json!(arn),
            "case {name:?}: the refusal must name the ARN it was asked for, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["ResourceType"],
            json!(RESOURCE_TYPE_CHALLENGE),
            "case {name:?}: expected ResourceType {RESOURCE_TYPE_CHALLENGE}, got {:?}",
            answer.body
        );

        // The refusal changed nothing: a second attempt answers exactly the same way, so the
        // 404 is a report about the state and not a one-shot.
        let repeat = delete_challenge(&router, &arn).await;
        assert_eq!(
            (repeat.status, repeat.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?}: the refusal must be stable across calls, got {} / {:?}",
            repeat.status,
            repeat.body
        );
    }
}

/// The success side of the same rule: a delete of an ARN that DOES hold a record is accepted,
/// removes exactly that record, and leaves every sibling readable — whatever position in the
/// connector's challenges it holds.
#[tokio::test]
async fn delete_challenge_removes_exactly_the_named_challenge() {
    // Rows differ in how many challenges the connector holds and which one is deleted: the
    // first, a middle one and the last.
    for (count, victim) in [(1usize, 0usize), (2, 1), (3, 1), (5, 0), (5, 4)] {
        let case = format!("{count} challenges, deleting #{victim}");
        let (router, engine) = new_router();
        let connector = active_connector(&router, &engine, CA_ARNS[count % CA_ARNS.len()]).await;

        let mut arns = Vec::new();
        for _ in 0..count {
            arns.push(new_challenge(&router, &connector).await);
        }

        let doomed = arns[victim].clone();
        let deleted = delete_challenge(&router, &doomed).await;
        assert_eq!(
            (deleted.status, deleted.error_type.as_str()),
            (202, ""),
            "case {case:?}: DeleteChallenge of a stored challenge must answer 202, got {} / {:?}",
            deleted.status,
            deleted.body
        );
        let gone = get_challenge_password(&router, &doomed).await;
        assert_eq!(
            (gone.status, gone.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted challenge must stop being readable, got {} / {:?}",
            gone.status,
            gone.body
        );

        // `@idempotent` constrains the EFFECT of a repeat, not its status: the record is gone
        // and stays gone, and nothing else was touched.
        let again = delete_challenge(&router, &doomed).await;
        assert_eq!(
            (again.status, again.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a repeat names no record, so it must answer 404, got {} / {:?}",
            again.status,
            again.body
        );

        for (index, sibling) in arns.iter().enumerate() {
            if index == victim {
                continue;
            }
            let read = get_challenge_password(&router, sibling).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: sibling #{index} {sibling:?} must survive the delete, got {} / \
                 {:?}",
                read.status, read.body
            );
            // And each sibling is itself deletable, once.
            let dropped = delete_challenge(&router, sibling).await;
            assert_eq!(
                dropped.status, 202,
                "case {case:?}: sibling #{index} must delete with 202, got {} / {:?}",
                dropped.status, dropped.body
            );
            let repeat = delete_challenge(&router, sibling).await;
            assert_eq!(
                repeat.status, 404,
                "case {case:?}: sibling #{index} is gone, so its repeat must answer 404, got {} / \
                 {:?}",
                repeat.status, repeat.body
            );
        }
    }
}

/// The 404 is reserved for an ARN that is WELL FORMED and holds no record: an ARN outside
/// `ChallengeArn`'s own `@length(5, 200)` and pattern is a 400 `ValidationException` instead,
/// so a caller can tell "you asked for something that is not there" from "that is not an ARN".
#[tokio::test]
async fn a_malformed_challenge_arn_is_a_validation_failure_not_a_404() {
    let (router, _engine) = new_router();
    let long_id = "a".repeat(200);
    let cases: Vec<(&str, String)> = vec![
        ("not an ARN at all", "challenge".to_string()),
        (
            "a connector ARN with no challenge segment",
            format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/11111111-1111-1111-1111-111111111111"),
        ),
        (
            "a challenge id that is not hexadecimal",
            challenge_arn(
                "aws",
                REGION,
                "11111111-1111-1111-1111-111111111111",
                "gggggggg-3333-4444-5555-666666666666",
            ),
        ),
        (
            "a challenge id missing a group",
            challenge_arn(
                "aws",
                REGION,
                "11111111-1111-1111-1111-111111111111",
                "22222222-3333-4444-666666666666",
            ),
        ),
        // One past `@length`'s upper bound of 200 characters.
        (
            "longer than the published length bound",
            challenge_arn(
                "aws",
                REGION,
                "11111111-1111-1111-1111-111111111111",
                &long_id,
            ),
        ),
    ];

    for (case, arn) in &cases {
        let answer = delete_challenge(&router, arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?} on {arn:?}: expected 400 ValidationException, got {} / {:?}",
            answer.status,
            answer.body
        );
    }
}
