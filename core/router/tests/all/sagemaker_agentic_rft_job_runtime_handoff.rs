// SPDX-License-Identifier: CC0-1.0

//! The AgenticRFT hand-off: the SageMaker CONTROL plane mints the job, and the AgenticRFT
//! job runtime DATA plane serves that job's rollouts — both over the wire, with no
//! in-process help.
//!
//! RULE: a data-plane operation that routes a request to a resource named only by an
//! identifier the request carries resolves that identifier against the resource a CREATE
//! minted, so the identifier a client can use is exactly the one a create handed back, and
//! a well-formed identifier no create minted is refused with the operation's own modelled
//! client error and writes nothing. Here that is `Sample` / `SampleWithResponseStream`
//! resolving the `X-Amzn-SageMaker-Job-Arn` header member (`@required`, `@length` 1..256,
//! pattern
//! `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:job/[a-zA-Z0-9_\-]+/[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`
//! — `smithy_query(query="operation",operation="Sample")`) against the jobs
//! `SageMaker.CreateJob` created: that runtime model binds no operation that creates,
//! lists or deletes a job, so the ARN `CreateJob` answers is the only thing a client can
//! sample with, and an ARN no create minted is refused `400 ValidationException` naming
//! the ARN the request carried.
//!
//! Only this suite can observe that hand-off: each half is a separate crate with a
//! separate store, and the two agree solely because the control plane records the job in
//! ONE shared directory (`service_common::agentic_rft_jobs`) the data plane reads. So both
//! halves are driven through the SAME [`MultiServiceRouter`] and the SAME engine
//! `core/router/src/lib.rs` mounts them on, and every job here comes to exist the way a
//! client makes one: an HTTP `CreateJob` whose answer is the ARN the sampling calls then
//! carry. No test here reaches into the engine to register a job.
//!
//! The rows span what the rule generalises over rather than one recorded call: the job's
//! category and the shape of its name, the Region the create was signed for, which of the
//! two sampling operations serves each turn, how many turns a trajectory captures, the
//! partition/Region/account the data plane re-addresses the job through, and — for the
//! refusal — WHY the ARN resolves to nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The host every request here is addressed to: the claim each service makes is resolved
/// from the SigV4 credential scope and the URI path space, not from this name.
const HOST: &str = "emulator.local";

/// A `RoleArn` satisfying `CreateJob`'s modelled pattern
/// `^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const ROLE_ARN: &str = "arn:aws:iam::123456789012:role/service-role/AmazonSageMaker-ExecutionRole";

/// A `JobConfigSchemaVersion` satisfying the modelled `^\d+\.\d+\.\d+$` and `len 5..16`.
const SCHEMA_VERSION: &str = "1.0.0";

/// The `JobCategory` values a job can be created under.
///
/// Catalog source: `smithy_query(query="shape",shape="JobCategory",
/// model_path="services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json")`
/// — exactly `AgentRFT` and `AgentRFTEvaluation`.
const JOB_CATEGORIES: [&str; 2] = ["AgentRFT", "AgentRFTEvaluation"];

/// The headers the runtime binds `JobArn` and `TrajectoryId` to.
const JOB_ARN_HEADER: &str = "x-amzn-sagemaker-job-arn";
const TRAJECTORY_ID_HEADER: &str = "x-amzn-sagemaker-trajectory-id";

/// The two sampling paths the runtime model binds.
const SAMPLE_PATH: &str = "/sample";
const SAMPLE_STREAM_PATH: &str = "/sample-with-response-stream";

/// The `Content-Type` each sampling operation renders its answer in: JSON for `Sample`,
/// server-sent events for the streaming one.
const JSON_CONTENT_TYPE: &str = "application/json";
const EVENT_STREAM_CONTENT_TYPE: &str = "text/event-stream";

