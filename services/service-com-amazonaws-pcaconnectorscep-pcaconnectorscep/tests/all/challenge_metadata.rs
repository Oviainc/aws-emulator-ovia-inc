// SPDX-License-Identifier: CC0-1.0

//! `GetChallengeMetadata` reports the stored record its ARN names.
//!
//! The rule under test: a single-resource read of this service answers, for the ARN it is
//! given, exactly the record that is stored under that ARN at the instant of the call — the
//! same values the create returned and the same values the listing reports — carries no member
//! the read's own output shape does not model (so no `@sensitive` password), changes nothing a
//! later caller sees, and answers `ResourceNotFoundException` (HTTP 404) naming that ARN and
//! its resource type whenever no record is stored under it.
//!
//! Every case drives the service the way a client does, one HTTP request per call through the
//! same `MultiServiceRouter` the emulator boots. The table's rows differ along what the rule
//! generalises over: how many challenges the owning connector holds and which of them is read,
//! whether the challenge carries tags, and whether the connector serves publicly or only
//! through a VPC endpoint.

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

/// Private-CA ARNs matching `CertificateAuthorityArn`'s own pattern
/// (`^arn:aws(-[a-z]+)*:acm-pca:<region>:<account>:certificate-authority/<uuid>$`).
const CA_ARNS: &[&str] = &[
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/11111111-2222-3333-4444-555555555555",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
    "arn:aws:acm-pca:eu-west-2:000000000000:certificate-authority/0123456f-89ab-cdef-0123-456789abcdef",
];

/// The four members `ChallengeMetadata` models (smithy_query: shape ChallengeMetadata) —
/// nothing else may appear in the response's `ChallengeMetadata` object, and in particular not
/// the `@sensitive` `Password` that only `GetChallengePassword` returns.
const METADATA_MEMBERS: &[&str] = &["Arn", "ConnectorArn", "CreatedAt", "UpdatedAt"];

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

async fn get_challenge_metadata(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/challengeMetadata/{}", label(arn)),
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

async fn list_challenge_metadata(router: &MultiServiceRouter, connector_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/challengeMetadata?ConnectorArn={}", label(connector_arn)),
        None,
    )
    .await
}

/// Make the transition the clock owes `arn` due, so the next call materialises it. Strictly a
/// clock move through the service's own state: nothing else on the record is touched, and no
/// test sleeps for `TRANSITION_SPAN`.
fn release_pending(engine: &engine::InMemoryEngine, arn: &str) {
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
    pending.due_at = SystemTime::now() - Duration::from_secs(1);
}

