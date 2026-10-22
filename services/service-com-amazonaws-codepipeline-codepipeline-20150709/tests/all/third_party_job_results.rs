// SPDX-License-Identifier: CC0-1.0

//! `PutThirdPartyJobSuccessResult`: a partner action's job worker reporting its
//! success for one third-party job, over the wire this service actually speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule these tables pin, one row per way the rule generalises:
//!
//! * a `jobId` no retained job carries is refused `JobNotFoundException` whatever
//!   the id's FORM — `ThirdPartyJobId` has no modelled pattern, so only whether a
//!   job is filed under the id decides this;
//! * a retained job reached with a `clientToken` other than the one it was handed
//!   out with is refused `InvalidClientTokenException`;
//! * a retained job that no worker currently holds — any `JobStatus` other than
//!   `InProgress`, whether it is still being handed out or already terminal — is
//!   refused `InvalidJobStateException`;
//! * only a held job accepts the result, and it then keeps every member the worker
//!   reported, so a later reader answers from the recorded result.
//!
//! `PollForThirdPartyJobs` reports the jobs waiting for a partner action type, but
//! the action-execution plane that QUEUES them is not part of this slice, so — as in
//! `tests/all/third_party_jobs.rs` — these tests queue their jobs through
//! `ComAmazonawsCodepipelineCodepipeline20150709State` (`enqueue_third_party_job`),
//! the only authoring seam the queue has, which is the SAME store the handler reads.
//! Every call itself goes through the router, so no assertion here can pass unless
//! the handler computes its answer from that store.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeId, JOB_STATUS_VALUES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State as CodePipelineState, ThirdPartyJobRecord,
    JOB_STATUS_IN_PROGRESS, JOB_STATUS_SUCCEEDED,
};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The operation under test.
const OPERATION: &str = "PutThirdPartyJobSuccessResult";

/// The id the seeded job is filed under. `ThirdPartyJob$jobId` carries the UUID
/// pattern `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`, which
/// is the form a real poll hands out, and it carries letters so a case-only variant
/// of it is a different id.
const JOB_ID: &str = "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d";

/// The `clientToken` the seeded job was handed out with.
const CLIENT_TOKEN: &str = "worker-token";

/// The `clientId` half of the pair the job was handed to, in the form
/// `ThirdPartyJob$clientId` models (the same UUID pattern).
const CLIENT_ID: &str = "7f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b";

/// `ThirdPartyJobId`'s modelled maximum length (`len 1..512`).
const MAX_JOB_ID_LENGTH: usize = 512;

/// `ClientToken`'s modelled maximum length (`len 1..256`).
const MAX_CLIENT_TOKEN_LENGTH: usize = 256;

/// `ContinuationToken`'s modelled maximum length (`len 1..2048`).
const MAX_CONTINUATION_TOKEN_LENGTH: usize = 2048;

/// When a seeded job's status last changed: a fixed past instant, so a result the
/// handler records is strictly later than it.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

