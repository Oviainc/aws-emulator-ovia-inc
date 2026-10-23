// SPDX-License-Identifier: CC0-1.0

//! `AcknowledgeThirdPartyJob`: a partner action's job worker confirming it received
//! one third-party job, over the wire this service actually speaks (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule these tables pin, one row per way the rule generalises:
//!
//! * a `jobId` no queued job carries is refused `JobNotFoundException` whatever the
//!   id's FORM and whatever credentials accompany it — `ThirdPartyJobId` carries no
//!   modelled pattern, so only whether a job is filed under the id decides this;
//! * a job reached with a `clientToken` other than the one it was handed out with is
//!   refused `InvalidClientTokenException`;
//! * a `nonce` that is not the job's current one — and any nonce at all once the
//!   job's work has ended — is refused `InvalidNonceException`, so one job is granted
//!   to only one worker;
//! * otherwise the receipt is recorded: the job moves to `InProgress` in the store,
//!   the response reports that persisted status, repeating the call is idempotent,
//!   and `PutThirdPartyJobSuccessResult` — which accepts a result only for a job a
//!   worker holds — now accepts one, which is the acknowledgement read back through
//!   another operation.
//!
//! `PollForThirdPartyJobs` reports the jobs waiting for a partner action type, but the
//! action-execution plane that QUEUES them is not part of this slice, so — as in
//! `tests/all/third_party_jobs.rs` and `tests/all/third_party_job_results.rs` — these
//! tests queue their jobs through `ComAmazonawsCodepipelineCodepipeline20150709State`
//! (`enqueue_third_party_job`), the only authoring seam the queue has, which is the
//! SAME store the handler reads and writes. Every call itself goes through the router.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::{
    ActionTypeId, JOB_STATUS_VALUES,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State as CodePipelineState, ThirdPartyJobRecord,
    JOB_STATUS_IN_PROGRESS, JOB_STATUS_QUEUED, TERMINAL_JOB_STATUSES,
};

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The operation under test.
const ACKNOWLEDGE: &str = "AcknowledgeThirdPartyJob";

/// The operation that reads the acknowledgement back: it accepts a result only for a
/// job a worker holds.
const PUT_SUCCESS: &str = "PutThirdPartyJobSuccessResult";

/// The `clientId` half of the pair a job is handed to, in the form
/// `ThirdPartyJob$clientId` models
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`).
const CLIENT_ID: &str = "7f1e2d3c-4b5a-4978-8a9b-0c1d2e3f4a5b";

/// `ThirdPartyJobId`'s modelled length bounds
/// (`smithy_query query="shape" shape="ThirdPartyJobId"` -> len 1..512).
const JOB_ID_MIN: usize = 1;
const JOB_ID_MAX: usize = 512;

/// `Nonce`'s modelled length bounds
/// (`smithy_query query="shape" shape="Nonce"` -> len 1..50).
const NONCE_MIN: usize = 1;
const NONCE_MAX: usize = 50;

/// `ClientToken`'s modelled length bounds
/// (`smithy_query query="shape" shape="ClientToken"` -> len 1..256).
const CLIENT_TOKEN_MIN: usize = 1;
const CLIENT_TOKEN_MAX: usize = 256;

/// The `ActionOwner` value a partner integration carries
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`): the store queues a third-party job only for a `ThirdParty` action type.
const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// When a seeded job's status last changed: a fixed past instant, so a change the
/// handler records is strictly later than it.
fn seeded_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000)
}

/// The identity of the `ThirdParty` action type whose queue a seeded job waits in.
fn partner_action_type() -> ActionTypeId {
    ActionTypeId {
        category: "Build".to_string(),
        owner: OWNER_THIRD_PARTY.to_string(),
        provider: "PartnerBuild".to_string(),
        version: "1".to_string(),
    }
}

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
///
/// The engine is kept for the same reason the sibling third-party suites keep it: the
/// action-execution plane that queues a partner action's jobs is not part of this
/// slice, so the store is the only place a job can come from — and it is what this
/// operation writes through, so it is also where the write is checked.
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

    /// Queue a job for the partner action type, as the action-execution plane does
    /// when an execution reaches a partner action: with the `clientToken` and `nonce`
    /// a worker has to present, and the `JobStatus` the job is in.
    fn seed_job(&self, job_id: &str, client_token: &str, nonce: &str, status: &str) {
        let mut job = ThirdPartyJobRecord::new(job_id, CLIENT_ID);
        job.client_token = client_token.to_string();
        job.nonce = nonce.to_string();
        job.status = status.to_string();
        job.last_status_change = seeded_at();
        let queued = self
            .engine
            .state::<CodePipelineState>()
            .lock()
            .enqueue_third_party_job(&partner_action_type(), job);
        assert!(
            queued,
            "seeding job {job_id}: the store must queue a job for a ThirdParty action type"
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

    async fn acknowledge(&self, job_id: &str, nonce: &str, client_token: &str) -> (u16, Value) {
        self.call(
            ACKNOWLEDGE,
            &json!({"jobId": job_id, "nonce": nonce, "clientToken": client_token}),
        )
        .await
    }

    async fn put_success(&self, job_id: &str, client_token: &str) -> (u16, Value) {
        self.call(
            PUT_SUCCESS,
            &json!({"jobId": job_id, "clientToken": client_token}),
        )
        .await
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
}

/// The awsJson error code (`__type`) an error body carries.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an error body, got {body}"))
        .to_string()
}

/// The `message` an error body carries, under either casing the error envelopes use.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected an error message, got {body}"))
        .to_string()
}