/// A SigV4 `Authorization` header whose credential scope names the signing name the whole
/// SageMaker family shares (`sagemaker`) and the Region the call is made in. Request
/// authentication is off on this router, so the signature itself is never checked; the
/// scope is what the router reads to resolve the route and what the control plane takes
/// the Region of a minted ARN from.
fn authorization(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/sagemaker/aws4_request, \
         SignedHeaders=host, \
         Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The SageMaker control plane and the AgenticRFT job runtime, mounted on one engine as
/// `core/router/src/lib.rs` mounts them.
fn build_router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// One HTTP answer, reduced to what these tests assert on.
#[derive(Debug)]
struct Answer {
    status: u16,
    error_type: Option<String>,
    content_type: Option<String>,
    body: Vec<u8>,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The bare error code: restJson1 carries it in `x-amzn-errortype` (which may also
    /// carry a namespace or a Coral shape-id suffix), awsJson1_1 in the body's `__type`.
    fn error_code(&self) -> String {
        let raw = match &self.error_type {
            Some(value) => value.clone(),
            None => self
                .json()
                .get("__type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        };
        raw.split(':')
            .next()
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_owned()
    }

    /// A string member of the answer, read under the name the MODEL declares.
    fn str_field(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// The prose of an error document, which both models name `Message`.
    fn message(&self) -> String {
        self.str_field("Message")
    }
}

/// Send one request through the whole router.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    region: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{HOST}{path}"))
        .header("host", HOST)
        .header("content-type", JSON_CONTENT_TYPE)
        .header("authorization", authorization(region));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(body))
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
    Answer {
        status,
        error_type,
        content_type,
        body: response.into_body().to_vec(),
    }
}

/// `CreateJob` on the control plane, in its awsJson1_1 RPC form — the ONE call that makes
/// a job exist for the data plane.
async fn create_job(
    router: &MultiServiceRouter,
    region: &str,
    category: &str,
    name: &str,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/",
        region,
        &[("x-amz-target", "SageMaker.CreateJob")],
        serde_json::to_vec(&json!({
            "JobName": name,
            "JobCategory": category,
            "JobConfigSchemaVersion": SCHEMA_VERSION,
            // A document conforming to the schema `DescribeJobSchemaVersion` publishes,
            // which `CreateJob` admits a configuration document against.
            "JobConfigDocument": json!({
                "AgentConfig": { "MaxTurns": 8 },
                "InputDataConfig": { "S3Uri": "s3://prompts/train" },
                "ModelPackageConfig": { "BaseModel": "demo-model" },
                "OutputDataConfig": { "S3Uri": "s3://artifacts/out" },
                "TrainingConfig": { "Epochs": 1 },
            })
            .to_string(),
            "RoleArn": ROLE_ARN,
        }))
        .expect("a serializable CreateJob body"),
    )
    .await
}

/// One `Sample` or `SampleWithResponseStream` call, as a runtime client makes it: the job
/// ARN and the trajectory id in headers, the inference payload as the body.
async fn sample(
    router: &MultiServiceRouter,
    streamed: bool,
    job_arn: &str,
    trajectory_id: &str,
    payload: &Value,
) -> Answer {
    let path = if streamed {
        SAMPLE_STREAM_PATH
    } else {
        SAMPLE_PATH
    };
    dispatch(
        router,
        "POST",
        path,
        "us-east-1",
        &[
            (JOB_ARN_HEADER, job_arn),
            (TRAJECTORY_ID_HEADER, trajectory_id),
        ],
        serde_json::to_vec(payload).expect("a serializable inference payload"),
    )
    .await
}

/// One `CompleteRollout` call.
async fn complete_rollout(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    status: &str,
) -> Answer {
    dispatch(
        router,
        "POST",
        "/complete-rollout",
        "us-east-1",
        &[(JOB_ARN_HEADER, job_arn)],
        serde_json::to_vec(&json!({
            "TrajectoryId": trajectory_id,
            "Status": status,
            "ClientToken": "handoff-complete",
        }))
        .expect("a serializable CompleteRollout body"),
    )
    .await
}

/// One `UpdateReward` call: `Rewards` is "one reward value per turn in the trajectory".
async fn update_reward(
    router: &MultiServiceRouter,
    job_arn: &str,
    trajectory_id: &str,
    rewards: &[f64],
) -> Answer {
    dispatch(
        router,
        "POST",
        "/update-reward",
        "us-east-1",
        &[(JOB_ARN_HEADER, job_arn)],
        serde_json::to_vec(&json!({
            "TrajectoryId": trajectory_id,
            "Rewards": rewards,
            "ClientToken": "handoff-reward",
        }))
        .expect("a serializable UpdateReward body"),
    )
    .await
}

/// A job ARN assembled from the parts the runtime's modelled pattern lets vary, for the
/// rows that must name a job no create minted.
fn job_arn(partition: &str, region: &str, account: &str, category: &str, name: &str) -> String {
    format!("arn:{partition}:sagemaker:{region}:{account}:job/{category}/{name}")
}

/// The chat-completion object one sampling answer carries: a non-streamed answer is the
/// object itself, a streamed one is the first `data:` event of the stream.
fn completion_of(answer: &Answer, streamed: bool) -> Value {
    if !streamed {
        return answer.json();
    }
    answer
        .text()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .find(|payload| *payload != "[DONE]")
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
        .unwrap_or(Value::Null)
}

/// One whole rollout a wire-only client performs against a job it had created for it.
struct Rollout {
    case: &'static str,
    /// The job the control plane is asked to create.
    category: &'static str,
    name: &'static str,
    /// The Region the create is signed for, which is the Region the minted ARN names.
    region: &'static str,
    /// The trajectory the turns are captured on.
    trajectory_id: &'static str,
    /// One `(payload, streamed)` pair per turn, in the order they are served.
    turns: Vec<(Value, bool)>,
}

/// An OpenAI-compatible chat-completions payload.
fn chat_payload(text: &str) -> Value {
    json!({
        "model": "rft-policy",
        "messages": [{ "role": "user", "content": text }],
        "max_tokens": 64,
    })
}

fn rollouts() -> Vec<Rollout> {
    // Rows differ along the job's category, the shape of its name (`JobName` is
    // `len 1..63`, pattern `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$`: one character, digits,
    // hyphens and the 63-character maximum), the Region the create was signed for, how
    // many turns the rollout captures, which sampling operation serves each turn, and the
    // form of the inference payload.
    let at_max_name = "a234567890123456789012345678901234567890123456789012345678901b";
    vec![
        Rollout {
            case: "one Sample turn on a one-character job name",
            category: JOB_CATEGORIES[0],
            name: "j",
            region: "us-east-1",
            trajectory_id: "traj-1",
            turns: vec![(chat_payload("Summarize the rollout."), false)],
        },
        Rollout {
            case: "two turns, Sample then the streaming operation",
            category: JOB_CATEGORIES[0],
            name: "rft2run3",
            region: "us-west-2",
            trajectory_id: "trajectory-with-a-longer-id-0042",
            turns: vec![
                (chat_payload("step one"), false),
                (json!({ "prompt": "step two" }), true),
            ],
        },
        Rollout {
            case: "three streamed turns on the evaluation category",
            category: JOB_CATEGORIES[1],
            name: "agentic-rft-eval-1",
            region: "eu-west-1",
            trajectory_id: "t",
            turns: vec![
                (chat_payload("first"), true),
                (chat_payload("second"), true),
                (json!({ "input": "third" }), true),
            ],
        },
        Rollout {
            case: "the 63-character job name, streaming then Sample",
            category: JOB_CATEGORIES[1],
            name: at_max_name,
            region: "ap-southeast-2",
            trajectory_id: "traj-at-max-name",
            turns: vec![
                (chat_payload("warm up"), true),
                (chat_payload("and again"), false),
            ],
        },
    ]
}

/// RULE (the served half): the `JobArn` `CreateJob` answers is the identifier a runtime
/// client samples with — every turn is served 200 in its operation's own framing and
/// captured on the `(JobArn, TrajectoryId)` trajectory, which the rest of the rollout
/// (`CompleteRollout`, then `UpdateReward` with one reward per captured turn) then reads
/// back. Nothing but HTTP is used to make the job exist.
#[tokio::test]
async fn a_client_samples_the_job_the_control_plane_minted_for_it() {
    for rollout in rollouts() {
        let Rollout {
            case,
            category,
            name,
            region,
            trajectory_id,
            turns,
        } = rollout;
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = build_router(&engine);

        let created = create_job(&router, region, category, name).await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: expected HTTP 200 from CreateJob, got {} {:?} / {}",
            created.status,
            created.error_code(),
            created.text()
        );
        // The ARN the client samples with is the one the create handed back — never one
        // this test assembled.
        let arn = created.str_field("JobArn");
        assert!(
            arn.ends_with(&format!(":job/{category}/{name}")),
            "case {case:?}: expected CreateJob to answer the ARN of the job it created, got \
             {arn:?}"
        );

        let mut completion_ids: BTreeSet<String> = BTreeSet::new();
        for (index, (payload, streamed)) in turns.iter().enumerate() {
            let served = sample(&router, *streamed, &arn, trajectory_id, payload).await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} turn {index}: expected HTTP 200 for the job the control plane \
                 minted, got {} {:?} / {}",
                served.status,
                served.error_code(),
                served.text()
            );
            let expected_content_type = if *streamed {
                EVENT_STREAM_CONTENT_TYPE
            } else {
                JSON_CONTENT_TYPE
            };
            assert_eq!(
                served.content_type.as_deref(),
                Some(expected_content_type),
                "case {case:?} turn {index}: expected Content-Type {expected_content_type:?}, \
                 got {:?}",
                served.content_type
            );
            if *streamed {
                let stream = served.text();
                assert!(
                    stream.trim_end().ends_with("data: [DONE]"),
                    "case {case:?} turn {index}: expected the event stream to end with its \
                     sentinel, got {stream:?}"
                );
            }

            let completion = completion_of(&served, *streamed);
            let expected_object = if *streamed {
                "chat.completion.chunk"
            } else {
                "chat.completion"
            };
            assert_eq!(
                completion.get("object").and_then(Value::as_str),
                Some(expected_object),
                "case {case:?} turn {index}: expected an OpenAI-compatible \
                 {expected_object:?} object, got {completion}"
            );
            // The envelope reports the JOB the call resolved to, as the shared directory
            // identifies one (its `<category>/<name>` resource path), not the ARN string
            // the request addressed that job through — so every turn of one rollout names
            // one job.
            let expected_model = format!("{category}/{name}");
            assert_eq!(
                completion.get("model").and_then(Value::as_str),
                Some(expected_model.as_str()),
                "case {case:?} turn {index}: expected the turn to be served for the job the \
                 request resolved to ({expected_model}), got {completion}"
            );
            let prompt_tokens = completion
                .pointer("/usage/prompt_tokens")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            assert!(
                prompt_tokens > 0,
                "case {case:?} turn {index}: expected the usage to count the prompt that was \
                 sent, got {completion}"
            );
            let id = completion
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            assert!(
                id.starts_with("chatcmpl-"),
                "case {case:?} turn {index}: expected a chat-completion id, got {id:?}"
            );
            completion_ids.insert(id);
        }
        // Each turn is its own captured turn of the trajectory, so no two share an id.
        assert_eq!(
            completion_ids.len(),
            turns.len(),
            "case {case:?}: expected {} distinct completion id(s), got {:?}",
            turns.len(),
            completion_ids
        );

        // The trajectory the sampling calls created is in the store the NEXT request
        // reads: sealing it and rewarding it — one reward per captured turn — can only
        // succeed if those turns were persisted under the minted ARN.
        let sealed = complete_rollout(&router, &arn, trajectory_id, "ready").await;
        assert_eq!(
            sealed.status,
            200,
            "case {case:?}: expected HTTP 200 from CompleteRollout on the sampled \
             trajectory, got {} {:?} / {}",
            sealed.status,
            sealed.error_code(),
            sealed.text()
        );
        let rewards: Vec<f64> = (0..turns.len())
            .map(|index| 0.25 + index as f64)
            .collect();
        let rewarded = update_reward(&router, &arn, trajectory_id, &rewards).await;
        assert_eq!(
            rewarded.status,
            200,
            "case {case:?}: expected HTTP 200 from UpdateReward with one reward per captured \
             turn ({} reward(s)), got {} {:?} / {}",
            rewards.len(),
            rewarded.status,
            rewarded.error_code(),
            rewarded.text()
        );
    }
}