/// The `ActionOwner` value a partner integration carries
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`): the store queues a third-party job only for a `ThirdParty` action
/// type.
const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// The identity of the `ThirdParty` action type whose queue a seeded job waits in.
/// `provider` distinguishes one partner action from another, so a second value of it
/// is a second queue.
fn third_party_action_type(provider: &str) -> ActionTypeId {
    ActionTypeId {
        category: "Build".to_string(),
        owner: OWNER_THIRD_PARTY.to_string(),
        provider: provider.to_string(),
        version: "1".to_string(),
    }
}

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
///
/// Each suite in this crate carries its own harness with the helpers its operation
/// needs: `tests/all/pipeline_lifecycle.rs` holds only the router, while
/// `tests/all/action_type_catalog.rs`, `tests/all/stage_transitions.rs` and
/// `tests/all/third_party_jobs.rs` keep the engine to seed the state their operation
/// reports on. This one keeps it for the same reason: the action-execution plane that
/// queues a partner action's jobs is not part of this slice, so the store is the only
/// place a job can come from, and it is what a handler writes through too.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { engine, router }
    }

    /// Queue a job for the default partner action type, as the action-execution
    /// plane does when an execution reaches a partner action, with the `clientToken`
    /// a worker has to present and the `JobStatus` the job is in.
    fn seed_job(&self, job_id: &str, client_token: &str, status: &str) {
        self.seed_job_for(&third_party_action_type("PartnerBuild"), job_id, client_token, status);
    }

    /// The same, for a named partner action type's queue.
    fn seed_job_for(
        &self,
        action_type: &ActionTypeId,
        job_id: &str,
        client_token: &str,
        status: &str,
    ) {
        let mut job = ThirdPartyJobRecord::new(job_id, CLIENT_ID);
        job.client_token = client_token.to_string();
        job.status = status.to_string();
        job.last_status_change = seeded_at();
        let queued = self
            .engine
            .state::<CodePipelineState>()
            .lock()
            .enqueue_third_party_job(action_type, job);
        assert!(
            queued,
            "seeding job {job_id}: the store must queue a job for the ThirdParty action \
             type {action_type:?}"
        );
    }

    /// The stored job, as the next caller would read it. A worker names a job by id
    /// alone, so the search spans every action type's queue.
    fn job(&self, job_id: &str) -> ThirdPartyJobRecord {
        self.engine
            .state::<CodePipelineState>()
            .lock()
            .third_party_jobs
            .values()
            .flatten()
            .find(|job| job.job_id == job_id)
            .cloned()
            .unwrap_or_else(|| panic!("job {job_id} must still be in the store"))
    }

    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("host", "codepipeline.us-east-1.amazonaws.com")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body)
                .unwrap_or_else(|e| panic!("{operation}: response body is not JSON ({e})"))
        };
        (status, parsed)
    }

    /// The HTTP status and the `__type` an error response carries (the awsJson error
    /// code clients match on).
    async fn expect_error(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, response) = self.call(operation, body).await;
        let code = response
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
            .to_string();
        (status, code)
    }

    async fn ok(&self, operation: &str, body: &Value) {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {response}"
        );
    }
}

/// The request a worker sends: the job and token it holds, plus the optional members
/// of this result.
fn result_body(job_id: &str, client_token: &str, extras: &Value) -> Value {
    let mut body = json!({"jobId": job_id, "clientToken": client_token});
    for (member, value) in extras.as_object().expect("an object of result members") {
        body[member] = value.clone();
    }
    body
}

#[tokio::test]
async fn an_id_no_retained_job_carries_is_not_found_whatever_its_form() {
    let emulator = Emulator::new();
    emulator.seed_job(JOB_ID, CLIENT_TOKEN, JOB_STATUS_IN_PROGRESS);

    let longest_id = "a".repeat(MAX_JOB_ID_LENGTH);
    let longest_token = "t".repeat(MAX_CLIENT_TOKEN_LENGTH);
    let cases: Vec<(&str, &str, &str)> = vec![
        ("a non-UUID id", "not-a-uuid", CLIENT_TOKEN),
        (
            "a one-character id, the modelled minimum length",
            "x",
            CLIENT_TOKEN,
        ),
        (
            "an id at the modelled maximum length of 512",
            longest_id.as_str(),
            CLIENT_TOKEN,
        ),
        (
            "a UUID-shaped id no job carries",
            "22222222-2222-4222-8222-222222222222",
            CLIENT_TOKEN,
        ),
        (
            "the retained id with its last character changed",
            "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5e",
            CLIENT_TOKEN,
        ),
        (
            "the retained id in upper case",
            "A1B2C3D4-E5F6-4A7B-8C9D-0E1F2A3B4C5D",
            CLIENT_TOKEN,
        ),
        ("an id carrying punctuation", "job/../id?x=1", CLIENT_TOKEN),
        ("an id that is a decimal number", "4815162342", CLIENT_TOKEN),
        (
            "an unknown id with a clientToken at the modelled maximum length of 256",
            "unknown-long-token",
            longest_token.as_str(),
        ),
        (
            "an unknown id with a one-character clientToken",
            "unknown-short-token",
            "t",
        ),
    ];

    for (case, job_id, client_token) in cases {
        let (status, code) = emulator
            .expect_error(OPERATION, &json!({"jobId": job_id, "clientToken": client_token}))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "JobNotFoundException"),
            "case {case:?}: expected a 400 JobNotFoundException, got {status} {code:?}"
        );
    }

    // None of those refusals touched the job the store does hold.
    let retained = emulator.job(JOB_ID);
    assert_eq!(
        retained.status, JOB_STATUS_IN_PROGRESS,
        "case \"the retained job after every refusal\": expected {JOB_STATUS_IN_PROGRESS}, got {retained:?}"
    );
    assert_eq!(
        retained.last_status_change,
        seeded_at(),
        "case \"the retained job after every refusal\": its status must not have changed, got {retained:?}"
    );
}

#[tokio::test]
async fn a_held_job_accepts_a_result_and_keeps_every_member_the_worker_reported() {
    let longest_continuation_token = "c".repeat(MAX_CONTINUATION_TOKEN_LENGTH);
    let cases: Vec<(&str, Value)> = vec![
        ("no optional member at all", json!({})),
        (
            "a continuation token: the partner action is still in progress",
            json!({"continuationToken": "deploy-d-ABC123"}),
        ),
        (
            "a continuation token at the modelled maximum length of 2048",
            json!({"continuationToken": longest_continuation_token}),
        ),
        (
            "a current revision with only its required members",
            json!({"currentRevision": {"revision": "r1", "changeIdentifier": "c1"}}),
        ),
        (
            "a current revision carrying every member",
            json!({"currentRevision": {
                "revision": "9f2c1ab",
                "changeIdentifier": "change-7",
                "created": 1_600_000_123_i64,
                "revisionSummary": "a one-line summary",
            }}),
        ),
        (
            "execution details at percentComplete 0, the published minimum",
            json!({"executionDetails": {
                "summary": "work started",
                "externalExecutionId": "ext-1",
                "percentComplete": 0,
            }}),
        ),
        (
            "execution details carrying only percentComplete 100, the published maximum",
            json!({"executionDetails": {"percentComplete": 100}}),
        ),
        (
            "every optional member together",
            json!({
                "continuationToken": "deploy-d-XYZ789",
                "currentRevision": {
                    "revision": "deadbeef",
                    "changeIdentifier": "change-42",
                    "created": 1_600_000_456_i64,
                    "revisionSummary": "everything at once",
                },
                "executionDetails": {
                    "summary": "halfway",
                    "externalExecutionId": "ext-2",
                    "percentComplete": 50,
                },
            }),
        ),
    ];

    for (case, extras) in cases {
        let emulator = Emulator::new();
        emulator.seed_job(JOB_ID, CLIENT_TOKEN, JOB_STATUS_IN_PROGRESS);
        let body = result_body(JOB_ID, CLIENT_TOKEN, &extras);

        emulator.ok(OPERATION, &body).await;

        let job = emulator.job(JOB_ID);
        assert_eq!(
            job.status, JOB_STATUS_SUCCEEDED,
            "case {case:?}: expected the job at {JOB_STATUS_SUCCEEDED}, got {job:?}"
        );
        assert!(
            job.last_status_change > seeded_at(),
            "case {case:?}: expected the status change recorded after {:?}, got {job:?}",
            seeded_at()
        );
        assert_eq!(
            job.client_id, CLIENT_ID,
            "case {case:?}: a result must not rebind the job's owner, got {job:?}"
        );
        assert_eq!(
            job.continuation_token.as_deref(),
            extras.get("continuationToken").and_then(Value::as_str),
            "case {case:?}: unexpected stored continuationToken, got {job:?}"
        );
        assert_eq!(
            job.current_revision.as_ref().map(|r| r.revision.as_str()),
            extras
                .pointer("/currentRevision/revision")
                .and_then(Value::as_str),
            "case {case:?}: unexpected stored currentRevision.revision, got {job:?}"
        );
        assert_eq!(
            job.current_revision
                .as_ref()
                .map(|r| r.change_identifier.as_str()),
            extras
                .pointer("/currentRevision/changeIdentifier")
                .and_then(Value::as_str),
            "case {case:?}: unexpected stored currentRevision.changeIdentifier, got {job:?}"
        );
        assert_eq!(
            job.current_revision
                .as_ref()
                .and_then(|r| r.created.as_ref().map(::aws_smithy_types::DateTime::secs)),
            extras
                .pointer("/currentRevision/created")
                .and_then(Value::as_i64),
            "case {case:?}: unexpected stored currentRevision.created, got {job:?}"
        );
        assert_eq!(
            job.current_revision
                .as_ref()
                .and_then(|r| r.revision_summary.as_deref()),
            extras
                .pointer("/currentRevision/revisionSummary")
                .and_then(Value::as_str),
            "case {case:?}: unexpected stored currentRevision.revisionSummary, got {job:?}"
        );
        assert_eq!(
            job.execution_details
                .as_ref()
                .and_then(|d| d.summary.as_deref()),
            extras
                .pointer("/executionDetails/summary")
                .and_then(Value::as_str),
            "case {case:?}: unexpected stored executionDetails.summary, got {job:?}"
        );
        assert_eq!(
            job.execution_details
                .as_ref()
                .and_then(|d| d.external_execution_id.as_deref()),
            extras
                .pointer("/executionDetails/externalExecutionId")
                .and_then(Value::as_str),
            "case {case:?}: unexpected stored executionDetails.externalExecutionId, got {job:?}"
        );
        assert_eq!(
            job.execution_details.as_ref().and_then(|d| d.percent_complete),
            extras
                .pointer("/executionDetails/percentComplete")
                .and_then(Value::as_i64)
                .and_then(|value| i32::try_from(value).ok()),
            "case {case:?}: unexpected stored executionDetails.percentComplete, got {job:?}"
        );

        // The job is complete, so the same result reported twice is refused rather
        // than replacing the outcome already recorded.
        let (status, code) = emulator.expect_error(OPERATION, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidJobStateException"),
            "case {case:?}: expected a repeated result to be refused, got {status} {code:?}"
        );
        let after = emulator.job(JOB_ID);
        assert_eq!(
            (after.status.as_str(), after.last_status_change),
            (job.status.as_str(), job.last_status_change),
            "case {case:?}: the refused repeat must leave the recorded result alone, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_job_reached_with_another_client_token_is_refused() {
    let longest_token = "t".repeat(MAX_CLIENT_TOKEN_LENGTH);
    let cases: Vec<(&str, &str)> = vec![
        ("a token no job was handed out with", "someone-elses-token"),
        ("the right token in upper case", "WORKER-TOKEN"),
        ("a prefix of the right token", "worker-toke"),
        ("the right token with a trailing space", "worker-token "),
        ("the job's own id used as a token", JOB_ID),
        (
            "a token at the modelled maximum length of 256",
            longest_token.as_str(),
        ),
    ];

    for (case, client_token) in cases {
        let emulator = Emulator::new();
        emulator.seed_job(JOB_ID, CLIENT_TOKEN, JOB_STATUS_IN_PROGRESS);
        let (status, code) = emulator
            .expect_error(
                OPERATION,
                &json!({"jobId": JOB_ID, "clientToken": client_token}),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "InvalidClientTokenException"),
            "case {case:?}: expected a 400 InvalidClientTokenException, got {status} {code:?}"
        );
        let job = emulator.job(JOB_ID);
        assert_eq!(
            job.status, JOB_STATUS_IN_PROGRESS,
            "case {case:?}: a refused call records no result, got {job:?}"
        );
    }

    // The control row: the token the job WAS handed out with reaches it.
    let emulator = Emulator::new();
    emulator.seed_job(JOB_ID, CLIENT_TOKEN, JOB_STATUS_IN_PROGRESS);
    emulator
        .ok(
            OPERATION,
            &json!({"jobId": JOB_ID, "clientToken": CLIENT_TOKEN}),
        )
        .await;
    let job = emulator.job(JOB_ID);
    assert_eq!(
        job.status, JOB_STATUS_SUCCEEDED,
        "case \"the token the job was handed out with\": expected {JOB_STATUS_SUCCEEDED}, got {job:?}"
    );
}

#[tokio::test]
async fn only_a_job_a_worker_currently_holds_accepts_a_result() {
    // Every value the model's `JobStatus` enum declares, from this service's own
    // generated catalog (`ops::types::JOB_STATUS_VALUES`): `Created`, `Dispatched`,
    // `Failed`, `InProgress`, `Queued`, `Succeeded`, `TimedOut`.
    assert_eq!(
        JOB_STATUS_VALUES.len(),
        7,
        "case \"the JobStatus catalog\": this table spans every modelled value, got {JOB_STATUS_VALUES:?}"
    );

    for status in JOB_STATUS_VALUES {
        let emulator = Emulator::new();
        emulator.seed_job(JOB_ID, CLIENT_TOKEN, status);
        let body = json!({"jobId": JOB_ID, "clientToken": CLIENT_TOKEN});

        if *status == JOB_STATUS_IN_PROGRESS {
            emulator.ok(OPERATION, &body).await;
            let job = emulator.job(JOB_ID);
            assert_eq!(
                job.status, JOB_STATUS_SUCCEEDED,
                "case {status:?}: a job the worker holds accepts the result, got {job:?}"
            );
        } else {
            let (code_status, code) = emulator.expect_error(OPERATION, &body).await;
            assert_eq!(
                (code_status, code.as_str()),
                (400, "InvalidJobStateException"),
                "case {status:?}: expected a 400 InvalidJobStateException, got {code_status} {code:?}"
            );
            let job = emulator.job(JOB_ID);
            assert_eq!(
                job.status.as_str(), *status,
                "case {status:?}: a refused call leaves the job where it was, got {job:?}"
            );
            assert_eq!(
                job.last_status_change,
                seeded_at(),
                "case {status:?}: a refused call records no status change, got {job:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_is_reached_by_id_whichever_partner_queue_it_waits_in() {
    // A worker names a job by its id alone, so the queue a job waits in cannot
    // change the answer: two partner action types, one job each, and each job's own
    // result reaches exactly its own job.
    let first = third_party_action_type("PartnerBuild");
    let second = third_party_action_type("PartnerDeploy");
    let cases: Vec<(&str, &ActionTypeId, &str, &str)> = vec![
        ("the first partner queue", &first, JOB_ID, CLIENT_TOKEN),
        (
            "the second partner queue",
            &second,
            "b2c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e",
            "other-worker-token",
        ),
    ];

    for (case, _action_type, job_id, client_token) in &cases {
        let emulator = Emulator::new();
        for (_, other_type, other_id, other_token) in &cases {
            emulator.seed_job_for(other_type, other_id, other_token, JOB_STATUS_IN_PROGRESS);
        }

        emulator
            .ok(
                OPERATION,
                &json!({"jobId": job_id, "clientToken": client_token}),
            )
            .await;

        for (other_case, _, other_id, _) in &cases {
            let job = emulator.job(other_id);
            let expected = if other_id == job_id {
                JOB_STATUS_SUCCEEDED
            } else {
                JOB_STATUS_IN_PROGRESS
            };
            assert_eq!(
                job.status, expected,
                "case {case:?} reporting for {job_id}: expected the job in {other_case:?} \
                 at {expected}, got {job:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_result_outside_a_modelled_constraint_is_refused_as_validation() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "a jobId one past the modelled maximum length of 512",
            json!({
                "jobId": "a".repeat(MAX_JOB_ID_LENGTH + 1),
                "clientToken": CLIENT_TOKEN,
            }),
        ),
        (
            "an empty jobId, below the modelled minimum length of 1",
            json!({"jobId": "", "clientToken": CLIENT_TOKEN}),
        ),
        (
            "a clientToken one past the modelled maximum length of 256",
            json!({
                "jobId": JOB_ID,
                "clientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH + 1),
            }),
        ),
        (
            "an empty clientToken, below the modelled minimum length of 1",
            json!({"jobId": JOB_ID, "clientToken": ""}),
        ),
        (
            "a continuationToken one past the modelled maximum length of 2048",
            json!({
                "jobId": JOB_ID,
                "clientToken": CLIENT_TOKEN,
                "continuationToken": "c".repeat(MAX_CONTINUATION_TOKEN_LENGTH + 1),
            }),
        ),
        (
            "percentComplete one past the published maximum of 100",
            json!({
                "jobId": JOB_ID,
                "clientToken": CLIENT_TOKEN,
                "executionDetails": {"percentComplete": 101},
            }),
        ),
        (
            "percentComplete one below the published minimum of 0",
            json!({
                "jobId": JOB_ID,
                "clientToken": CLIENT_TOKEN,
                "executionDetails": {"percentComplete": -1},
            }),
        ),
        (
            "no jobId at all, which the model requires",
            json!({"clientToken": CLIENT_TOKEN}),
        ),
        (
            "no clientToken at all, which the model requires",
            json!({"jobId": JOB_ID}),
        ),
    ];

    for (case, body) in cases {
        let emulator = Emulator::new();
        emulator.seed_job(JOB_ID, CLIENT_TOKEN, JOB_STATUS_IN_PROGRESS);
        let (status, code) = emulator.expect_error(OPERATION, &body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {code:?}"
        );
        let job = emulator.job(JOB_ID);
        assert_eq!(
            job.status, JOB_STATUS_IN_PROGRESS,
            "case {case:?}: a request refused before the handler records no result, got {job:?}"
        );
    }
}