/// The `status` member a successful response reports.
fn reported_status(body: &Value) -> String {
    body.get("status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a status member, got {body}"))
        .to_string()
}

/// A string of `len` characters, for exercising a modelled length bound.
fn sized(len: usize, fill: char) -> String {
    std::iter::repeat_n(fill, len).collect()
}

/// `value` with its last character replaced by `replacement`: a DIFFERENT value of
/// the same length, so altering a value that already sits at its modelled maximum
/// does not turn the case into a length violation.
fn altered(value: &str, replacement: char) -> String {
    let mut chars: Vec<char> = value.chars().collect();
    let last = chars.len() - 1;
    assert_ne!(
        chars[last], replacement,
        "the alteration must change {value:?}"
    );
    chars[last] = replacement;
    chars.into_iter().collect()
}

/// An id no queued job carries is not a job: `JobNotFoundException`, with the message
/// built from the requested id — whatever the nonce and client token are, because
/// there is no job yet for either to be checked against.
#[tokio::test]
async fn an_id_no_queued_job_carries_is_not_a_job() {
    let emulator = Emulator::new();
    // One real job, so every refusal below is a lookup miss and not an empty store.
    emulator.seed_job(
        "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d",
        "stored-token",
        "stored-nonce",
        JOB_STATUS_QUEUED,
    );

    // (case, jobId, nonce, clientToken)
    let cases: Vec<(&str, String, &str, &str)> = vec![
        (
            "uuid-form id never queued",
            "00000000-0000-0000-0000-000000000000".to_string(),
            "any-nonce",
            "any-token",
        ),
        (
            "shortest id the model allows",
            sized(JOB_ID_MIN, 'x'),
            "any-nonce",
            "any-token",
        ),
        (
            "longest id the model allows",
            sized(JOB_ID_MAX, 'j'),
            "any-nonce",
            "any-token",
        ),
        (
            "opaque partner id",
            "partner/job:7f3a".to_string(),
            "any-nonce",
            "any-token",
        ),
        (
            "the stored id differing only in case",
            "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d".to_uppercase(),
            "stored-nonce",
            "stored-token",
        ),
        (
            "a prefix of the stored id",
            "a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5".to_string(),
            "stored-nonce",
            "stored-token",
        ),
        (
            "the stored credentials on an absent id",
            "another-job".to_string(),
            "stored-nonce",
            "stored-token",
        ),
    ];

    for (case, job_id, nonce, client_token) in cases {
        let (status, body) = emulator.acknowledge(&job_id, nonce, client_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "JobNotFoundException",
            "case {case:?}: expected JobNotFoundException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Job with id '{job_id}' does not exist."),
            "case {case:?}: expected the message to name the requested id, got {body}"
        );
    }

    let stored = emulator.job("a1b2c3d4-e5f6-4a7b-8c9d-0e1f2a3b4c5d");
    assert_eq!(
        stored.status, JOB_STATUS_QUEUED,
        "a refused acknowledgement must leave the queued job where it was, got {:?}",
        stored.status
    );
}