/// A connector that is serving, so it can mint challenge passwords.
async fn active_connector(
    router: &MultiServiceRouter,
    engine: &engine::InMemoryEngine,
    request: Value,
) -> String {
    let created = create_connector(router, request.clone()).await;
    assert_eq!(
        created.status, 202,
        "case {request:?}: expected 202 from CreateConnector, got {} / {:?}",
        created.status, created.body
    );
    let arn = created.body["ConnectorArn"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a ConnectorArn in {:?}", created.body))
        .to_string();
    release_pending(engine, &arn);
    arn
}

/// A challenge ARN of exactly the form `ChallengeArn`'s own pattern requires, built from the
/// parts a row varies.
fn challenge_arn(partition: &str, region: &str, connector_id: &str, challenge_id: &str) -> String {
    format!(
        "arn:{partition}:pca-connector-scep:{region}:{ACCOUNT}:connector/{connector_id}\
         /challenge/{challenge_id}"
    )
}

/// One row of the success table.
#[derive(Debug)]
struct ReadCase {
    name: &'static str,
    /// How many challenges the owning connector holds.
    count: usize,
    /// Which of them the read names: the first, a middle one, the last.
    index: usize,
    /// Tags the read challenge was created with, if any — metadata models no tag member, so
    /// they must not show up in the answer either way.
    tags: Option<Value>,
    /// Whether the connector was created restricted to a VPC endpoint: a different access path
    /// for the same read.
    vpc_endpoint_id: Option<&'static str>,
}

/// The whole rule, row by row: the read answers the stored record, exactly the members its
/// output shape models, agreeing with both the create and the listing.
#[tokio::test]
async fn get_challenge_metadata_reports_the_stored_challenge() {
    let cases = [
        ReadCase {
            name: "the only challenge of a connector",
            count: 1,
            index: 0,
            tags: None,
            vpc_endpoint_id: None,
        },
        ReadCase {
            name: "the first of three",
            count: 3,
            index: 0,
            tags: None,
            vpc_endpoint_id: None,
        },
        ReadCase {
            name: "a middle one of five",
            count: 5,
            index: 2,
            tags: Some(json!({ "team": "pki", "env": "test" })),
            vpc_endpoint_id: None,
        },
        ReadCase {
            name: "the last of four",
            count: 4,
            index: 3,
            tags: None,
            vpc_endpoint_id: Some("vpce-0123456789abcdef0"),
        },
        ReadCase {
            name: "a tagged challenge on a VPC-restricted connector",
            count: 2,
            index: 1,
            tags: Some(json!({ "owner": "scep" })),
            vpc_endpoint_id: Some("vpce-00000000000000001"),
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, engine) = new_router();
        let mut connector_request = json!({
            "CertificateAuthorityArn": CA_ARNS[case.count % CA_ARNS.len()],
        });
        if let Some(vpce) = case.vpc_endpoint_id {
            connector_request["VpcEndpointId"] = json!(vpce);
        }
        let connector = active_connector(&router, &engine, connector_request).await;

        // Create every challenge the row asks for, remembering what each create answered: the
        // read must agree with it member for member.
        let mut created = Vec::new();
        for position in 0..case.count {
            let mut request = json!({ "ConnectorArn": connector });
            if position == case.index {
                if let Some(tags) = &case.tags {
                    request["Tags"] = tags.clone();
                }
            }
            let answer = create_challenge(&router, request).await;
            assert_eq!(
                answer.status, 202,
                "case {name:?}: expected 202 from CreateChallenge #{position}, got {} / {:?}",
                answer.status, answer.body
            );
            created.push(answer.body["Challenge"].clone());
        }

        let subject = &created[case.index];
        let arn = subject["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: expected a challenge Arn in {subject:?}"))
            .to_string();

        let read = get_challenge_metadata(&router, &arn).await;
        assert_eq!(
            (read.status, read.error_type.as_str()),
            (200, ""),
            "case {name:?}: expected 200 from GetChallengeMetadata, got {} / {:?}",
            read.status,
            read.body
        );
        let metadata = &read.body["ChallengeMetadata"];

        // Every modelled member reports what the create returned for it.
        for member in METADATA_MEMBERS {
            assert_eq!(
                metadata[member], subject[member],
                "case {name:?}: {member} must be the stored value {:?}, got {:?}",
                subject[member], metadata[member]
            );
            assert!(
                !metadata[member].is_null(),
                "case {name:?}: {member} must be reported, got {:?}",
                metadata
            );
        }
        assert_eq!(
            metadata["Arn"],
            json!(arn),
            "case {name:?}: the read must report the ARN it was asked for, got {metadata:?}"
        );
        assert_eq!(
            metadata["ConnectorArn"],
            json!(connector),
            "case {name:?}: expected the owning connector {connector:?}, got {metadata:?}"
        );

        // Exactly the members the output shape models, and nothing else: no password, no tags.
        let object = metadata
            .as_object()
            .unwrap_or_else(|| panic!("case {name:?}: expected an object, got {metadata:?}"));
        let mut unexpected: Vec<&String> = object
            .keys()
            .filter(|key| !METADATA_MEMBERS.contains(&key.as_str()))
            .collect();
        unexpected.sort();
        assert!(
            unexpected.is_empty(),
            "case {name:?}: ChallengeMetadata models only {METADATA_MEMBERS:?}, got the extra \
             members {unexpected:?} in {metadata:?}"
        );
        assert_eq!(
            read.body.as_object().map(|body| body.len()),
            Some(1),
            "case {name:?}: the response carries only ChallengeMetadata, got {:?}",
            read.body
        );

        // The read agrees with the listing of the same connector, entry for entry.
        let listed = list_challenge_metadata(&router, &connector).await;
        assert_eq!(
            listed.status, 200,
            "case {name:?}: expected 200 from ListChallengeMetadata, got {} / {:?}",
            listed.status, listed.body
        );
        let entries = listed.body["Challenges"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected Challenges in {:?}", listed.body))
            .clone();
        assert_eq!(
            entries.len(),
            case.count,
            "case {name:?}: expected {} listed challenges, got {}",
            case.count,
            entries.len()
        );
        let entry = entries
            .iter()
            .find(|entry| entry["Arn"] == json!(arn))
            .unwrap_or_else(|| panic!("case {name:?}: {arn:?} must be listed, got {entries:?}"));
        for member in METADATA_MEMBERS {
            assert_eq!(
                metadata[member], entry[member],
                "case {name:?}: {member} must match the listing's {:?}, got {:?}",
                entry[member], metadata[member]
            );
        }

        // A read changes nothing a later caller sees: the second read answers identically, the
        // password is still readable, and every sibling still reports its own record.
        let again = get_challenge_metadata(&router, &arn).await;
        assert_eq!(
            (again.status, again.body.clone()),
            (read.status, read.body.clone()),
            "case {name:?}: a repeat read must answer identically, got {} / {:?}",
            again.status,
            again.body
        );
        let password = get_challenge_password(&router, &arn).await;
        assert_eq!(
            password.status, 200,
            "case {name:?}: the password must stay readable after a metadata read, got {} / {:?}",
            password.status, password.body
        );
        for (position, sibling) in created.iter().enumerate() {
            let sibling_arn = sibling["Arn"].as_str().expect("a sibling Arn");
            let sibling_read = get_challenge_metadata(&router, sibling_arn).await;
            assert_eq!(
                sibling_read.status, 200,
                "case {name:?}: sibling #{position} {sibling_arn:?} must be readable, got {} / \
                 {:?}",
                sibling_read.status, sibling_read.body
            );
            assert_eq!(
                sibling_read.body["ChallengeMetadata"]["Arn"],
                json!(sibling_arn),
                "case {name:?}: sibling #{position} must report its OWN ARN, got {:?}",
                sibling_read.body
            );
        }
    }
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
    /// The partition in the ARN. `ChallengeArn`'s pattern allows `aws` and any `aws-<suffix>`
    /// form, so a row can vary it; only rows that create a record use the emulator's own.
    partition: &'static str,
    region: &'static str,
    connector_id: &'static str,
    challenge_id: &'static str,
}

/// The refusal side of the same rule: every well-formed challenge ARN that holds no record is a
/// 404 `ResourceNotFoundException` naming that ARN and the `Challenge` resource type.
#[tokio::test]
async fn get_challenge_metadata_refuses_an_arn_that_holds_no_challenge() {
    let cases = [
        MissingCase {
            name: "never created, under a connector that exists",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "22222222-3333-4444-5555-666666666666",
        },
        MissingCase {
            name: "never created, under a connector that never existed",
            how: Missing::NeverCreated,
            partition: "aws",
            region: REGION,
            connector_id: "11111111-1111-1111-1111-111111111111",
            challenge_id: "ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb",
        },
        MissingCase {
            name: "never created, another partition",
            how: Missing::NeverCreated,
            partition: "aws-us-gov",
            region: "us-gov-west-1",
            connector_id: "0123456f-89ab-cdef-0123-456789abcdef",
            challenge_id: "99999999-8888-7777-6666-555544443333",
        },
        MissingCase {
            name: "never created, another region",
            how: Missing::NeverCreated,
            partition: "aws",
            region: "ap-southeast-2",
            connector_id: "deadbeef-0000-1111-2222-333344445555",
            challenge_id: "abcdef01-2345-6789-abcd-ef0123456789",
        },
        MissingCase {
            name: "already deleted",
            how: Missing::AlreadyDeleted,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "",
        },
        MissingCase {
            name: "the owning connector's teardown took it",
            how: Missing::ConnectorTornDown,
            partition: "aws",
            region: REGION,
            connector_id: "",
            challenge_id: "",
        },
    ];

    for case in &cases {
        let name = case.name;
        let (router, engine) = new_router();
        let connector = active_connector(
            &router,
            &engine,
            json!({ "CertificateAuthorityArn": CA_ARNS[0] }),
        )
        .await;

        let arn = match case.how {
            Missing::NeverCreated => {
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
                let created =
                    create_challenge(&router, json!({ "ConnectorArn": connector })).await;
                let arn = created.body["Challenge"]["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: expected an Arn in {:?}", created.body))
                    .to_string();
                let dropped = delete_challenge(&router, &arn).await;
                assert_eq!(
                    dropped.status, 202,
                    "case {name:?}: the delete must be accepted, got {} / {:?}",
                    dropped.status, dropped.body
                );
                arn
            }
            Missing::ConnectorTornDown => {
                let created =
                    create_challenge(&router, json!({ "ConnectorArn": connector })).await;
                let arn = created.body["Challenge"]["Arn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: expected an Arn in {:?}", created.body))
                    .to_string();
                let accepted = delete_connector(&router, &connector).await;
                assert_eq!(
                    accepted.status, 202,
                    "case {name:?}: expected 202 from DeleteConnector, got {} / {:?}",
                    accepted.status, accepted.body
                );
                // Let the clock make the teardown due: only then is the challenge gone.
                release_pending(&engine, &connector);
                arn
            }
        };

        let answer = get_challenge_metadata(&router, &arn).await;
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
            json!(state::RESOURCE_TYPE_CHALLENGE),
            "case {name:?}: expected ResourceType {}, got {:?}",
            state::RESOURCE_TYPE_CHALLENGE,
            answer.body
        );

        // The refusal reports the state rather than consuming anything: it is stable.
        let repeat = get_challenge_metadata(&router, &arn).await;
        assert_eq!(
            (repeat.status, repeat.error_type.as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?}: the refusal must be stable across calls, got {} / {:?}",
            repeat.status,
            repeat.body
        );
    }
}

