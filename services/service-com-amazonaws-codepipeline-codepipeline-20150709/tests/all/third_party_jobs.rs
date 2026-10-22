// SPDX-License-Identifier: CC0-1.0

//! `PollForThirdPartyJobs` end to end over the wire the service actually speaks:
//! `awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`.
//!
//! The two rules the handler is:
//!
//! 1. CodePipeline serves an action type's third-party jobs ONLY to the account
//!    registered as that action type's partner; any other caller is refused
//!    `AccessDeniedException` (HTTP 400), whatever action type it names and whether or
//!    not a job is waiting.
//! 2. To that partner it reports the jobs waiting for the named action type, oldest
//!    first, at most `maxBatchSize` of them, and leaves them queued.
//!
//! The partner registry and the job queue are seeded through
//! `ComAmazonawsCodepipelineCodepipeline20150709State`, the only authoring seam they
//! have: AWS registers a partner integration out of band (no modelled operation
//! creates one) and the action-execution plane that queues jobs is not part of this
//! slice. Everything else goes through `MultiServiceRouter::dispatch`, so nothing here
//! can pass by reading a handler's internals.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State, ThirdPartyJobRecord,
};
use service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix OPERATIONS.md records for this service.
const TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The operation under test.
const POLL: &str = "PollForThirdPartyJobs";

/// The canonical emulator account id every request is signed by, and therefore the
/// caller's account (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";

/// Another account, standing in for a partner that is not the caller. Satisfies the
/// 12-digit account form and is not [`ACCOUNT_ID`].
const OTHER_ACCOUNT_ID: &str = "210987654321";

/// The error code the AWS "Common Errors" table names for an authorization refusal,
/// with the status that table publishes for it.
const ACCESS_DENIED: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 400;