/// Every modelled length bound of this operation's three members, at the bound and
/// one past it: a value inside the bound reaches the lookup (and is answered from the
/// store), one outside it never does — the boundary validator answers
/// `ValidationException`.
#[tokio::test]
async fn a_value_outside_a_published_bound_never_reaches_the_lookup() {
    let emulator = Emulator::new();

    // (case, jobId, nonce, clientToken, expected error code)
    let cases: Vec<(&str, String, String, String, &str)> = vec![
        (
            "jobId at its minimum",
            sized(JOB_ID_MIN, 'j'),
            "n".to_string(),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "jobId at its maximum",
            sized(JOB_ID_MAX, 'j'),
            "n".to_string(),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "jobId one below its minimum",
            sized(JOB_ID_MIN - 1, 'j'),
            "n".to_string(),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "jobId one past its maximum",
            sized(JOB_ID_MAX + 1, 'j'),
            "n".to_string(),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "nonce at its minimum",
            "absent-job".to_string(),
            sized(NONCE_MIN, 'n'),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "nonce at its maximum",
            "absent-job".to_string(),
            sized(NONCE_MAX, 'n'),
            "t".to_string(),
            "JobNotFoundException",
        ),
        (
            "nonce one below its minimum",
            "absent-job".to_string(),
            sized(NONCE_MIN - 1, 'n'),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "nonce one past its maximum",
            "absent-job".to_string(),
            sized(NONCE_MAX + 1, 'n'),
            "t".to_string(),
            "ValidationException",
        ),
        (
            "clientToken at its minimum",
            "absent-job".to_string(),
            "n".to_string(),
            sized(CLIENT_TOKEN_MIN, 't'),
            "JobNotFoundException",
        ),
        (
            "clientToken at its maximum",
            "absent-job".to_string(),
            "n".to_string(),
            sized(CLIENT_TOKEN_MAX, 't'),
            "JobNotFoundException",
        ),
        (
            "clientToken one below its minimum",
            "absent-job".to_string(),
            "n".to_string(),
            sized(CLIENT_TOKEN_MIN - 1, 't'),
            "ValidationException",
        ),
        (
            "clientToken one past its maximum",
            "absent-job".to_string(),
            "n".to_string(),
            sized(CLIENT_TOKEN_MAX + 1, 't'),
            "ValidationException",
        ),
    ];

    for (case, job_id, nonce, client_token, expected) in cases {
        let (status, body) = emulator.acknowledge(&job_id, &nonce, &client_token).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// A queued job admits exactly the `clientToken` it was handed out with and exactly
/// its current `nonce`; anything else is refused with the error modelled for that
/// half of the pair and leaves the job queued. The accepted call is the only one that
/// moves it, repeating it is idempotent, and a second worker is then refused.
#[tokio::test]
async fn a_job_admits_only_the_client_token_and_nonce_it_was_handed_out_with() {
    // (case, jobId, nonce, clientToken) — the rows differ along every value the rule
    // generalises over: the id's form and length, the nonce's length, the token's.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "uuid-form id, shortest nonce and token",
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee".to_string(),
            sized(NONCE_MIN, 'n'),
            sized(CLIENT_TOKEN_MIN, 't'),
        ),
        (
            "opaque id, nonce and token at their maxima",
            "partner/job:0001".to_string(),
            sized(NONCE_MAX, 'n'),
            sized(CLIENT_TOKEN_MAX, 't'),
        ),
        (
            "longest id, mixed-case nonce",
            sized(JOB_ID_MAX, 'j'),
            "NoNcE-2f8c".to_string(),
            "partner-client-token".to_string(),
        ),
        (
            "unicode nonce and token",
            "job-ünïcode".to_string(),
            "nonce-ünïcode".to_string(),
            "token-ünïcode".to_string(),
        ),
    ];

    for (case, job_id, nonce, client_token) in cases {
        let emulator = Emulator::new();
        emulator.seed_job(&job_id, &client_token, &nonce, JOB_STATUS_QUEUED);

        // A client token the job was not handed out with.
        let (status, body) = emulator
            .acknowledge(&job_id, &nonce, &altered(&client_token, 'z'))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidClientTokenException"),
            "case {case:?}: another client token must not be admitted, got {status} {body}"
        );

        // A nonce that is not the job's current one.
        let (status, body) = emulator
            .acknowledge(&job_id, &altered(&nonce, 'z'), &client_token)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNonceException"),
            "case {case:?}: another nonce must not be admitted, got {status} {body}"
        );

        // The pair swapped between the two members is neither half's value — checked
        // only where the swap keeps both inside their own modelled lengths, since a
        // nonce longer than 50 is a boundary violation rather than a wrong nonce.
        if client_token.chars().count() <= NONCE_MAX && nonce.chars().count() >= CLIENT_TOKEN_MIN {
            let (status, body) = emulator.acknowledge(&job_id, &client_token, &nonce).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidClientTokenException"),
                "case {case:?}: the swapped pair must not be admitted, got {status} {body}"
            );
        }

        let refused = emulator.job(&job_id);
        assert_eq!(
            refused.status, JOB_STATUS_QUEUED,
            "case {case:?}: a refused acknowledgement must leave the job {JOB_STATUS_QUEUED}"
        );
        assert_eq!(
            refused.last_status_change,
            seeded_at(),
            "case {case:?}: a refused acknowledgement must not re-stamp the job"
        );

        // The pair the job was handed out with: the receipt is recorded, and the
        // status reported is the one now in the store.
        let (status, body) = emulator.acknowledge(&job_id, &nonce, &client_token).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the job's own pair must be admitted, got {status} with {body}"
        );
        assert_eq!(
            reported_status(&body),
            JOB_STATUS_IN_PROGRESS,
            "case {case:?}: expected status {JOB_STATUS_IN_PROGRESS}, got {body}"
        );
        let acknowledged = emulator.job(&job_id);
        assert_eq!(
            acknowledged.status, JOB_STATUS_IN_PROGRESS,
            "case {case:?}: the store must hold the acknowledged status, got {:?}",
            acknowledged.status
        );
        assert!(
            acknowledged.last_status_change > seeded_at(),
            "case {case:?}: the acknowledgement must stamp the job, got {:?}",
            acknowledged.last_status_change
        );

        // Repeating it is idempotent: same answer, and the record does not move again.
        let (status, body) = emulator.acknowledge(&job_id, &nonce, &client_token).await;
        assert_eq!(
            (status, reported_status(&body)),
            (200, JOB_STATUS_IN_PROGRESS.to_string()),
            "case {case:?}: re-acknowledging must report the stored status, got {status} {body}"
        );
        assert_eq!(
            emulator.job(&job_id).last_status_change,
            acknowledged.last_status_change,
            "case {case:?}: an idempotent acknowledgement must not re-stamp the job"
        );

        // A second worker's nonce is still refused, and the job stays where it is.
        let (status, body) = emulator
            .acknowledge(&job_id, &altered(&nonce, 'q'), &client_token)
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidNonceException"),
            "case {case:?}: a second worker must not take the job, got {status} {body}"
        );
        assert_eq!(
            emulator.job(&job_id).status,
            JOB_STATUS_IN_PROGRESS,
            "case {case:?}: the refusal must leave the acknowledged status in place"
        );
    }
}

