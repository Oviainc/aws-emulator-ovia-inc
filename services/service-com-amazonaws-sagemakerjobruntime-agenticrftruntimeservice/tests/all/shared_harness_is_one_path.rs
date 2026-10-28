// SPDX-License-Identifier: CC0-1.0

//! The shared harness is ONE path: every suite of this crate reduces a wire answer and
//! brings a job into existence through the same code.
//!
//! RULE: the reduction of an answer to `(status, error code, Message)` is a property of
//! the PROTOCOL and not of the operation that answered — `x-amzn-errortype` names the
//! error shape, optionally decorated with a Smithy namespace (`…#Name`) or a Coral
//! shape-id suffix (`Name:http://…`), and a restJson1 error document carries the modelled
//! `Message` member — so one reduction serves every operation of the service, and a job
//! becomes addressable only through the shared job directory the SageMaker control plane
//! writes, however the ARN that addresses it was assembled.
//!
//! This suite drives that one path through an operation of each of the four the model
//! binds (`Sample`, `SampleWithResponseStream`, `CompleteRollout`, `UpdateReward` —
//! `smithy_query(query="digest")`), over each spelling of the job setup the harness
//! offers, and pins the reduction itself over the decorations an `x-amzn-errortype` value
//! can carry and the body forms a refusal can arrive in.
//!
//! Every answer goes over the wire through a [`MultiServiceRouter`], and the store is read
//! afterwards the way the NEXT request would read it.

use serde_json::json;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::state;
use service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::ComAmazonawsSagemakerjobruntimeAgenticrftruntimeserviceState as ServiceState;
use smithy_server_runtime::MultiServiceRouter;

// The harness under test: the wire-answer reduction and the job-directory setup every
// suite of this crate drives (`tests/all/harness.rs`).
use crate::harness::{
    create_job, create_job_at_arn, job_arn, minted_job_arn, mounted_service, register_job, send,
    Reply, ACCOUNT, COMPLETE_ROLLOUT_PATH, JOB_ARN_HEADER, SAMPLE_PATH, SAMPLE_STREAM_PATH,
    TRAJECTORY_ID_HEADER, UPDATE_REWARD_PATH,
};

/// The four operations the model binds, each named by the path it is bound to.
const OPERATIONS: [(&str, &str); 4] = [
    ("Sample", SAMPLE_PATH),
    ("SampleWithResponseStream", SAMPLE_STREAM_PATH),
    ("CompleteRollout", COMPLETE_ROLLOUT_PATH),
    ("UpdateReward", UPDATE_REWARD_PATH),
];

/// How a row brings the job into existence: the three spellings the suites of this crate
/// need, each of which must leave the job addressable at the ARN the call carries.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// The ARN the control plane mints for the category and name.
    Minted,
    /// A job registered from the ARN a caller addresses it by, which may name another
    /// partition, Region or account.
    AtArn,
    /// A job registered under its category and name with an ARN stated separately.
    Registered,
}

/// One row of the served table: a job brought into existence one of the three ways, and
/// the ARN the calls address it by.
struct ServedRow {
    case: &'static str,
    setup: Setup,
    category: &'static str,
    name: &'static str,
    arn: String,
}

/// One row of the refused table: an ARN that resolves to no job, the trajectory id the
/// calls carry, and whether an unrelated job exists in the directory.
struct RefusedRow {
    case: &'static str,
    another_job: bool,
    arn: String,
    trajectory_id: String,
}

/// One row of the reduction table: a decorated (or absent) `x-amzn-errortype`, a body
/// form, and what the reduction must read out of them.
struct DecorationRow {
    case: &'static str,
    error_type: Option<&'static str>,
    body: &'static [u8],
    expected_code: &'static str,
    expected_message: &'static str,
}