/// The 404 is reserved for an ARN that is WELL FORMED and holds no record: an ARN outside
/// `ChallengeArn`'s own `@length(5, 200)` and pattern is a 400 `ValidationException` instead.
/// The length rows sit on each published bound and one past it.
#[tokio::test]
async fn a_malformed_challenge_arn_is_a_validation_failure_not_a_404() {
    let (router, _engine) = new_router();
    // One past `@length`'s upper bound of 200 characters, and one short of its lower bound of 5.
    let too_long = format!("arn:{}", "a".repeat(197));
    let too_short = "arn:";
    let cases: Vec<(&str, String)> = vec![
        ("not an ARN at all", "challenge".to_string()),
        ("shorter than the published length bound", too_short.to_string()),
        (
            "a connector ARN with no challenge segment",
            format!(
                "arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/11111111-1111-1111-1111-111111111111"
            ),
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
        ("longer than the published length bound", too_long),
    ];

    for (case, arn) in &cases {
        assert!(
            arn.len() <= 200 || arn.len() == 201,
            "case {case:?}: the row must sit at or just past the published bound, got {}",
            arn.len()
        );
        let answer = get_challenge_metadata(&router, arn).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?} on {arn:?}: expected 400 ValidationException, got {} / {:?}",
            answer.status,
            answer.body
        );
    }
}
