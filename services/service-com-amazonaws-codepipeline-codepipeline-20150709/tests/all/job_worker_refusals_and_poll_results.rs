// SPDX-License-Identifier: CC0-1.0

//! What the JOB-WORKER surface answers, over the wire the service actually speaks
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: CodePipeline_20150709.<Operation>`):
//! the wording of its refusals, and the shape of a poll's answer.
//!
//! Three rules, each asserted over a SET of inputs rather than over one call:
//!
//! 1. **A poll always reports its `jobs` list.** `PollForJobs` and
//!    `PollForThirdPartyJobs` both model an output list, and a poll with nothing
//!    waiting answers that list EMPTY rather than omitting the member — a worker loop
//!    reads `jobs` on every answer.
//! 2. **A custom action type this account registered stays pollable once it is
//!    deleted.** The delete marks the custom action deleted: it leaves the catalog and
//!    its ARN stops naming a resource, but the worker that was serving it is answered
//!    an empty poll. Only an identity the account NEVER registered is refused
//!    `ActionTypeNotFoundException`.
//! 3. **A refusal's message is the one the answering operation's family states**, built
//!    from the request. The job planes word a missing job three ways and the two
//!    action-type planes word a missing action type two ways, so each family is pinned
//!    here over a table of identities rather than on one id.
//!
//! Every call goes through `MultiServiceRouter::dispatch`; only the seeding of jobs the
//! execution plane would queue reaches the store directly, through its one write path.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::ops::types::ActionTypeId;
use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    ComAmazonawsCodepipelineCodepipeline20150709State, JobRecord,
};

use crate::pipeline_lifecycle::Emulator;

/// The `ActionOwner` values the model declares
/// (`smithy_query query="shape" shape="ActionOwner"` -> `AWS`, `ThirdParty`, `Custom`).
const OWNER_CUSTOM: &str = "Custom";
const OWNER_THIRD_PARTY: &str = "ThirdParty";

/// The refusals these operations model, with the status the model defaults them to.
const JOB_NOT_FOUND: &str = "JobNotFoundException";
const ACTION_TYPE_NOT_FOUND: &str = "ActionTypeNotFoundException";
const ACCESS_DENIED: &str = "AccessDeniedException";
const REFUSAL_STATUS: u16 = 400;

/// The identity a request names an action type by, as `ActionTypeId` declares the four
/// members.
fn id(category: &str, owner: &str, provider: &str, version: &str) -> ActionTypeId {
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

/// A value matching `JobId`'s modelled pattern
/// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), distinct per
/// `index`.
fn uuid_like(index: usize) -> String {
    format!("{index:08x}-0000-4000-8000-{index:012x}")
}

/// The awsJson error code (`__type`) a response carries.
fn error_code(operation: &str, response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{operation}: expected an error body, got {response}"))
        .to_string()
}

/// The message an error response carries under `key`, or the empty string.
fn message_under(key: &str, response: &Value) -> String {
    response
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Register a `Custom` action type with no configuration properties.
async fn register_custom(emulator: &Emulator, action_type: &ActionTypeId) {
    emulator
        .ok(
            "CreateCustomActionType",
            &json!({
                "category": action_type.category,
                "provider": action_type.provider,
                "version": action_type.version,
                "inputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
                "outputArtifactDetails": {"minimumCount": 0, "maximumCount": 5},
            }),
        )
        .await;
}

/// Mark that registration deleted.
async fn delete_custom(emulator: &Emulator, action_type: &ActionTypeId) {
    emulator
        .ok(
            "DeleteCustomActionType",
            &json!({
                "category": action_type.category,
                "provider": action_type.provider,
                "version": action_type.version,
            }),
        )
        .await;
}

/// Queue `count` jobs for `action_type`, oldest first, through the store's one write
/// path — the execution plane is what queues them in production, and what these rows
/// are about is the SHAPE of the answer.
fn queue_jobs(emulator: &Emulator, action_type: &ActionTypeId, count: usize) -> Vec<String> {
    let store = emulator
        .engine()
        .state::<ComAmazonawsCodepipelineCodepipeline20150709State>();
    let now = SystemTime::now();
    (0..count)
        .map(|index| {
            let job = JobRecord::new(uuid_like(index), action_type.clone(), now);
            let job_id = job.job_id.clone();
            let accepted = store.lock().enqueue_job(action_type, job);
            assert!(
                accepted,
                "seeding job {index} for {action_type:?}: expected the store to accept it, got \
                 refused"
            );
            job_id
        })
        .collect()
}

/// The ids a successful poll reported, in order.
fn reported_ids(response: &Value) -> Vec<String> {
    response["jobs"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a jobs list, got {response}"))
        .iter()
        .map(|job| job["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE 1 for `PollForJobs`: the `jobs` member is always serialized, and it is empty
/// exactly when nothing is waiting.
///
/// The rows differ along what the rule generalises over: how many jobs are waiting and
/// how many the caller asked for (`MaxBatchSize` is modelled `range 1..`, so 1 is the
/// smallest a request can carry, and one past the queue covers "asked for more than
/// there is").
#[tokio::test]
async fn a_custom_action_poll_always_reports_its_jobs_list() {
    for queued_count in 0..=3usize {
        for max_batch_size in [None, Some(1), Some(2), Some(4)] {
            let case = format!("{queued_count} queued, maxBatchSize {max_batch_size:?}");
            let emulator = Emulator::new();
            let action_type = id("Build", OWNER_CUSTOM, "My-Worker", "1");
            register_custom(&emulator, &action_type).await;
            let queued = queue_jobs(&emulator, &action_type, queued_count);

            let mut body = json!({"actionTypeId": action_type_json(&action_type)});
            if let Some(size) = max_batch_size {
                body["maxBatchSize"] = json!(size);
            }
            let (status, response) = emulator.call("PollForJobs", &body).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {response}"
            );
            assert!(
                response["jobs"].is_array(),
                "case {case:?}: expected the jobs list to be reported, got {response}"
            );
            let limit = max_batch_size.map_or(queued_count, |size| size as usize);
            let expected: Vec<String> = queued.into_iter().take(limit.min(queued_count)).collect();
            let actual = reported_ids(&response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}

/// RULE 1 for `PollForThirdPartyJobs`: the same rule through the partner plane's poll —
/// its registered partner with nothing waiting is answered an empty list, not a missing
/// member.
#[tokio::test]
async fn a_third_party_poll_always_reports_its_jobs_list() {
    // The rows differ in the category and the provider of the partner action type: the
    // rule is about the answer's shape and not about one identity.
    for (case, action_type) in [
        ("a partner source action", id("Source", OWNER_THIRD_PARTY, "ExampleScm", "1")),
        ("a partner test action", id("Test", OWNER_THIRD_PARTY, "A", "123456789")),
    ] {
        let emulator = Emulator::new();
        let accepted = emulator
            .engine()
            .state::<ComAmazonawsCodepipelineCodepipeline20150709State>()
            .lock()
            .register_third_party_partner(&action_type, "000000000000");
        assert!(
            accepted,
            "case {case:?}: seeding the partner registration must be accepted"
        );

        let (status, response) = emulator
            .call(
                "PollForThirdPartyJobs",
                &json!({"actionTypeId": action_type_json(&action_type)}),
            )
            .await;
        assert_eq!(
            (status, response.get("jobs")),
            (200, Some(&json!([]))),
            "case {case:?}: expected a 200 with an empty jobs list, got {status} {response}"
        );
    }
}

/// RULE 2: which identities a custom-action poll resolves. A registration the account
/// holds and one it has deleted are both answered a poll; an identity it never
/// registered is refused.
///
/// The rows differ along every member the rule generalises over — the category, the
/// provider and the version — and along the history of the identity: held, deleted,
/// deleted and registered again, never held, and a neighbour of a deleted one.
#[tokio::test]
async fn a_deleted_custom_action_type_stays_pollable_and_an_unregistered_one_does_not() {
    /// What was done to the identity before it was polled.
    #[derive(Debug, Clone, Copy)]
    enum History {
        Held,
        Deleted,
        Recreated,
        Never,
    }

    let rows: [(&str, ActionTypeId, History, bool); 6] = [
        (
            "a registration the account holds",
            id("Build", OWNER_CUSTOM, "My-Worker", "1"),
            History::Held,
            true,
        ),
        (
            "a registration the account deleted",
            id("Build", OWNER_CUSTOM, "My-Worker", "1"),
            History::Deleted,
            true,
        ),
        (
            "a deleted registration of another category",
            id("Test", OWNER_CUSTOM, "Other-Worker", "2"),
            History::Deleted,
            true,
        ),
        (
            "a deleted registration at the ends of the modelled lengths",
            id("Compute", OWNER_CUSTOM, "A", "123456789"),
            History::Deleted,
            true,
        ),
        (
            "a registration deleted and then made again",
            id("Deploy", OWNER_CUSTOM, "My-Worker", "1"),
            History::Recreated,
            true,
        ),
        (
            "an identity the account never registered",
            id("Build", OWNER_CUSTOM, "No-Such-Worker", "1"),
            History::Never,
            false,
        ),
    ];

    for (case, action_type, history, pollable) in rows {
        let emulator = Emulator::new();
        match history {
            History::Never => {}
            History::Held => register_custom(&emulator, &action_type).await,
            History::Deleted => {
                register_custom(&emulator, &action_type).await;
                delete_custom(&emulator, &action_type).await;
            }
            History::Recreated => {
                register_custom(&emulator, &action_type).await;
                delete_custom(&emulator, &action_type).await;
                register_custom(&emulator, &action_type).await;
            }
        }

        let body = json!({"actionTypeId": action_type_json(&action_type)});
        let (status, response) = emulator.call("PollForJobs", &body).await;
        if pollable {
            assert_eq!(
                (status, response.get("jobs")),
                (200, Some(&json!([]))),
                "case {case:?}: expected a 200 with an empty jobs list, got {status} {response}"
            );
        } else {
            assert_eq!(
                (status, error_code("PollForJobs", &response).as_str()),
                (REFUSAL_STATUS, ACTION_TYPE_NOT_FOUND),
                "case {case:?}: expected a {REFUSAL_STATUS} {ACTION_TYPE_NOT_FOUND}, got \
                 {status} {response}"
            );
        }

        // A deleted registration is pollable and nothing else: the catalog no longer
        // carries it, so the poll's resolution is not a registration coming back.
        if matches!(history, History::Deleted) {
            let listed = emulator.ok("ListActionTypes", &json!({})).await;
            let carried = listed["actionTypes"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .any(|entry| entry["id"] == action_type_json(&action_type));
            assert!(
                !carried,
                "case {case:?}: a deleted registration must be gone from the catalog, got \
                 {listed}"
            );
        }

        // A neighbour of the polled identity — the same provider at another version —
        // was never registered, so it is refused whatever happened to its neighbour.
        let neighbour = id(
            &action_type.category,
            OWNER_CUSTOM,
            &action_type.provider,
            "77",
        );
        let (status, response) = emulator
            .call("PollForJobs", &json!({"actionTypeId": action_type_json(&neighbour)}))
            .await;
        assert_eq!(
            (status, error_code("PollForJobs", &response).as_str()),
            (REFUSAL_STATUS, ACTION_TYPE_NOT_FOUND),
            "case {case:?}: another version of it was never registered, got {status} {response}"
        );
    }
}

/// RULE 2's other half: a job queued for an identity that is then deleted is not handed
/// out — the custom action is marked deleted, so the poll answers the worker with no
/// jobs and the queue is left as it was.
///
/// The rows differ in how many jobs were waiting when the delete landed, because the
/// rule is about the deleted identity and not about one queue depth.
#[tokio::test]
async fn a_deleted_action_types_queued_jobs_are_not_handed_out() {
    for queued_count in 1..=3usize {
        let case = format!("{queued_count} queued when the registration was deleted");
        let emulator = Emulator::new();
        let action_type = id("Build", OWNER_CUSTOM, "My-Worker", "1");
        register_custom(&emulator, &action_type).await;
        let queued = queue_jobs(&emulator, &action_type, queued_count);
        delete_custom(&emulator, &action_type).await;

        let (status, response) = emulator
            .call(
                "PollForJobs",
                &json!({"actionTypeId": action_type_json(&action_type)}),
            )
            .await;
        assert_eq!(
            (status, response.get("jobs")),
            (200, Some(&json!([]))),
            "case {case:?}: expected a 200 with no jobs, got {status} {response}"
        );

        // The jobs are still where they were: the poll neither handed them out nor
        // forgot them.
        let still_queued: Vec<String> = emulator
            .engine()
            .state::<ComAmazonawsCodepipelineCodepipeline20150709State>()
            .lock()
            .jobs
            .values()
            .flatten()
            .map(|job| job.job_id.clone())
            .collect();
        assert_eq!(
            still_queued, queued,
            "case {case:?}: expected the queue untouched, got {still_queued:?}"
        );
    }
}

/// RULE 2's refusal wording: `PollForJobs` states an identity it cannot resolve over
/// the request's own four members, each quoted and labelled, and nothing about the
/// store travels in it.
///
/// The rows differ along every member the rule generalises over: each `ActionCategory`
/// the model declares (`smithy_query query="shape" shape="ActionCategory"`), and
/// providers and versions at the ends of their modelled `@length` (`ActionProvider`
/// 1..35, `Version` 1..9).
#[tokio::test]
async fn an_unresolvable_custom_action_type_is_refused_as_unavailable() {
    let emulator = Emulator::new();
    // One registration the account does hold, so the refusals below are about the
    // identity each request named rather than about an empty registry.
    register_custom(&emulator, &id("Build", OWNER_CUSTOM, "My-Worker", "1")).await;

    let cases = [
        id("Source", OWNER_CUSTOM, "AbsentSource", "1"),
        id("Build", OWNER_CUSTOM, "My-Worker", "2"),
        id("Deploy", OWNER_CUSTOM, "A", "123456789"),
        id("Test", OWNER_CUSTOM, "Absent-Test_9", "2a"),
        id("Invoke", OWNER_CUSTOM, "Absent-Provider-0123456789012345678", "1"),
        id("Approval", OWNER_CUSTOM, "AbsentApproval", "1"),
        id("Compute", OWNER_CUSTOM, "AbsentCompute", "9"),
    ];

    let mut messages: Vec<String> = Vec::new();
    for polled in &cases {
        let case = format!(
            "{}/{}/{}/{}",
            polled.category, polled.owner, polled.provider, polled.version
        );
        let (status, response) = emulator
            .call("PollForJobs", &json!({"actionTypeId": action_type_json(polled)}))
            .await;
        assert_eq!(
            (status, error_code("PollForJobs", &response).as_str()),
            (REFUSAL_STATUS, ACTION_TYPE_NOT_FOUND),
            "case {case:?}: expected a {REFUSAL_STATUS} {ACTION_TYPE_NOT_FOUND}, got {status} \
             {response}"
        );
        let expected = format!(
            "ActionType (Category: '{}', Owner: '{}', Provider: '{}', Version: '{}') is not \
             available",
            polled.category, polled.owner, polled.provider, polled.version
        );
        let actual = message_under("message", &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the refusal {expected:?}, got {actual:?}"
        );
        messages.push(actual);
    }

    // Built from the request: no two identities are refused with the same text.
    let total = messages.len();
    messages.sort();
    messages.dedup();
    assert_eq!(
        messages.len(),
        total,
        "expected one refusal per identity, got {messages:?}"
    );
}

/// RULE 3 for the job planes: each family of job-worker operations states a missing job
/// in its OWN words, built from the id the request named — the custom-action
/// acknowledge/details family bare, the custom-action put-result family with the id
/// quoted, and the partner plane with the id quoted and the statement closed.
///
/// The rows are every operation of the three families; the ids differ along what the
/// rule generalises over (the value of the id, across the forms `JobId`'s modelled
/// pattern admits).
#[tokio::test]
async fn each_job_worker_family_states_a_missing_job_in_its_own_words() {
    /// How a family words it, given the id the request named.
    type Family = fn(&str) -> String;

    /// How one operation names the job it is called for: its request body.
    type Body = fn(&str, &Value) -> Value;

    /// One operation of one family: its name, its request body, and the wording its
    /// family states a missing job with.
    type Row = (&'static str, Body, Family);

    let bare: Family = |job_id| format!("Job with id {job_id} does not exist");
    let quoted: Family = |job_id| format!("Job with id '{job_id}' does not exist");
    let closed: Family = |job_id| format!("Job with id '{job_id}' does not exist.");

    // `FailureDetails` is `@required` on both failure results, with a `type` from
    // `FailureType` (`smithy_query query="shape" shape="FailureType"`) and a message.
    let failure = json!({"type": "JobFailed", "message": "nothing ran"});

    let operations: [Row; 7] = [
        (
            "AcknowledgeJob",
            |job_id, _| json!({"jobId": job_id, "nonce": "3"}),
            bare,
        ),
        ("GetJobDetails", |job_id, _| json!({"jobId": job_id}), bare),
        (
            "PutJobSuccessResult",
            |job_id, _| json!({"jobId": job_id}),
            quoted,
        ),
        (
            "PutJobFailureResult",
            |job_id, failure| json!({"jobId": job_id, "failureDetails": failure}),
            quoted,
        ),
        (
            "AcknowledgeThirdPartyJob",
            |job_id, _| json!({"jobId": job_id, "nonce": "3", "clientToken": "client-1"}),
            closed,
        ),
        (
            "PutThirdPartyJobSuccessResult",
            |job_id, _| json!({"jobId": job_id, "clientToken": "client-1"}),
            closed,
        ),
        (
            "PutThirdPartyJobFailureResult",
            |job_id, failure| {
                json!({
                    "jobId": job_id,
                    "clientToken": "client-1",
                    "failureDetails": failure,
                })
            },
            closed,
        ),
    ];

    // Every id is a `JobId` (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`),
    // which `ThirdPartyJobId` (length 1..512) also admits, so one table covers both
    // planes: all zeroes, every hex digit, and all `f`s.
    let job_ids = [
        "00000000-0000-4000-8000-000000000000",
        "0123456f-89ab-4cde-8f01-23456789abcd",
        "ffffffff-ffff-4fff-bfff-ffffffffffff",
    ];

    let emulator = Emulator::new();
    for job_id in job_ids {
        for (operation, body, expected_message) in operations {
            let case = format!("{operation} for {job_id}");
            let (status, response) = emulator.call(operation, &body(job_id, &failure)).await;
            assert_eq!(
                (status, error_code(operation, &response).as_str()),
                (REFUSAL_STATUS, JOB_NOT_FOUND),
                "case {case:?}: expected a {REFUSAL_STATUS} {JOB_NOT_FOUND}, got {status} \
                 {response}"
            );
            let expected = expected_message(job_id);
            let actual = message_under("message", &response);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the refusal {expected:?}, got {actual:?}"
            );
        }
    }
}

