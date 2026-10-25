// SPDX-License-Identifier: CC0-1.0

//! `Sample` and `SampleWithResponseStream` are ONE path through the store.
//!
//! RULE: both sampling operations run the same pre-flight in the same order and
//! capture the turn through the same store call, so for one state and one request
//! they answer with the same status, the same error code and the same sentence;
//! what differs between them is only the framing of a served body — the
//! `Content-Type` it carries and whether the captured turn is recorded as
//! streamed. The two operations model the same errors
//! (`smithy_query(query="operation",operation="Sample,SampleWithResponseStream")`
//! — both bind `ValidationException` 400 and `ServiceQuotaExceededException` 402
//! and neither binds a `ConflictException`), and the service's own docs say a
//! single trajectory can mix turns served by either of them.
//!
//! The rows span what the rule generalises over: WHICH shared rule answers the
//! call (no inference payload, a `JobArn` that resolves to no created job, the
//! trajectory's own sealed state, an emulator bound), the form and size of the
//! payload (empty, chat-completions JSON, non-JSON, larger than one turn may
//! retain), the terminal status a trajectory was sealed with, and the case where
//! the call is served. Every row is run twice — once through `/sample`, once
//! through `/sample-with-response-stream`, each on its own engine — and the two
//! answers are compared with each other as well as against the rule.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the
//! store is read afterwards the way the NEXT request would read it.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService;
use service_common::agentic_rft_jobs::{JobDirectory, JobRecord};
use smithy_server_runtime::MultiServiceRouter;

/// This service's own endpoint host, built from the model's endpoint prefix.
const HOST: &str = "job-runtime.sagemaker.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`sagemaker`). Request authentication is off on the test router, so the signature
/// itself is never checked.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20240101/us-east-1/sagemaker/aws4_request, SignedHeaders=host, \
     Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The headers `JobArn` and `TrajectoryId` are bound to.
const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";
const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// The two sampling paths this model binds.
const SAMPLE_PATH: &str = "/sample";
const SAMPLE_STREAM_PATH: &str = "/sample-with-response-stream";