/// One call of `operation`, built the way that operation's model binds its members: the
/// sampling operations carry `TrajectoryId` in a header, the rollout operations in the
/// JSON body.
async fn call(router: &MultiServiceRouter, path: &str, job: &str, trajectory_id: &str) -> Reply {
    let json_headers = [
        (JOB_ARN_HEADER, job),
        (TRAJECTORY_ID_HEADER, trajectory_id),
        ("content-type", "application/json"),
    ];
    match path {
        SAMPLE_PATH | SAMPLE_STREAM_PATH => {
            send(
                router,
                path,
                &json_headers,
                serde_json::to_vec(&json!({ "messages": [{ "role": "user", "content": "go" }] }))
                    .expect("a serializable payload"),
            )
            .await
        }
        COMPLETE_ROLLOUT_PATH => {
            let body = json!({
                "TrajectoryId": trajectory_id,
                "Status": "ready",
                "ClientToken": "tok-complete",
            });
            send(
                router,
                path,
                &[(JOB_ARN_HEADER, job), ("content-type", "application/json")],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
        UPDATE_REWARD_PATH => {
            let body = json!({
                "TrajectoryId": trajectory_id,
                "Rewards": [1.0],
                "ClientToken": "tok-reward",
            });
            send(
                router,
                path,
                &[(JOB_ARN_HEADER, job), ("content-type", "application/json")],
                serde_json::to_vec(&body).expect("a serializable body"),
            )
            .await
        }
        other => unreachable!("this model binds no operation at {other}"),
    }
}

/// RULE (the served half): a job created through any of the harness's spellings is
/// addressable, so every operation of the service is SERVED for it — and the shared
/// reduction reports a served answer as carrying no error code and no `Message`.
#[tokio::test]
async fn every_operation_is_served_for_a_job_the_shared_setup_created() {
    // Rows differ along HOW the job was created and along the partition, Region and
    // account the ARN that addresses it names.
    let rows = [
        ServedRow {
            case: "the ARN the control plane minted",
            setup: Setup::Minted,
            category: "AgentRFT",
            name: "harness-run-1",
            arn: minted_job_arn("AgentRFT", "harness-run-1"),
        },
        ServedRow {
            case: "registered from the ARN a caller addresses it by",
            setup: Setup::AtArn,
            category: "AgentRFTEvaluation",
            name: "harness-eval-1",
            arn: job_arn(
                "aws",
                "eu-west-1",
                "123456789012",
                "AgentRFTEvaluation",
                "harness-eval-1",
            ),
        },
        ServedRow {
            case: "registered under its pair with the ARN stated separately",
            setup: Setup::Registered,
            category: "AgentRFT",
            name: "harness-run-2",
            arn: job_arn(
                "aws-cn",
                "cn-north-1",
                "123456789012",
                "AgentRFT",
                "harness-run-2",
            ),
        },
        ServedRow {
            case: "registered from an ARN in another partition",
            setup: Setup::AtArn,
            category: "AgentRFT",
            name: "harness-run-3",
            arn: job_arn(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                "AgentRFT",
                "harness-run-3",
            ),
        },
    ];

    for ServedRow {
        case,
        setup,
        category,
        name,
        arn,
    } in rows
    {
        // Each operation is driven on its own engine, in the order a rollout runs: the
        // operation under test is the LAST call, so what it answers is its own.
        for (operation, path) in OPERATIONS {
            let (engine, router) = mounted_service();
            match setup {
                Setup::Minted => create_job(&engine, category, name),
                Setup::AtArn => create_job_at_arn(&engine, &arn),
                Setup::Registered => register_job(&engine, category, name, &arn),
            }

            // The rollout operations need a captured turn and (for the reward) a sealed
            // trajectory before they can be served.
            if path == COMPLETE_ROLLOUT_PATH || path == UPDATE_REWARD_PATH {
                let seeded = call(&router, SAMPLE_PATH, &arn, "traj-1").await;
                assert_eq!(
                    seeded.status,
                    200,
                    "case {case:?}/{operation}: expected the seeding Sample to answer 200, got \
                     {} / {}",
                    seeded.status,
                    seeded.text()
                );
            }
            if path == UPDATE_REWARD_PATH {
                let sealed = call(&router, COMPLETE_ROLLOUT_PATH, &arn, "traj-1").await;
                assert_eq!(
                    sealed.status,
                    200,
                    "case {case:?}/{operation}: expected the seeding CompleteRollout to answer \
                     200, got {} / {}",
                    sealed.status,
                    sealed.text()
                );
            }

            let served = call(&router, path, &arn, "traj-1").await;
            assert_eq!(
                served.status,
                200,
                "case {case:?}/{operation}: expected 200 for a job the shared setup created, got \
                 {} {:?} / {}",
                served.status,
                served.error_type,
                served.text()
            );
            assert_eq!(
                (served.error_code().as_str(), served.message().as_str()),
                ("", ""),
                "case {case:?}/{operation}: expected a served answer to reduce to no error code \
                 and no Message, got {:?} / {:?}",
                served.error_code(),
                served.message()
            );
            if path == SAMPLE_PATH || path == SAMPLE_STREAM_PATH {
                let expected = if path == SAMPLE_STREAM_PATH {
                    state::EVENT_STREAM_CONTENT_TYPE
                } else {
                    state::JSON_CONTENT_TYPE
                };
                assert_eq!(
                    served.content_type.as_deref(),
                    Some(expected),
                    "case {case:?}/{operation}: expected Content-Type {expected:?}, got {:?}",
                    served.content_type
                );
            }

            // The answer came from persisted state: the trajectory the calls drove is the
            // one the next request would read, under the ARN the call carried.
            let store = engine.state::<ServiceState>();
            let guard = store.lock();
            let turns = guard
                .trajectory(&arn, "traj-1")
                .map_or(0, |trajectory| trajectory.turns.len());
            assert_eq!(
                turns, 1,
                "case {case:?}/{operation}: expected the store to hold the one captured turn, got \
                 {turns}"
            );
        }
    }
}

/// RULE (the refused half): the same reduction reads every operation's refusal — the
/// sampling operations refuse an unresolvable `JobArn` as a `ValidationException` naming
/// the ARN, the rollout operations answer `ResourceNotFoundException` naming the
/// trajectory and the account — so one body serves all four.
#[tokio::test]
async fn every_operation_refusal_is_read_by_the_one_reduction() {
    // Rows differ along WHY the ARN resolves to nothing and along the trajectory id's
    // length, including its modelled 256-character maximum.
    let at_max = "t".repeat(256);
    let rows = [
        RefusedRow {
            case: "no job was ever created",
            another_job: false,
            arn: minted_job_arn("AgentRFT", "ghost-run"),
            trajectory_id: "traj-1".to_string(),
        },
        RefusedRow {
            case: "another job of the same category exists",
            another_job: true,
            arn: minted_job_arn("AgentRFT", "ghost-run-2"),
            trajectory_id: "traj-1".to_string(),
        },
        RefusedRow {
            case: "a trajectory id at the modelled maximum, no job created",
            another_job: false,
            arn: job_arn("aws", "us-west-2", ACCOUNT, "AgentRFTEvaluation", "ghost-eval"),
            trajectory_id: at_max,
        },
    ];

    for RefusedRow {
        case,
        another_job,
        arn,
        trajectory_id,
    } in rows
    {
        for (operation, path) in OPERATIONS {
            let (engine, router) = mounted_service();
            if another_job {
                create_job(&engine, "AgentRFT", "another-run");
            }

            let refused = call(&router, path, &arn, &trajectory_id).await;
            let (expected_status, expected_code, expected_message) =
                if path == SAMPLE_PATH || path == SAMPLE_STREAM_PATH {
                    (
                        400,
                        "ValidationException",
                        format!("Could not find job with jobArn {arn}"),
                    )
                } else {
                    (
                        404,
                        "ResourceNotFoundException",
                        format!("Trajectory {trajectory_id} not found for account {ACCOUNT}"),
                    )
                };
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (expected_status, expected_code),
                "case {case:?}/{operation}: expected {expected_status} {expected_code}, got {} \
                 {:?} / {}",
                refused.status,
                refused.error_type,
                refused.text()
            );
            assert_eq!(
                refused.content_type.as_deref(),
                Some("application/json"),
                "case {case:?}/{operation}: expected a restJson1 error document, got {:?}",
                refused.content_type
            );
            assert_eq!(
                refused.message(),
                expected_message,
                "case {case:?}/{operation}: expected the message {expected_message:?}, got {:?}",
                refused.message()
            );
            let held = engine.state::<ServiceState>().lock().trajectories.len();
            assert_eq!(
                held, 0,
                "case {case:?}/{operation}: expected a refused call to write nothing, got {held} \
                 trajectory/ies"
            );
        }
    }
}

/// RULE (the reduction itself): `x-amzn-errortype` names the error shape, and the bare
/// name is what survives the decorations restJson1 permits — a Smithy namespace before a
/// `#`, a Coral shape-id suffix after a `:`, or both — while the `Message` member is read
/// under the name the model declares and under no other.
#[tokio::test]
async fn the_reduction_strips_every_decoration_an_error_type_can_carry() {
    // Rows span the decorations the header can carry (bare, namespaced, suffixed, both,
    // absent) and the body forms a refusal can arrive in (the modelled document, the
    // framework's lowercase envelope, a document without the member, a non-JSON body).
    let rows = [
        DecorationRow {
            case: "the bare shape name",
            error_type: Some("ValidationException"),
            body: br#"{"Message":"a modelled sentence"}"#,
            expected_code: "ValidationException",
            expected_message: "a modelled sentence",
        },
        DecorationRow {
            case: "a namespaced shape id",
            error_type: Some("com.amazonaws.sagemakerjobruntime#ConflictException"),
            body: br#"{"Message":"a modelled sentence"}"#,
            expected_code: "ConflictException",
            expected_message: "a modelled sentence",
        },
        DecorationRow {
            case: "a Coral shape-id suffix",
            error_type: Some("ResourceNotFoundException:http://internal.amazon.com/coral/service/"),
            body: br#"{"Message":"a modelled sentence"}"#,
            expected_code: "ResourceNotFoundException",
            expected_message: "a modelled sentence",
        },
        DecorationRow {
            case: "a namespace and a suffix together",
            error_type: Some(
                "com.amazonaws.sagemakerjobruntime#ThrottlingException:http://internal/",
            ),
            body: br#"{"Message":"a modelled sentence"}"#,
            expected_code: "ThrottlingException",
            expected_message: "a modelled sentence",
        },
        DecorationRow {
            case: "no error type at all",
            error_type: None,
            body: br#"{"Message":"a modelled sentence"}"#,
            expected_code: "",
            expected_message: "a modelled sentence",
        },
        DecorationRow {
            case: "the framework's lowercase envelope, which is not the modelled member",
            error_type: Some("ValidationException"),
            body: br#"{"message":"a pre-dispatch sentence"}"#,
            expected_code: "ValidationException",
            expected_message: "",
        },
        DecorationRow {
            case: "a document carrying no message member",
            error_type: Some("ValidationException"),
            body: br#"{"Other":"something else"}"#,
            expected_code: "ValidationException",
            expected_message: "",
        },
        DecorationRow {
            case: "a body that is not JSON at all",
            error_type: Some("ValidationException"),
            body: b"not json",
            expected_code: "ValidationException",
            expected_message: "",
        },
    ];

    for DecorationRow {
        case,
        error_type,
        body,
        expected_code,
        expected_message,
    } in rows
    {
        let mut response = http::Response::builder()
            .status(400)
            .header("content-type", "application/json");
        if let Some(error_type) = error_type {
            response = response.header("x-amzn-errortype", error_type);
        }
        let response = response
            .body(bytes::Bytes::from(body.to_vec()))
            .expect("a well-formed response");
        let reply = Reply::of(response);

        assert_eq!(
            reply.error_code(),
            expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            reply.message(),
            expected_message,
            "case {case:?}: expected the Message {expected_message:?}, got {:?}",
            reply.message()
        );
        assert_eq!(
            (reply.status, reply.text().as_str()),
            (400, String::from_utf8_lossy(body).as_ref()),
            "case {case:?}: expected the status and body to survive the reduction untouched, got \
             {} / {:?}",
            reply.status,
            reply.text()
        );
    }
}