/// RULE (the re-addressed half): a job is resolved by the category and name its ARN
/// carries, so the job a create minted is served however a caller addresses it — this
/// emulator serves one account in one Region, and partition, Region and account are how an
/// ARN is addressed, not which job it names.
#[tokio::test]
async fn the_minted_job_is_served_however_the_data_plane_addresses_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = build_router(&engine);
    let created = create_job(&router, "us-east-1", JOB_CATEGORIES[0], "rollout-9").await;
    assert_eq!(
        created.status,
        200,
        "case \"CreateJob\": expected HTTP 200, got {} / {}",
        created.status,
        created.text()
    );
    let minted = created.str_field("JobArn");
    // The ONE job every row below addresses, as the shared directory identifies one: the
    // `<category>/<name>` resource path of its ARN, which is what the served envelope
    // reports as the job that served the turn.
    let expected_model = format!("{}/rollout-9", JOB_CATEGORIES[0]);

    // Rows differ along the partition, Region and account the sampling call addresses the
    // SAME created job through — including the empty Region the runtime's pattern admits
    // (`[a-z0-9\-]*`). That these collapse onto one job is the directory's stated rule for
    // an emulator serving a single account and Region
    // (`services/common/src/agentic_rft_jobs.rs`, `JobDirectory::get_by_arn`), which the
    // control plane and this data plane share; the crate-level rows for it are in the
    // runtime's own `tests/all/job_resolution.rs`.
    let rows: Vec<(&str, String)> = vec![
        ("the ARN CreateJob answered", minted),
        (
            "another Region and account",
            job_arn(
                "aws",
                "eu-west-1",
                "123456789012",
                JOB_CATEGORIES[0],
                "rollout-9",
            ),
        ),
        (
            "another partition",
            job_arn(
                "aws-cn",
                "cn-north-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-9",
            ),
        ),
        (
            "a gov partition and account",
            job_arn(
                "aws-us-gov",
                "us-gov-west-1",
                "210987654321",
                JOB_CATEGORIES[0],
                "rollout-9",
            ),
        ),
        (
            "the empty Region the pattern admits",
            job_arn("aws", "", "123456789012", JOB_CATEGORIES[0], "rollout-9"),
        ),
    ];

    for (index, (case, arn)) in rows.iter().enumerate() {
        for streamed in [false, true] {
            let trajectory_id = format!("traj-{index}-{streamed}");
            let served = sample(
                &router,
                streamed,
                arn,
                &trajectory_id,
                &chat_payload("addressed differently"),
            )
            .await;
            assert_eq!(
                served.status,
                200,
                "case {case:?} (streamed={streamed}): expected HTTP 200 for the created job \
                 addressed as {arn}, got {} {:?} / {}",
                served.status,
                served.error_code(),
                served.text()
            );
            let completion = completion_of(&served, streamed);
            // The job the call resolved to is the same job for every row, so the envelope
            // reports the one job however the ARN addressed it: partition, Region and
            // account are how an ARN is addressed, not which job it names.
            assert_eq!(
                completion.get("model").and_then(Value::as_str),
                Some(expected_model.as_str()),
                "case {case:?} (streamed={streamed}): expected the turn to be served for the \
                 one job the create minted ({expected_model}), got {completion}"
            );
        }
    }
}