/// One HTTP answer, reduced to what this suite asserts on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Reply {
    /// The bare error code out of `x-amzn-errortype`, which may carry a namespace or a
    /// shape-id suffix alongside the name.
    fn error_code(&self) -> String {
        let raw = self.error_type.clone().unwrap_or_default();
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// The `Message` member of a restJson1 error document, read under the name the MODEL
    /// declares (`Message`) and no other.
    fn message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| {
                body.get("Message")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A router carrying exactly this service over one shared engine. The engine comes back
/// too, so a test can create a job the way the control plane does and read this service's
/// store the way the next request would.
fn mounted_service() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router
        .set_request_authentication(false)
        .register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// Create the job `job_arn` names in the shared job directory — the registration the
/// SageMaker control plane's create writes, which is what makes a job addressable in this
/// data plane. This service binds no operation that creates a job.
fn create_job(engine: &engine::InMemoryEngine, job_arn: &str) {
    let resource = ::service_common::arn::resource_id_of_type(job_arn, "job")
        .expect("a job ARN naming a job resource");
    let (category, name) = resource
        .split_once('/')
        .expect("a job resource of the form <category>/<name>");
    engine.state::<JobDirectory>().lock().register(JobRecord {
        name: name.to_string(),
        category: category.to_string(),
        arn: job_arn.to_string(),
    });
}

async fn send(
    router: &MultiServiceRouter,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Reply {
    let mut request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{HOST}{path}"))
        .header("authorization", AUTHORIZATION)
        .header("host", HOST);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request
        .body(bytes::Bytes::from(body))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    };
    let error_type = header("x-amzn-errortype");
    let content_type = header("content-type");
    Reply {
        status,
        error_type,
        content_type,
        body: response.into_body().to_vec(),
    }
}

/// One sampling call on `path`.
async fn sample(
    router: &MultiServiceRouter,
    path: &str,
    job_arn: &str,
    trajectory_id: &str,
    payload: &[u8],
) -> Reply {
    send(
        router,
        path,
        &[
            (JOB_ARN_HEADER, job_arn),
            (TRAJECTORY_ID_HEADER, trajectory_id),
            ("content-type", "application/json"),
        ],
        payload.to_vec(),
    )
    .await
}

/// One `CompleteRollout` call, which is what seals a trajectory.
async fn complete_rollout(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    status: &str,
) -> Reply {
    let body = json!({
        "TrajectoryId": trajectory_id,
        "Status": status,
        "ClientToken": "seal-token",
    });
    send(
        router,
        "/complete-rollout",
        &[
            (JOB_ARN_HEADER, job_arn),
            ("content-type", "application/json"),
        ],
        serde_json::to_vec(&body).expect("a serializable body"),
    )
    .await
}

/// An OpenAI-compatible chat-completions request body.
fn chat_payload(text: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": text }] }))
        .expect("a serializable payload")
}

/// A job ARN assembled from the parts the modelled pattern lets vary.
fn job_arn(partition: &str, region: &str, account: &str, category: &str, name: &str) -> String {
    format!("arn:{partition}:sagemaker:{region}:{account}:job/{category}/{name}")
}

/// How many turns the trajectory holds, read the way the next request would read it.
fn captured_turns(engine: &engine::InMemoryEngine, job_arn: &str, trajectory_id: &str) -> usize {
    let store = engine.state::<ServiceState>();
    let guard = store.lock();
    guard
        .trajectory(job_arn, trajectory_id)
        .map_or(0, |trajectory| trajectory.turns.len())
}

/// The state a row puts the emulator in before the sampling call under test.
enum Setup {
    /// The job the call names was never created.
    NoJob,
    /// The job exists and the trajectory holds no turn yet.
    JobOnly,
    /// The job exists, the trajectory holds one turn and `CompleteRollout` sealed
    /// it with this terminal status.
    Sealed(&'static str),
    /// The job exists and the trajectory already holds the per-trajectory turn
    /// bound, seeded rather than driven through a thousand requests.
    AtTurnBound,
}

/// What both operations must answer for a row.
enum Expected {
    /// The call is served: 200, and the body is this operation's own framing.
    Served,
    /// The call is refused, with this status, error code and sentence.
    Refused {
        status: u16,
        code: &'static str,
        message: String,
    },
}

/// One row: a state, a payload, and the one answer both sampling paths owe it.
struct Case {
    case: &'static str,
    setup: Setup,
    job_arn: String,
    trajectory_id: String,
    payload: Vec<u8>,
    expected: Expected,
}

/// The ARN a row addresses when the job is NOT created, kept distinct from the
/// created one so the refusal names the value the request carried.
fn uncreated_arn() -> String {
    job_arn(
        "aws",
        "eu-central-1",
        "210987654321",
        "AgentRFTEvaluation",
        "never-created",
    )
}

fn cases() -> Vec<Case> {
    let created = job_arn("aws", "us-east-1", "123456789012", "AgentRFT", "run-1");
    let missing = uncreated_arn();
    vec![
        Case {
            case: "an empty payload carries no inference request",
            setup: Setup::JobOnly,
            job_arn: created.clone(),
            trajectory_id: "traj-empty".to_string(),
            payload: Vec::new(),
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::EMPTY_BODY_MESSAGE.to_string(),
            },
        },
        Case {
            // Both operations check the payload BEFORE resolving the job, so a
            // call that fails both rules is reported the same way by each.
            case: "an empty payload for a job that was never created",
            setup: Setup::NoJob,
            job_arn: missing.clone(),
            trajectory_id: "traj-empty-no-job".to_string(),
            payload: Vec::new(),
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::EMPTY_BODY_MESSAGE.to_string(),
            },
        },
        Case {
            case: "a chat-completions payload for a job that was never created",
            setup: Setup::NoJob,
            job_arn: missing.clone(),
            trajectory_id: "traj-no-job".to_string(),
            payload: chat_payload("route me somewhere"),
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::job_not_found_message(&missing),
            },
        },
        Case {
            case: "a non-JSON payload for a trajectory sealed as ready",
            setup: Setup::Sealed("ready"),
            job_arn: created.clone(),
            trajectory_id: "traj-sealed-ready".to_string(),
            payload: b"not JSON at all".to_vec(),
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::AppendRefusal::Sealed {
                    status: state::TrajectoryStatus::Complete,
                    completion_status: Some(state::CompletionStatus::Ready),
                }
                .message("traj-sealed-ready"),
            },
        },
        Case {
            case: "a chat-completions payload for a trajectory sealed as failed",
            setup: Setup::Sealed("failed"),
            job_arn: created.clone(),
            trajectory_id: "traj-sealed-failed".to_string(),
            payload: chat_payload("one more turn"),
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::AppendRefusal::Sealed {
                    status: state::TrajectoryStatus::Complete,
                    completion_status: Some(state::CompletionStatus::Failed),
                }
                .message("traj-sealed-failed"),
            },
        },
        Case {
            case: "a payload larger than one captured turn may retain",
            setup: Setup::JobOnly,
            job_arn: created.clone(),
            trajectory_id: "traj-too-large".to_string(),
            payload: vec![b'x'; state::MAX_TURN_BODY_BYTES + 1],
            expected: Expected::Refused {
                status: 400,
                code: "ValidationException",
                message: state::AppendRefusal::BodyTooLarge {
                    bytes: state::MAX_TURN_BODY_BYTES + 1,
                }
                .message("traj-too-large"),
            },
        },
        Case {
            case: "a trajectory already holding the turn bound",
            setup: Setup::AtTurnBound,
            job_arn: created.clone(),
            trajectory_id: "traj-full".to_string(),
            payload: chat_payload("one more turn"),
            expected: Expected::Refused {
                status: 402,
                code: "ServiceQuotaExceededException",
                message: state::AppendRefusal::TurnQuota.message("traj-full"),
            },
        },
        Case {
            case: "a chat-completions payload on a fresh trajectory is served",
            setup: Setup::JobOnly,
            job_arn: created.clone(),
            trajectory_id: "traj-served".to_string(),
            payload: chat_payload("serve this"),
            expected: Expected::Served,
        },
        Case {
            case: "a minimal JSON payload on a fresh trajectory is served",
            setup: Setup::JobOnly,
            job_arn: created,
            trajectory_id: "t".to_string(),
            payload: b"{}".to_vec(),
            expected: Expected::Served,
        },
    ]
}

