// SPDX-License-Identifier: CC0-1.0

//! `DeleteQuotaShare`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * a delete whose preconditions hold answers an empty 200 and leaves the share in the
//!   non-terminal `DELETING` status: the REMOVAL is owed by the clock, not by the call, and
//!   once it falls due the share's own reader and its queue's listing no longer hold it
//!   while every other share, and the queue itself, is untouched;
//! * repeating a delete already accepted asks for a removal the clock already owes, so it
//!   is the same success, and so is a delete of a share the clock has already removed — or
//!   of one that was never stored: the post-state the call asks for already holds;
//! * the one refusal the identifier can earn is the FRONTEND's: a `quotaShareArn` that is no
//!   quota-share ARN at all is the modelled `ClientException` (HTTP 400) inside the envelope
//!   that layer wraps its own refusals in, before any lookup. The one refusal the RECORD can
//!   earn is the service's own precondition, stated bare: a share whose state still accepts
//!   submissions must be `DISABLED` first. Neither changes anything;
//! * when the removal falls due, the service jobs the share carried end `FAILED` with the
//!   reason the removal states, and a job of another share, of no share, or of a share of
//!   the same name on another queue is left exactly as it was.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state::{self, ServiceJob};

use crate::job_lifecycle::{
    arn, assert_clock_owed_status, bracketed, compute_environment, error_code, error_message,
    job_queue, Batch, REGION,
};
use crate::refusal_contract::ENVELOPE_REQUEST_ID;
use crate::service_job_submission::{sagemaker_queue, service_environment};

/// `DeleteQuotaShare`'s route (`smithy_query(query="operation",
/// operation="DeleteQuotaShare")`: POST /v1/deletequotashare).
const DELETE: &str = "/v1/deletequotashare";

/// `CreateQuotaShare`'s route.
const CREATE: &str = "/v1/createquotashare";

/// `UpdateQuotaShare`'s route — how a share reaches `DISABLED` after it was created
/// accepting submissions.
const UPDATE: &str = "/v1/updatequotashare";

/// `DescribeQuotaShare`'s route — the reader a delete is proven through.
const DESCRIBE: &str = "/v1/describequotashare";

/// `ListQuotaShares`' route — the other reader a removal must be visible in.
const LIST: &str = "/v1/listquotashares";

/// The queue the shares under test hang off.
const QUEUE: &str = "jq-deleted-shares";