/// The `ActionOwner` values the model declares
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`,
/// `Custom`). `ThirdParty` is the only one a partner integration can carry.
const OWNER_THIRD_PARTY: &str = "ThirdParty";
const OWNER_AWS: &str = "AWS";
const OWNER_CUSTOM: &str = "Custom";

/// One emulator: the shared engine, the service mounted on it, and the router that
/// dispatches to it — exactly how `core/router` assembles the production emulator.
struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// This service's store, the way a handler reaches it.
    fn store(
        &self,
    ) -> engine::ServiceState<ComAmazonawsCodepipelineCodepipeline20150709State> {
        self.engine
            .state::<ComAmazonawsCodepipelineCodepipeline20150709State>()
    }

    /// Register `account_id` as the partner for `action_type`, asserting the store
    /// accepted it (only a `ThirdParty` action type has a partner).
    fn register(&self, action_type: &ActionTypeId, account_id: &str) {
        let accepted = self
            .store()
            .lock()
            .register_third_party_partner(action_type, account_id);
        assert!(
            accepted,
            "seeding a partner for {action_type:?}: expected the store to accept the \
             registration, got refused"
        );
    }

    /// Queue `count` jobs for `action_type`, oldest first, and return the records as
    /// queued.
    fn queue_jobs(&self, action_type: &ActionTypeId, count: usize) -> Vec<ThirdPartyJobRecord> {
        let queued: Vec<ThirdPartyJobRecord> = (0..count)
            .map(|index| ThirdPartyJobRecord::new(uuid_like(index), uuid_like(index + 100)))
            .collect();
        for job in &queued {
            let accepted = self
                .store()
                .lock()
                .enqueue_third_party_job(action_type, job.clone());
            assert!(
                accepted,
                "seeding a job for {action_type:?}: expected the store to accept it, got \
                 refused"
            );
        }
        queued
    }

    /// Call `POLL` with `action_type` and an optional `maxBatchSize`.
    async fn poll(&self, action_type: &ActionTypeId, max_batch_size: Option<i32>) -> (u16, Value) {
        let mut body = json!({"actionTypeId": action_type_json(action_type)});
        if let Some(size) = max_batch_size {
            body["maxBatchSize"] = json!(size);
        }
        self.send(POLL, &body).await
    }

    async fn send(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header(
                "authorization",
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/codepipeline/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=0123456789abcdef",
            )
            .body(bytes::Bytes::from(body.to_string()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = response.into_body();
        let parsed = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or_else(|e| {
                panic!("{operation}: response body is not JSON ({e}): {body:?}")
            })
        };
        (status, parsed)
    }
}

/// An `ActionTypeId`'s four identifying fields, every one `@required`.
fn action_type(category: &str, owner: &str, provider: &str, version: &str) -> ActionTypeId {
    ActionTypeId {
        category: category.to_string(),
        owner: owner.to_string(),
        provider: provider.to_string(),
        version: version.to_string(),
    }
}

fn action_type_json(id: &ActionTypeId) -> Value {
    json!({
        "category": id.category,
        "owner": id.owner,
        "provider": id.provider,
        "version": id.version,
    })
}

/// The partner action type the registered cases use: `ActionProvider` and `Version`
/// are modelled `^[0-9A-Za-z_-]+$`, and `Source` is an `ActionCategory` value.
fn partner_action_type() -> ActionTypeId {
    action_type("Source", OWNER_THIRD_PARTY, "ExampleScm", "1")
}

/// A value matching `ThirdPartyJob$jobId` / `$clientId`'s modelled pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), distinct per
/// `index`.
fn uuid_like(index: usize) -> String {
    format!("{index:08x}-0000-4000-8000-{index:012x}")
}

/// The awsJson error code (`__type`) and message an error response carries.
fn error_of(operation: &str, response: &Value) -> (String, String) {
    let code = response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .to_string();
    let message = response
        .get("message")
        .or_else(|| response.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    (code, message)
}

/// The `(clientId, jobId)` pairs a successful poll reported, in order.
fn reported_jobs(response: &Value) -> Vec<(String, String)> {
    response
        .get("jobs")
        .map(|jobs| {
            jobs.as_array()
                .unwrap_or_else(|| panic!("jobs is not a list: {response}"))
                .iter()
                .map(|job| {
                    let client_id = job
                        .get("clientId")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("a job without clientId: {job}"))
                        .to_string();
                    let job_id = job
                        .get("jobId")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| panic!("a job without jobId: {job}"))
                        .to_string();
                    (client_id, job_id)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the refusal table: the case name, the partner registration the store
/// holds (if any) and the action type the request polls for.
type DenialRow = (&'static str, Option<(ActionTypeId, &'static str)>, ActionTypeId);

/// Rule 1, over the class of callers that are not an action type's partner: the
/// registry holds no entry, holds one for another account, or holds one for a
/// DIFFERENT action type — differing in each of the four identifying fields in turn,
/// including the two owners that can never carry a partner integration at all.
#[tokio::test]
async fn a_caller_that_is_not_the_registered_partner_is_refused() {
    let registered = partner_action_type();
    // (case, what the registry holds, what the request polls for)
    let rows: [DenialRow; 8] = [
        ("no partner registered at all", None, partner_action_type()),
        (
            "registered to another account",
            Some((registered.clone(), OTHER_ACCOUNT_ID)),
            partner_action_type(),
        ),
        (
            "another account, nothing of the caller's registered",
            Some((
                action_type("Build", OWNER_THIRD_PARTY, "OtherScm", "3"),
                OTHER_ACCOUNT_ID,
            )),
            partner_action_type(),
        ),
        (
            "an AWS-owned action type is never a partner action",
            Some((registered.clone(), ACCOUNT_ID)),
            action_type("Source", OWNER_AWS, "S3", "1"),
        ),
        (
            "a Custom-owned action type is never a partner action",
            Some((registered.clone(), ACCOUNT_ID)),
            action_type("Build", OWNER_CUSTOM, "MyBuilder", "1"),
        ),
        (
            "a different category",
            Some((registered.clone(), ACCOUNT_ID)),
            action_type("Build", OWNER_THIRD_PARTY, "ExampleScm", "1"),
        ),
        (
            "a different provider",
            Some((registered.clone(), ACCOUNT_ID)),
            action_type("Source", OWNER_THIRD_PARTY, "OtherScm", "1"),
        ),
        (
            "a different version",
            Some((registered.clone(), ACCOUNT_ID)),
            action_type("Source", OWNER_THIRD_PARTY, "ExampleScm", "2"),
        ),
    ];

    for (case, seed, polled) in rows {
        let emulator = Emulator::new();
        if let Some((seeded_type, account_id)) = &seed {
            emulator.register(seeded_type, account_id);
        }
        // A job waiting changes nothing about the refusal.
        emulator.queue_jobs(&partner_action_type(), 1);

        let (status, response) = emulator.poll(&polled, None).await;
        let (code, message) = error_of(POLL, &response);
        assert_eq!(
            status, ACCESS_DENIED_STATUS,
            "case {case:?}: expected HTTP {ACCESS_DENIED_STATUS}, got {status} with {response}"
        );
        assert_eq!(
            code, ACCESS_DENIED,
            "case {case:?}: expected error code {ACCESS_DENIED}, got {code:?} with {response}"
        );
        // The refusal is about the action type the REQUEST named, so it names that
        // action type's four identifying fields — the wording the service states for
        // an account that is not whitelisted for it.
        let expected_message = format!(
            "Not whitelisted for accessing ActionType.Category='{}' ActionType.Owner='{}' \
             ActionType.Provider='{}' ActionType.Version='{}'",
            polled.category, polled.owner, polled.provider, polled.version
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected message {expected_message:?}, got {message:?}"
        );
    }
}

/// Rule 2 as an invariant, exhaustively over how many jobs are queued and how many the
/// caller asked for: the registered partner is reported the first `min(queued,
/// maxBatchSize)` jobs, oldest first, and an absent `maxBatchSize` asks for every one.
#[tokio::test]
async fn a_partner_is_reported_the_queued_jobs_capped_by_max_batch_size() {
    // `MaxBatchSize` is modelled `range 1..`, so 1 is the smallest value a request can
    // carry; one past the queue length covers the "asked for more than there is" edge.
    let batch_sizes: [Option<i32>; 6] = [None, Some(1), Some(2), Some(3), Some(4), Some(5)];

    for queued_count in 0..=4usize {
        for max_batch_size in batch_sizes {
            let case = format!("{queued_count} queued, maxBatchSize {max_batch_size:?}");
            let emulator = Emulator::new();
            let action_type = partner_action_type();
            emulator.register(&action_type, ACCOUNT_ID);
            let queued = emulator.queue_jobs(&action_type, queued_count);

            let (status, response) = emulator.poll(&action_type, max_batch_size).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );

            let limit = max_batch_size.map_or(queued_count, |size| size as usize);
            let expected: Vec<(String, String)> = queued
                .iter()
                .take(limit.min(queued_count))
                .map(|job| (job.client_id.clone(), job.job_id.clone()))
                .collect();
            let actual = reported_jobs(&response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the first {} queued jobs oldest first, got {actual:?}",
                expected.len()
            );
            // An empty list is reported as an ABSENT member, never as `[]`.
            if expected.is_empty() {
                assert!(
                    response.get("jobs").is_none(),
                    "case {case:?}: expected no jobs member at all, got {response}"
                );
            }
        }
    }
}