/// Put the emulator in the row's state, and answer the trajectory's turn count
/// once the setup is done.
async fn apply_setup(
    engine: &engine::InMemoryEngine,
    router: &MultiServiceRouter,
    case: &Case,
) -> usize {
    match case.setup {
        Setup::NoJob => {}
        Setup::JobOnly => create_job(engine, &case.job_arn),
        Setup::Sealed(status) => {
            create_job(engine, &case.job_arn);
            let first = sample(
                router,
                SAMPLE_PATH,
                &case.job_arn,
                &case.trajectory_id,
                &chat_payload("first turn"),
            )
            .await;
            assert_eq!(
                first.status, 200,
                "case {:?}: expected the first turn to be served, got {} / {}",
                case.case,
                first.status,
                first.text()
            );
            let sealed = complete_rollout(router, &case.job_arn, &case.trajectory_id, status).await;
            assert_eq!(
                sealed.status, 200,
                "case {:?}: expected CompleteRollout to seal the trajectory, got {} / {}",
                case.case,
                sealed.status,
                sealed.text()
            );
        }
        Setup::AtTurnBound => {
            create_job(engine, &case.job_arn);
            let store = engine.state::<ServiceState>();
            let mut guard = store.lock();
            let now = std::time::SystemTime::now();
            let mut trajectory = state::Trajectory::new(&case.job_arn, &case.trajectory_id, now);
            trajectory.turns = (0..state::MAX_TURNS_PER_TRAJECTORY)
                .map(|index| state::Turn {
                    request_body: b"{}".to_vec(),
                    request_content_type: Some("application/json".to_string()),
                    response_body: b"{}".to_vec(),
                    response_content_type: Some(state::JSON_CONTENT_TYPE.to_string()),
                    streamed: false,
                    index,
                    at: now,
                })
                .collect();
            guard.trajectories.insert(
                (case.job_arn.clone(), case.trajectory_id.clone()),
                trajectory,
            );
        }
    }
    captured_turns(engine, &case.job_arn, &case.trajectory_id)
}