/// The job's own status decides whether its nonce still admits a worker, over the
/// whole catalog of modelled `JobStatus` values (`JOB_STATUS_VALUES`): a job whose
/// work has ended ([`TERMINAL_JOB_STATUSES`]) is being worked on by nobody, so no
/// nonce admits it, while every live status accepts the job's nonce and reports
/// `InProgress`.
#[tokio::test]
async fn a_jobs_status_decides_whether_its_nonce_still_admits_a_worker() {
    for status_value in JOB_STATUS_VALUES {
        let emulator = Emulator::new();
        let job_id = format!("job-{status_value}");
        emulator.seed_job(&job_id, "the-token", "the-nonce", status_value);

        let (status, body) = emulator
            .acknowledge(&job_id, "the-nonce", "the-token")
            .await;

        if TERMINAL_JOB_STATUSES.contains(status_value) {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidNonceException"),
                "case {status_value:?}: a finished job admits no worker, got {status} {body}"
            );
            assert_eq!(
                emulator.job(&job_id).status.as_str(),
                *status_value,
                "case {status_value:?}: a refused acknowledgement must not change the status"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {status_value:?}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                reported_status(&body),
                JOB_STATUS_IN_PROGRESS,
                "case {status_value:?}: expected status {JOB_STATUS_IN_PROGRESS}, got {body}"
            );
            assert_eq!(
                emulator.job(&job_id).status,
                JOB_STATUS_IN_PROGRESS,
                "case {status_value:?}: the store must hold the acknowledged status"
            );
        }
    }
}

/// The acknowledgement, read back through another operation: a worker may report a
/// result only for a job it holds, so `PutThirdPartyJobSuccessResult` refuses a job
/// that has not been acknowledged (`InvalidJobStateException`) and accepts one that
/// has. Nothing here reads the handler's internals — the second call is a second
/// request against the same store.
#[tokio::test]
async fn an_acknowledged_job_is_the_one_whose_result_is_accepted() {
    let emulator = Emulator::new();
    let job_id = "b2c3d4e5-f6a7-4b8c-9d0e-1f2a3b4c5d6e";
    emulator.seed_job(job_id, "worker-token", "worker-nonce", JOB_STATUS_QUEUED);

    let (status, body) = emulator.put_success(job_id, "worker-token").await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidJobStateException"),
        "case \"result before the acknowledgement\": expected the result to be refused, \
         got {status} with {body}"
    );

    let (status, body) = emulator
        .acknowledge(job_id, "worker-nonce", "worker-token")
        .await;
    assert_eq!(
        (status, reported_status(&body)),
        (200, JOB_STATUS_IN_PROGRESS.to_string()),
        "case \"the acknowledgement\": expected HTTP 200 and {JOB_STATUS_IN_PROGRESS}, \
         got {status} with {body}"
    );

    let (status, body) = emulator.put_success(job_id, "worker-token").await;
    assert_eq!(
        status, 200,
        "case \"result after the acknowledgement\": expected HTTP 200, got {status} with {body}"
    );
    assert!(
        !emulator.job(job_id).accepts_result(),
        "case \"result after the acknowledgement\": the reported result must end the job"
    );
}