/// RULE 3 for the partner poll: its authorization refusal carries the cause, under the
/// key the AWS common-error envelope uses for it.
///
/// The rows differ in the category, the provider and the version of the action type
/// polled, because the refusal is built from the request's own members.
#[tokio::test]
async fn the_partner_polls_refusal_carries_its_cause() {
    let emulator = Emulator::new();

    for polled in [
        id("Source", OWNER_THIRD_PARTY, "ExampleScm", "1"),
        id("Build", OWNER_THIRD_PARTY, "A", "123456789"),
        id("Test", OWNER_THIRD_PARTY, "Partner-Nobody_9", "2a"),
    ] {
        let case = format!(
            "{}/{}/{}/{}",
            polled.category, polled.owner, polled.provider, polled.version
        );
        let (status, response) = emulator
            .call(
                "PollForThirdPartyJobs",
                &json!({"actionTypeId": action_type_json(&polled)}),
            )
            .await;
        assert_eq!(
            (status, error_code("PollForThirdPartyJobs", &response).as_str()),
            (REFUSAL_STATUS, ACCESS_DENIED),
            "case {case:?}: expected a {REFUSAL_STATUS} {ACCESS_DENIED}, got {status} {response}"
        );
        let expected = format!(
            "Not whitelisted for accessing ActionType.Category='{}' ActionType.Owner='{}' \
             ActionType.Provider='{}' ActionType.Version='{}'",
            polled.category, polled.owner, polled.provider, polled.version
        );
        let actual = message_under("Message", &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the refusal {expected:?} under the envelope's own key, got \
             {response}"
        );
    }
}