/// RULE: for one state and one request, `/sample` and
/// `/sample-with-response-stream` answer with the same status, the same error
/// code and the same sentence, and a refusal captures nothing on either path.
#[tokio::test]
async fn both_sampling_paths_answer_one_request_the_same_way() {
    for case in cases() {
        let mut answers: Vec<(&str, Reply, usize, usize)> = Vec::new();
        for path in [SAMPLE_PATH, SAMPLE_STREAM_PATH] {
            let (engine, router) = mounted_service();
            let before = apply_setup(&engine, &router, &case).await;
            let reply = sample(
                &router,
                path,
                &case.job_arn,
                &case.trajectory_id,
                &case.payload,
            )
            .await;
            let after = captured_turns(&engine, &case.job_arn, &case.trajectory_id);
            answers.push((path, reply, before, after));
        }

        for (path, reply, before, after) in &answers {
            match &case.expected {
                Expected::Served => {
                    assert_eq!(
                        reply.status, 200,
                        "case {:?} on {path}: expected 200, got {} / {}",
                        case.case,
                        reply.status,
                        reply.text()
                    );
                    assert_eq!(
                        *after,
                        before + 1,
                        "case {:?} on {path}: expected the served turn to be captured, got {after} turns after {before}",
                        case.case
                    );
                    let expected_content_type = if *path == SAMPLE_STREAM_PATH {
                        state::EVENT_STREAM_CONTENT_TYPE
                    } else {
                        state::JSON_CONTENT_TYPE
                    };
                    assert_eq!(
                        reply.content_type.as_deref(),
                        Some(expected_content_type),
                        "case {:?} on {path}: expected the operation's own framing, got {:?}",
                        case.case,
                        reply.content_type
                    );
                }
                Expected::Refused {
                    status,
                    code,
                    message,
                } => {
                    assert_eq!(
                        reply.status, *status,
                        "case {:?} on {path}: expected {status}, got {} / {}",
                        case.case,
                        reply.status,
                        reply.text()
                    );
                    assert_eq!(
                        reply.error_code(),
                        *code,
                        "case {:?} on {path}: expected {code}, got {:?}",
                        case.case,
                        reply.error_type
                    );
                    assert_eq!(
                        &reply.message(),
                        message,
                        "case {:?} on {path}: expected the shared sentence, got {:?}",
                        case.case,
                        reply.message()
                    );
                    assert_eq!(
                        after, before,
                        "case {:?} on {path}: expected a refusal to capture nothing, got {after} turns after {before}",
                        case.case
                    );
                }
            }
        }

        let (first_path, first, _, _) = &answers[0];
        let (second_path, second, _, _) = &answers[1];
        assert_eq!(
            first.status, second.status,
            "case {:?}: expected {first_path} and {second_path} to answer the same status, got {} and {}",
            case.case, first.status, second.status
        );
        assert_eq!(
            first.error_code(),
            second.error_code(),
            "case {:?}: expected {first_path} and {second_path} to answer the same error code, got {:?} and {:?}",
            case.case,
            first.error_type,
            second.error_type
        );
        assert_eq!(
            first.message(),
            second.message(),
            "case {:?}: expected {first_path} and {second_path} to answer the same sentence, got {:?} and {:?}",
            case.case,
            first.message(),
            second.message()
        );
    }
}

/// RULE: a trajectory can mix turns served by either operation — the two run the
/// same capture, so the turns land in one trajectory, in call order, each
/// recording which operation served it.
#[tokio::test]
async fn turns_served_by_either_operation_land_in_one_trajectory() {
    // Rows differ along the interleaving of the two operations, so no row's order
    // is the one that happens to work.
    let orders: [&[&str]; 4] = [
        &[SAMPLE_PATH, SAMPLE_STREAM_PATH],
        &[SAMPLE_STREAM_PATH, SAMPLE_PATH],
        &[SAMPLE_STREAM_PATH, SAMPLE_STREAM_PATH, SAMPLE_PATH],
        &[
            SAMPLE_PATH,
            SAMPLE_STREAM_PATH,
            SAMPLE_PATH,
            SAMPLE_STREAM_PATH,
        ],
    ];
    for order in orders {
        let (engine, router) = mounted_service();
        let arn = job_arn("aws", "us-west-2", "000000000000", "AgentRFT", "mixed-run");
        create_job(&engine, &arn);
        let trajectory_id = "mixed-trajectory";
        for (index, path) in order.iter().enumerate() {
            let reply = sample(
                &router,
                path,
                &arn,
                trajectory_id,
                &chat_payload(&format!("turn {index}")),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {order:?} turn {index} on {path}: expected 200, got {} / {}",
                reply.status,
                reply.text()
            );
        }

        let store = engine.state::<ServiceState>();
        let guard = store.lock();
        let trajectory = guard
            .trajectory(&arn, trajectory_id)
            .unwrap_or_else(|| panic!("case {order:?}: expected one trajectory to hold every turn"));
        let served: Vec<bool> = trajectory.turns.iter().map(|turn| turn.streamed).collect();
        let expected: Vec<bool> = order
            .iter()
            .map(|path| *path == SAMPLE_STREAM_PATH)
            .collect();
        assert_eq!(
            served, expected,
            "case {order:?}: expected the turns in call order with each operation's own flag, got {served:?}"
        );
    }
}