/// How long to wait for a clock-owed transition to fall due: the span the clock owes it
/// ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// The create body for a share called `name` on `queue`, with `extra` laid over it.
fn create_body(name: &str, queue: &str, extra: Value) -> Value {
    let mut body = json!({
        "quotaShareName": name,
        "jobQueue": queue,
        "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
        "resourceSharingConfiguration": {"strategy": "RESERVE"},
        "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// The ARN a share of `name` on `queue` is stored under.
fn share_arn(queue: &str, name: &str) -> String {
    arn(&format!("job-queue/{queue}/quota-share/{name}"))
}

/// Create the queue the shares hang off, over a compute environment of its own.
async fn queue(batch: &Batch) {
    let environment = compute_environment(batch, &format!("ce-for-{QUEUE}")).await;
    job_queue(batch, QUEUE, &environment).await;
}

/// Create a share and answer the ARN the create reports.
async fn share(batch: &Batch, name: &str, queue: &str, extra: Value) -> String {
    let created = batch.ok(CREATE, create_body(name, queue, extra)).await;
    created["quotaShareArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DeleteQuotaShare` over the wire: the status and the decoded body.
async fn delete(batch: &Batch, reference: &str) -> (u16, Value) {
    batch
        .call(DELETE, json!({"quotaShareArn": reference}))
        .await
}

/// `DescribeQuotaShare` over the wire: the status and the decoded body.
async fn describe(batch: &Batch, reference: &str) -> (u16, Value) {
    batch.call(DESCRIBE, json!({"quotaShareArn": reference})).await
}

/// `DescribeQuotaShare`, requiring the 200.
async fn described(batch: &Batch, case: &str, reference: &str) -> Value {
    let (status, body) = describe(batch, reference).await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeQuotaShare to answer 200, got {status} {body}"
    );
    body
}

/// The ARNs `ListQuotaShares` reports for `queue`.
async fn listed_arns(batch: &Batch, queue: &str) -> Vec<String> {
    let body = batch.ok(LIST, json!({"jobQueue": queue})).await;
    body["quotaShares"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|share| {
            share["quotaShareArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Assert a `DeleteQuotaShare` answered the empty body its output shape models
/// (`smithy_query(query="operation", operation="DeleteQuotaShare")`: no output members).
fn assert_empty_body(case: &str, body: &Value) {
    let empty = body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty);
    assert!(
        empty,
        "case {case}: DeleteQuotaShare models no output member, got {body}"
    );
}

/// How a row's share reached the `DISABLED` state a delete requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disabled {
    /// The create asked for `DISABLED`.
    AtCreate,
    /// The create left it accepting submissions and an update disabled it, with the create
    /// already settled.
    ByUpdateWhenSettled,
    /// The same update, applied while the create the clock owes is still pending.
    ByUpdateWhilePending,
}

/// One deletable share: what the row varies, and how it got to `DISABLED`.
struct Row {
    label: &'static str,
    name: String,
    extra: Value,
    disabled: Disabled,
}

/// The table of shares this suite deletes.
///
/// Rows differ in HOW the share reached `DISABLED` (at the create, by an update of a settled
/// share, and by an update of one whose create is still pending), in the length and alphabet
/// of the share name (including the documented bound, [`state::MAX_QUOTA_SHARE_NAME_LEN`]),
/// in how many capacity limits the share allocates and of which units, in the sharing
/// strategy and whether it carries a `borrowLimit`, in the preemption state, and in whether
/// the share carries tags.
///
/// The enum values are the sets the model declares (`smithy_query(query="shape",
/// shape="QuotaShareState,QuotaShareResourceSharingStrategy,
/// QuotaShareInSharePreemptionState")`: `ENABLED | DISABLED`, `RESERVE | LEND |
/// LEND_AND_BORROW`, `ENABLED | DISABLED`), and the capacity units are of the form
/// `QuotaShareCapacityLimit$capacityUnit` documents ("For example, `ml.m5.large`").
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "created DISABLED, one capacity limit",
            name: "qs-born-disabled".to_string(),
            extra: json!({"state": "DISABLED"}),
            disabled: Disabled::AtCreate,
        },
        Row {
            label: "created DISABLED with tags and three capacity limits",
            name: "qs_Born_Disabled-2".to_string(),
            extra: json!({
                "state": "DISABLED",
                "tags": {"team": "research", "cost-centre": "42"},
                "capacityLimits": [
                    {"maxCapacity": 0, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 3, "capacityUnit": "ml.m5.2xlarge"},
                    {"maxCapacity": 9, "capacityUnit": "ml.p4d.24xlarge"},
                ],
            }),
            disabled: Disabled::AtCreate,
        },
        Row {
            label: "created DISABLED, LEND_AND_BORROW with a borrowLimit",
            name: "q".to_string(),
            extra: json!({
                "state": "DISABLED",
                "resourceSharingConfiguration": {"strategy": "LEND_AND_BORROW", "borrowLimit": 200},
                "preemptionConfiguration": {"inSharePreemption": "ENABLED"},
            }),
            disabled: Disabled::AtCreate,
        },
        Row {
            label: "created ENABLED and disabled by an update once settled",
            name: "qs-disabled-later".to_string(),
            extra: json!({"state": "ENABLED", "resourceSharingConfiguration": {"strategy": "LEND"}}),
            disabled: Disabled::ByUpdateWhenSettled,
        },
        Row {
            label: "created with no state at all (ENABLED by default) and disabled by an update",
            name: "qs-default-state".to_string(),
            extra: json!({}),
            disabled: Disabled::ByUpdateWhenSettled,
        },
        Row {
            label: "disabled by an update while the create is still pending, name at the bound",
            name: "q".repeat(state::MAX_QUOTA_SHARE_NAME_LEN),
            extra: json!({"state": "ENABLED"}),
            disabled: Disabled::ByUpdateWhilePending,
        },
    ]
}

/// RULE: a delete whose preconditions hold answers an empty 200, leaves the share in
/// `DELETING`, and the REMOVAL is owed by the clock — after which neither reader holds the
/// share, while a sibling share and the queue itself are untouched.
#[tokio::test]
async fn an_accepted_delete_reports_deleting_and_the_clock_removes_the_share() {
    let batch = Batch::new();
    queue(&batch).await;

    // A share nothing in this test deletes: every removal below must leave it alone.
    let sibling = share(&batch, "qs-sibling", QUEUE, json!({"state": "DISABLED"})).await;

    let mut deleted: Vec<(String, String)> = Vec::new();
    for row in rows() {
        let case = row.label;
        let reference = share(&batch, &row.name, QUEUE, row.extra.clone()).await;
        match row.disabled {
            Disabled::AtCreate => {}
            Disabled::ByUpdateWhenSettled => {
                wait_for_the_clock().await;
                batch
                    .ok(
                        UPDATE,
                        json!({"quotaShareArn": reference, "state": "DISABLED"}),
                    )
                    .await;
            }
            Disabled::ByUpdateWhilePending => {
                batch
                    .ok(
                        UPDATE,
                        json!({"quotaShareArn": reference, "state": "DISABLED"}),
                    )
                    .await;
            }
        }

        let accepted = bracketed(delete(&batch, &reference)).await;
        let (status, body) = accepted.value.clone();
        assert_eq!(
            status, 200,
            "case {case}: expected the empty 200 for a DISABLED share, got {status} {body}"
        );
        assert_empty_body(case, &body);

        // Straight after the call the share is still there, in DELETING: the removal is a
        // transition the clock owes, and the span is never zero.
        let fresh = bracketed(described(&batch, case, &reference)).await;
        assert_clock_owed_status(
            case,
            &fresh,
            accepted.due_window(),
            state::STATUS_DELETING,
            None,
        );
        assert_eq!(
            fresh.value["state"].as_str(),
            Some(state::STATE_DISABLED),
            "case {case}: the delete must not touch the share's state, got {}",
            fresh.value
        );
        // The listing agrees with the share's own reader while the removal is pending.
        assert!(
            listed_arns(&batch, QUEUE).await.contains(&reference),
            "case {case}: a share whose removal is still owed is still listed"
        );

        // Repeating the delete asks for a removal that is already owed: the same success.
        let (status, repeated) = delete(&batch, &reference).await;
        assert_eq!(
            status, 200,
            "case {case}: expected the same empty 200 for a DELETING share, got {status} \
             {repeated}"
        );
        assert_empty_body(case, &repeated);

        deleted.push((case.to_string(), reference));
    }

    // The CLOCK, not a read, removes every one of them.
    wait_for_the_clock().await;
    let listed = listed_arns(&batch, QUEUE).await;
    for (case, reference) in &deleted {
        let (status, refused) = describe(&batch, reference).await;
        assert_eq!(
            status, 400,
            "case {case}: once the removal falls due the share's reader must refuse it, got \
             {status} {refused}"
        );
        assert_eq!(
            error_message(&refused),
            state::does_not_exist_sentence(reference),
            "case {case}: expected the reader's bare nonexistence refusal, got {refused}"
        );        assert!(
            !listed.contains(reference),
            "case {case}: a removed share must leave the listing, got {listed:?}"
        );
        // The delete stays idempotent: the removal it asks for is already done.
        let (status, accepted) = delete(&batch, reference).await;
        assert_eq!(
            status, 200,
            "case {case}: deleting a removed share is the same empty 200, got {status} \
             {accepted}"
        );
        assert_empty_body(case, &accepted);
    }
    // Nothing else went with them: the sibling share and the queue are exactly as they were.
    assert_eq!(
        listed,
        vec![sibling.clone()],
        "case after every removal: only the shares that were deleted may go, got {listed:?}"
    );
    let still_there = described(&batch, "the sibling share", &sibling).await;
    assert_eq!(
        still_there["quotaShareArn"].as_str(),
        Some(sibling.as_str()),
        "case the sibling share: a delete of another share must leave it, got {still_there}"
    );
}

/// RULE: the only refusals a delete can answer are the frontend's unusable identifier and
/// the service's own `DISABLED` precondition — each the modelled `ClientException` (HTTP
/// 400) that changes nothing — while an identifier naming nothing stored is a removal that
/// is already done.
#[tokio::test]
async fn a_delete_refuses_what_it_cannot_serve_and_changes_nothing() {
    let batch = Batch::new();
    queue(&batch).await;
    let reference = share(&batch, "qs-refusals", QUEUE, json!({"state": "ENABLED"})).await;

    // A value that is no quota-share ARN at all: rows differ in HOW it fails the pattern —
    // not an ARN, empty, an ARN of another resource kind, a queue ARN with no share path,
    // and a share path with no name.
    for malformed in [
        "not-an-arn".to_string(),
        String::new(),
        arn(&format!("job-queue/{QUEUE}")),
        arn("quota-share/qs-refusals"),
        arn(&format!("job-queue/{QUEUE}/quota-share/")),
        arn(&format!("compute-environment/ce-for-{QUEUE}")),
    ] {
        let (status, refused) = delete(&batch, &malformed).await;
        assert_eq!(
            status, 400,
            "case delete by {malformed:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case delete by {malformed:?}: expected the modelled ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains(state::QUOTA_SHARE_ARN_NOT_VALID),
            "case delete by {malformed:?}: expected the ARN-pattern refusal, got {message:?}"
        );
        assert!(
            message.contains(ENVELOPE_REQUEST_ID),
            "case delete by {malformed:?}: the frontend's own refusal states the call it \
             refused, got {message:?}"
        );
    }

    // A well-formed ARN naming nothing stored is NOT a refusal: the removal it asks for has
    // already happened. Rows differ in whether the queue the ARN nests under exists, and in
    // whether the share name does.
    for unknown in [
        share_arn(QUEUE, "qs-absent"),
        share_arn("no-such-queue", "qs-refusals"),
        share_arn("no-such-queue", "qs-absent"),
    ] {
        let (status, accepted) = delete(&batch, &unknown).await;
        assert_eq!(
            status, 200,
            "case delete the unknown share {unknown:?}: expected the empty 200 for a removal \
             that is already done, got {status} {accepted}"
        );
        assert_empty_body(&format!("delete the unknown share {unknown:?}"), &accepted);
        // And nothing was stored for it: the reader still reports it nonexistent.
        let (status, refused) = describe(&batch, &unknown).await;
        assert_eq!(
            status, 400,
            "case delete the unknown share {unknown:?}: nothing may be stored for it, got \
             {status} {refused}"
        );
        assert_eq!(
            error_message(&refused),
            state::does_not_exist_sentence(&unknown),
            "case delete the unknown share {unknown:?}: expected the reader's bare sentence"
        );
    }

    // A share that still accepts submissions must be DISABLED first: rows differ in how it
    // came to be enabled — the create said so, the create defaulted to it, and an update put
    // it back — and in whether its create has settled.
    let enabling: Vec<(&str, String)> = vec![
        ("the create asked for ENABLED", reference.clone()),
        (
            "the create named no state at all",
            share(&batch, "qs-default-enabled", QUEUE, json!({})).await,
        ),
        ("an update enabled it again", {
            let disabled = share(
                &batch,
                "qs-enabled-again",
                QUEUE,
                json!({"state": "DISABLED"}),
            )
            .await;
            batch
                .ok(UPDATE, json!({"quotaShareArn": disabled, "state": "ENABLED"}))
                .await;
            disabled
        }),
    ];
    for (case, enabled) in enabling {
        let before = described(&batch, case, &enabled).await;
        let (status, refused) = delete(&batch, &enabled).await;
        assert_eq!(
            status, 400,
            "case delete an ENABLED share, {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case delete an ENABLED share, {case}: expected the modelled ClientException, got \
             {refused}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message,
            state::QUOTA_SHARE_MUST_BE_DISABLED,
            "case delete an ENABLED share, {case}: expected the precondition's own sentence, \
             stated bare"
        );
        assert!(
            !message.contains(ENVELOPE_REQUEST_ID),
            "case delete an ENABLED share, {case}: the service's own precondition is stated \
             bare, got {message:?}"
        );
        // The refused delete wrote nothing: the share is exactly as it was, and the clock
        // still owes it only what it owed before.
        let after = described(&batch, case, &enabled).await;
        assert_eq!(
            after["state"].as_str(),
            before["state"].as_str(),
            "case delete an ENABLED share, {case}: a refused delete must leave the state, got \
             {after} after {before}"
        );
        assert_ne!(
            after["status"].as_str(),
            Some(state::STATUS_DELETING),
            "case delete an ENABLED share, {case}: a refused delete must not start a removal, \
             got {after}"
        );
    }

    // And the share the refusals named is still there once the clock has had every chance to
    // remove something it was never asked to remove.
    wait_for_the_clock().await;
    let still_there = described(&batch, "after every refusal", &reference).await;
    assert_eq!(
        still_there["state"].as_str(),
        Some(state::STATE_ENABLED),
        "case after every refusal: the share must survive untouched, got {still_there}"
    );
}

/// Seed one `RUNNABLE` service job on `queue_arn` carrying `quota_share_name`, and answer
/// its id.
///
/// A seeded record is how a row reaches a service job that owes the clock nothing: a job
/// nothing can place RESTS in `RUNNABLE` (`state` module docs), so what the removal below
/// does to it is the only thing that moves it. The record is written through the same state
/// API the submit handler uses, which is the store the handlers read.
fn seed_service_job(batch: &Batch, name: &str, queue_arn: &str, quota_share: Option<&str>) -> String {
    let store = batch.store();
    let mut store = store.lock();
    let id = store.new_uuid();
    let job = ServiceJob {
        job_id: id.clone(),
        job_arn: state::service_job_arn(REGION, &id),
        job_name: name.to_string(),
        job_queue_arn: queue_arn.to_string(),
        service_job_type: "SAGEMAKER_TRAINING".to_string(),
        status: state::SERVICE_JOB_RUNNABLE.to_string(),
        created_at: state::now_millis(),
        // Resting: only a call may move it.
        next_phase_due_at: None,
        quota_share_name: quota_share.map(str::to_string),
        ..ServiceJob::default()
    };
    store
        .insert_service_job(job)
        .expect("the seeded service job is stored");
    id
}

/// `DescribeServiceJob`, requiring the 200.
async fn described_job(batch: &Batch, case: &str, id: &str) -> Value {
    let (status, body) = batch
        .call("/v1/describeservicejob", json!({"jobId": id}))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeServiceJob to answer 200, got {status} {body}"
    );
    body
}

/// RULE: when a share's removal falls due, the service jobs that share carried end `FAILED`
/// with the reason the removal states, and no other job is touched.
#[tokio::test]
async fn the_removal_ends_the_service_jobs_the_share_carried() {
    let batch = Batch::new();
    // The jobs under test are service jobs, so they hang off a SAGEMAKER_TRAINING queue.
    let environment = service_environment(&batch, "se-deleted-shares").await;
    let queue_arn = sagemaker_queue(&batch, "jq-share-jobs", &environment).await;
    let other_environment = service_environment(&batch, "se-deleted-shares-2").await;
    let other_queue_arn =
        sagemaker_queue(&batch, "jq-share-jobs-other", &other_environment).await;

    let doomed = share(
        &batch,
        "qs-carrier",
        "jq-share-jobs",
        json!({"state": "DISABLED"}),
    )
    .await;
    share(
        &batch,
        "qs-bystander",
        "jq-share-jobs",
        json!({"state": "DISABLED"}),
    )
    .await;
    // The same share NAME on another queue is another share: its jobs are not these.
    share(
        &batch,
        "qs-carrier",
        "jq-share-jobs-other",
        json!({"state": "DISABLED"}),
    )
    .await;

    // (case, job id, whether the removal must end it)
    let carried = seed_service_job(&batch, "sj-carried", &queue_arn, Some("qs-carrier"));
    let untouched = vec![
        (
            "a job of another share on the same queue",
            seed_service_job(&batch, "sj-bystander", &queue_arn, Some("qs-bystander")),
        ),
        (
            "a job naming no share at all",
            seed_service_job(&batch, "sj-no-share", &queue_arn, None),
        ),
        (
            "a job of a share of the same name on another queue",
            seed_service_job(
                &batch,
                "sj-other-queue",
                &other_queue_arn,
                Some("qs-carrier"),
            ),
        ),
    ];

    let before: Vec<(&str, String, Value)> = {
        let mut rows = Vec::new();
        for (case, id) in &untouched {
            let record = described_job(&batch, case, id).await;
            rows.push((*case, id.clone(), record));
        }
        rows
    };

    let (status, accepted) = delete(&batch, &doomed).await;
    assert_eq!(
        status, 200,
        "case delete the carrier share: expected the empty 200, got {status} {accepted}"
    );
    // The removal is owed by the clock, so the job it ends is still running right now.
    let pending = described_job(&batch, "the carried job before the removal", &carried).await;
    assert_eq!(
        pending["status"].as_str(),
        Some(state::SERVICE_JOB_RUNNABLE),
        "case the carried job before the removal: the delete itself must not end it, got \
         {pending}"
    );

    wait_for_the_clock().await;

    let ended = described_job(&batch, "the carried job", &carried).await;
    assert_eq!(
        ended["status"].as_str(),
        Some(state::SERVICE_JOB_FAILED),
        "case the carried job: expected the removal to end it, got {ended}"
    );
    assert_eq!(
        ended["statusReason"].as_str(),
        Some(state::QUOTA_SHARE_DELETED_REASON),
        "case the carried job: expected the removal's own reason, got {ended}"
    );
    assert_eq!(
        ended["isTerminated"].as_bool(),
        Some(true),
        "case the carried job: a job the service ended is terminated, got {ended}"
    );

    for (case, id, record) in &before {
        let now = described_job(&batch, case, id).await;
        assert_eq!(
            now, *record,
            "case {case}: the removal of another share must leave it, got {now} after {record}"
        );
    }
}