/// Polling is a READ: the jobs it reveals stay queued, so a second poll — by the same
/// worker or another one — sees exactly the same jobs.
#[tokio::test]
async fn polling_leaves_the_jobs_queued() {
    let emulator = Emulator::new();
    let action_type = partner_action_type();
    emulator.register(&action_type, ACCOUNT_ID);
    emulator.queue_jobs(&action_type, 3);

    let (_, first) = emulator.poll(&action_type, Some(2)).await;
    let (_, second) = emulator.poll(&action_type, Some(2)).await;
    assert_eq!(
        reported_jobs(&first),
        reported_jobs(&second),
        "case \"two polls in a row\": expected the same jobs both times, got {:?} then {:?}",
        reported_jobs(&first),
        reported_jobs(&second)
    );

    let still_queued = emulator
        .store()
        .lock()
        .third_party_jobs_for(&action_type, usize::MAX)
        .len();
    assert_eq!(
        still_queued, 3,
        "case \"two polls in a row\": expected all 3 jobs still queued, got {still_queued}"
    );
}

/// A poll reports the jobs of the action type it NAMED: a partner registered for two
/// action types is reported each one's own queue.
#[tokio::test]
async fn each_action_types_jobs_are_reported_only_for_that_action_type() {
    let emulator = Emulator::new();
    let source = partner_action_type();
    let build = action_type("Build", OWNER_THIRD_PARTY, "ExampleScm", "1");
    emulator.register(&source, ACCOUNT_ID);
    emulator.register(&build, ACCOUNT_ID);
    let source_jobs = emulator.queue_jobs(&source, 2);

    let (status, response) = emulator.poll(&source, None).await;
    assert_eq!(
        status, 200,
        "case \"the action type with jobs\": expected HTTP 200, got {status} with {response}"
    );
    let expected: Vec<(String, String)> = source_jobs
        .iter()
        .map(|job| (job.client_id.clone(), job.job_id.clone()))
        .collect();
    let actual = reported_jobs(&response);
    assert_eq!(
        actual, expected,
        "case \"the action type with jobs\": expected {expected:?}, got {actual:?}"
    );

    let (status, response) = emulator.poll(&build, None).await;
    assert_eq!(
        status, 200,
        "case \"the action type with no jobs\": expected HTTP 200, got {status} with {response}"
    );
    let actual = reported_jobs(&response);
    assert!(
        actual.is_empty(),
        "case \"the action type with no jobs\": expected no jobs, got {actual:?}"
    );
}

/// Only a `ThirdParty` action type can carry a partner integration or a third-party
/// job queue: the store refuses both for either other `ActionOwner` value, so no
/// reader has to re-check the owner.
#[tokio::test]
async fn only_a_third_party_action_type_can_carry_a_partner_or_a_job() {
    let emulator = Emulator::new();
    for owner in [OWNER_AWS, OWNER_CUSTOM] {
        let id = action_type("Source", owner, "ExampleScm", "1");
        let store = emulator.store();
        let mut store = store.lock();
        let registered = store.register_third_party_partner(&id, ACCOUNT_ID);
        assert!(
            !registered,
            "case {owner:?}: expected the store to refuse a partner registration, got accepted"
        );
        let queued = store.enqueue_third_party_job(&id, ThirdPartyJobRecord::new(uuid_like(1), uuid_like(2)));
        assert!(
            !queued,
            "case {owner:?}: expected the store to refuse a queued job, got accepted"
        );
    }
}