/// RULE (the refused half): a well-formed `JobArn` that no create minted resolves to no
/// job, so the sampling call is refused `400 ValidationException` naming the ARN the
/// request carried, and nothing is captured — the refused pair holds no trajectory for
/// `CompleteRollout` to seal.
#[tokio::test]
async fn an_arn_no_create_minted_is_refused_and_captures_nothing() {
    /// What the control plane holds when the refused call is made.
    enum Setup {
        /// No job was ever created.
        Nothing,
        /// This category and name were created, and the request names something else.
        Created(&'static str, &'static str),
    }

    // Rows differ along WHY the ARN resolves to nothing: nothing was created, another
    // name of the same category was, the name is held under the other category, the name
    // differs only in case, or the ARN's category segment is one no `JobCategory` can
    // name (the control plane's `JobArn` carries `[a-zA-Z0-9]+` there, while the runtime's
    // pattern also admits `_` and `-`).
    let rows: Vec<(&str, Setup, String)> = vec![
        (
            "no job was ever created",
            Setup::Nothing,
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "ghost-run",
            ),
        ),
        (
            "another name of the same category",
            Setup::Created(JOB_CATEGORIES[0], "rollout-1"),
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-2",
            ),
        ),
        (
            "the created name under the other category",
            Setup::Created(JOB_CATEGORIES[1], "rollout-1"),
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "rollout-1",
            ),
        ),
        (
            "the created name in another case",
            Setup::Created(JOB_CATEGORIES[0], "rollout-1"),
            job_arn(
                "aws",
                "us-east-1",
                "000000000000",
                JOB_CATEGORIES[0],
                "ROLLOUT-1",
            ),
        ),
        (
            "a category segment no JobCategory can name",
            Setup::Created(JOB_CATEGORIES[0], "rollout-1"),
            job_arn(
                "aws",
                "us-west-2",
                "123456789012",
                "agentic_rft",
                "rollout-1",
            ),
        ),
    ];

    for (case, setup, arn) in rows {
        for streamed in [false, true] {
            let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
            let router = build_router(&engine);
            if let Setup::Created(category, name) = setup {
                let created = create_job(&router, "us-east-1", category, name).await;
                assert_eq!(
                    created.status,
                    200,
                    "case {case:?}: expected the setup CreateJob to answer HTTP 200, got {} / {}",
                    created.status,
                    created.text()
                );
            }

            let refused = sample(
                &router,
                streamed,
                &arn,
                "traj-1",
                &chat_payload("sample my own job"),
            )
            .await;
            assert_eq!(
                (refused.status, refused.error_code().as_str()),
                (400, "ValidationException"),
                "case {case:?} (streamed={streamed}): expected 400 ValidationException for an \
                 ARN no create minted, got {} {:?} / {}",
                refused.status,
                refused.error_code(),
                refused.text()
            );
            assert_eq!(
                refused.message(),
                format!("Could not find job with jobArn {arn}"),
                "case {case:?} (streamed={streamed}): expected the refusal to name the \
                 request's own jobArn, got {:?}",
                refused.message()
            );
            assert_eq!(
                refused.content_type.as_deref(),
                Some(JSON_CONTENT_TYPE),
                "case {case:?} (streamed={streamed}): expected a restJson1 error document, got \
                 {:?}",
                refused.content_type
            );

            // Nothing was captured: the pair the refused call named holds no trajectory,
            // which is what the next request sees.
            let sealed = complete_rollout(&router, &arn, "traj-1", "ready").await;
            assert_eq!(
                (sealed.status, sealed.error_code().as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?} (streamed={streamed}): expected the refused call to capture no \
                 turn, so CompleteRollout finds no trajectory, got {} {:?} / {}",
                sealed.status,
                sealed.error_code(),
                sealed.text()
            );
        }
    }
}
