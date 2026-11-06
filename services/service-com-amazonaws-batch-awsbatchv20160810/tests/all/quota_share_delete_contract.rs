// SPDX-License-Identifier: CC0-1.0

//! What `DeleteQuotaShare` answers for an identifier it cannot serve and for a share that
//! still accepts submissions, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client branches on. The expected sentences are
//! written out here as literals rather than read from the crate's constants, so a reworded
//! message fails this suite instead of moving with it.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! * a `quotaShareArn` whose SHAPE names no quota share — anything carrying no
//!   `job-queue/<queue>/quota-share/<share>` path — is refused BEFORE any lookup with the
//!   modelled `ClientException` (HTTP 400) inside Batch's frontend envelope, whose only
//!   variable part is the refused call's OWN request id: the same string the response reports
//!   in the header an SDK surfaces as the call's request id. The rule belongs to the
//!   identifier, not to one handler, so every operation that takes a `quotaShareArn` answers
//!   it identically;
//! * a share whose state still accepts submissions cannot be removed: the delete is refused
//!   with the same modelled `ClientException` (HTTP 400) carrying the precondition's own
//!   sentence, stated bare — no envelope and no request id in the message;
//! * both refusals are effect-free: the share keeps its name, its state and its place in its
//!   queue's listing, and a share no call named is untouched;
//! * once the state no longer accepts submissions the same delete is accepted with the empty
//!   body its output shape models, and when the removal the clock owes falls due the share's
//!   own reader AND its updater both report the dead identifier nonexistent.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, compute_environment, error_code, error_message, job_queue, Batch};
use crate::refusal_contract::assert_uuid_shaped;

/// The routes of the quota-share operations under test (`smithy_query(query="operation",
/// operation="DeleteQuotaShare,DescribeQuotaShare,UpdateQuotaShare,CreateQuotaShare,
/// ListQuotaShares")`: POST /v1/deletequotashare, /v1/describequotashare,
/// /v1/updatequotashare, /v1/createquotashare, /v1/listquotashares).
const DELETE: &str = "/v1/deletequotashare";
const DESCRIBE: &str = "/v1/describequotashare";
const UPDATE: &str = "/v1/updatequotashare";
const CREATE: &str = "/v1/createquotashare";
const LIST: &str = "/v1/listquotashares";

/// The header AWS reports a response's own request id in, and the name botocore reads
/// `ResponseMetadata.RequestId` off.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The prefix of Batch's frontend envelope: the fixed text of a refusal that layer raises.
const ENVELOPE_PREFIX: &str = "Error executing request, Exception : ";

/// The segment that envelope names the refused call's request id in.
const ENVELOPE_REQUEST_ID: &str = ", RequestId: ";

/// The rule a `quotaShareArn` of the wrong shape broke, as the quota-share plane states it.
const ARN_PATTERN_DETAIL: &str = "Quota Share arn should match a valid Quota Share arn pattern";

/// The precondition sentence a delete answers while the share still accepts submissions.
const MUST_BE_DISABLED: &str = "Quota Share must be DISABLED before deleting.";

/// The queue the shares under test hang off.
const QUEUE: &str = "jq-share-delete-contract";

/// How long to wait for a clock-owed transition to fall due: the span the clock owes it
/// ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// Create the queue the shares hang off, over a compute environment of its own.
async fn queue(batch: &Batch) {
    let environment = compute_environment(batch, &format!("ce-for-{QUEUE}")).await;
    job_queue(batch, QUEUE, &environment).await;
}

/// Create a share called `name` with `extra` laid over the create body, and answer the ARN
/// the create reports.
async fn share(batch: &Batch, name: &str, extra: Value) -> String {
    let mut body = json!({
        "quotaShareName": name,
        "jobQueue": QUEUE,
        "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
        "resourceSharingConfiguration": {"strategy": "RESERVE"},
        "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let created = batch.ok(CREATE, body).await;
    created["quotaShareArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DescribeQuotaShare`, requiring the 200.
async fn described(batch: &Batch, case: &str, reference: &str) -> Value {
    let (status, body) = batch
        .call(DESCRIBE, json!({"quotaShareArn": reference}))
        .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeQuotaShare to answer 200, got {status} {body}"
    );
    body
}

/// The ARNs `ListQuotaShares` reports for this suite's queue.
async fn listed_arns(batch: &Batch) -> Vec<String> {
    let body = batch.ok(LIST, json!({"jobQueue": QUEUE})).await;
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

/// One operation that takes a `quotaShareArn`: its route and the body carrying `reference`.
struct Taker {
    label: &'static str,
    uri: &'static str,
    body: fn(&str) -> Value,
}

/// Every operation whose identifier is a `quotaShareArn`, so the shape rule is shown to be
/// the identifier's and not one handler's: the remover, the reader and the updater.
fn takers() -> Vec<Taker> {
    vec![
        Taker {
            label: "DeleteQuotaShare",
            uri: DELETE,
            body: |reference| json!({"quotaShareArn": reference}),
        },
        Taker {
            label: "DescribeQuotaShare",
            uri: DESCRIBE,
            body: |reference| json!({"quotaShareArn": reference}),
        },
        Taker {
            label: "UpdateQuotaShare",
            uri: UPDATE,
            body: |reference| json!({"quotaShareArn": reference, "state": "DISABLED"}),
        },
    ]
}

/// Identifiers that carry no `job-queue/<queue>/quota-share/<share>` path, so they name no
/// quota share whichever operation takes them.
///
/// The rows differ along what the rule generalises over: the KIND of malformation (not an ARN
/// at all, an ARN with nothing after the account, an ARN of another Batch resource kind, a
/// queue ARN with no share path, the two path segments in the wrong order), and WHICH part of
/// the share path is missing or empty (no queue name, no share name, a nested share name).
fn malformed_identifiers() -> Vec<(&'static str, String)> {
    vec![
        // The share path with no job-queue segment at all.
        (
            "an ARN with no job-queue segment",
            arn("quota-share/qs-shape"),
        ),
        ("an empty identifier", String::new()),
        ("not an ARN at all", "qs-shape".to_string()),
        ("an ARN with an empty resource", arn("")),
        (
            "a job-queue ARN with no share path",
            arn(&format!("job-queue/{QUEUE}")),
        ),
        (
            "an ARN with an empty share name",
            arn(&format!("job-queue/{QUEUE}/quota-share/")),
        ),
        (
            "an ARN with an empty queue name",
            arn("job-queue//quota-share/qs-shape"),
        ),
        (
            "an ARN whose share name carries a nested path",
            arn(&format!("job-queue/{QUEUE}/quota-share/qs-shape/extra")),
        ),
        (
            "the two path segments in the wrong order",
            arn(&format!("quota-share/qs-shape/job-queue/{QUEUE}")),
        ),
        (
            "a compute-environment ARN",
            arn(&format!("compute-environment/ce-for-{QUEUE}")),
        ),
        (
            "a scheduling-policy ARN",
            arn("scheduling-policy/sp-shape"),
        ),
    ]
}

/// RULE: a `quotaShareArn` of a shape that names no quota share is refused 400 /
/// `ClientException` with the frontend envelope's fixed text around the ARN-pattern rule,
/// whose only variable part is the refused call's own request id — the value the response
/// reports in its request-id header — and the refusal changes nothing.
#[tokio::test]
async fn a_quota_share_arn_of_the_wrong_shape_is_refused_by_every_taker() {
    let batch = Batch::new();
    queue(&batch).await;

    // A real share none of these calls names: every refusal below must leave it exactly as it
    // is. The clock has settled it first, so what it reports is stable.
    let bystander = share(&batch, "qs-bystander", json!({"state": "ENABLED"})).await;
    wait_for_the_clock().await;
    let before = described(&batch, "the bystander share", &bystander).await;

    let mut reported_ids: Vec<String> = Vec::new();
    for (label, identifier) in malformed_identifiers() {
        for taker in takers() {
            let case = format!("{label} on {}", taker.label);
            let (status, headers, refused) = batch
                .send_reported("POST", taker.uri, Some((taker.body)(&identifier)))
                .await;
            assert_eq!(
                status, 400,
                "case {case}: expected the modelled ClientException 400, got {status} {refused}"
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case {case}: expected the modelled ClientException, got {refused}"
            );
            let message = error_message(&refused);
            let reported = headers
                .get(REQUEST_ID_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            assert_uuid_shaped(&case, &reported);
            assert_eq!(
                message,
                format!("{ENVELOPE_PREFIX}{ARN_PATTERN_DETAIL}{ENVELOPE_REQUEST_ID}{reported}"),
                "case {case}: expected the enveloped ARN-pattern refusal naming this call's \
                 own request id {reported:?}, got {message:?}"
            );
            reported_ids.push(reported);
        }
    }

    // The id is the CALL's, so no two of those refusals named the same request.
    let mut distinct = reported_ids.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        reported_ids.len(),
        "case every refused call: expected one request id per call, got {reported_ids:?}"
    );

    // Nothing was written: the share no call named is exactly as it was, and it is the only
    // share the queue holds.
    let after = described(&batch, "the bystander share", &bystander).await;
    assert_eq!(
        after, before,
        "case the bystander share: a refused identifier must change nothing, got {after} after \
         {before}"
    );
    let listed = listed_arns(&batch).await;
    assert_eq!(
        listed,
        vec![bystander.clone()],
        "case the bystander share: a refused identifier must not touch the listing, got \
         {listed:?}"
    );
}

/// One share a delete is refused for: what the row varies, and how the share came to accept
/// submissions.
struct Row {
    label: &'static str,
    name: &'static str,
    /// Laid over the create body.
    extra: Value,
    /// Whether an update put the share back to accepting submissions after the create
    /// disabled it.
    enabled_by_update: bool,
}

/// The table of shares whose delete must be refused.
///
/// Rows differ in HOW the share came to accept submissions (the create asked for `ENABLED`,
/// the create named no state at all and took the documented default, and an update enabled a
/// share the create disabled), in how many capacity limits the share allocates and of which
/// units, in the sharing strategy and whether it carries a `borrowLimit`, in the preemption
/// state, and in whether the share carries tags.
///
/// The enum values are the sets the model declares (`smithy_query(query="shape",
/// shape="QuotaShareState,QuotaShareResourceSharingStrategy,
/// QuotaShareInSharePreemptionState")`: `ENABLED | DISABLED`, `RESERVE | LEND |
/// LEND_AND_BORROW`, `ENABLED | DISABLED`), and the capacity units are of the form
/// `QuotaShareCapacityLimit$capacityUnit` documents ("For example, `ml.m5.large`").
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "the create asked for ENABLED",
            name: "qs-enabled-at-create",
            extra: json!({"state": "ENABLED"}),
            enabled_by_update: false,
        },
        Row {
            label: "the create named no state at all",
            name: "qs-default-state",
            extra: json!({}),
            enabled_by_update: false,
        },
        Row {
            label: "an update enabled a share the create disabled",
            name: "qs_Enabled_Again-2",
            extra: json!({
                "state": "DISABLED",
                "tags": {"team": "research", "cost-centre": "42"},
            }),
            enabled_by_update: true,
        },
        Row {
            label: "ENABLED, lending and borrowing with three capacity limits",
            name: "q",
            extra: json!({
                "state": "ENABLED",
                "resourceSharingConfiguration": {"strategy": "LEND_AND_BORROW", "borrowLimit": 200},
                "preemptionConfiguration": {"inSharePreemption": "ENABLED"},
                "capacityLimits": [
                    {"maxCapacity": 0, "capacityUnit": "ml.m5.large"},
                    {"maxCapacity": 3, "capacityUnit": "ml.m5.2xlarge"},
                    {"maxCapacity": 9, "capacityUnit": "ml.p4d.24xlarge"},
                ],
            }),
            enabled_by_update: false,
        },
        Row {
            label: "ENABLED and lending only",
            name: "qs-lending",
            extra: json!({
                "state": "ENABLED",
                "resourceSharingConfiguration": {"strategy": "LEND"},
            }),
            enabled_by_update: false,
        },
    ]
}

/// RULE: a share whose state still accepts submissions is not removable — the delete is
/// refused 400 / `ClientException` with the precondition's own sentence, stated bare, and
/// writes nothing — while the same delete of a share that no longer accepts submissions is
/// accepted with the empty body the operation models, after which the dead identifier is
/// reported nonexistent by both the reader and the updater.
#[tokio::test]
async fn a_share_that_still_accepts_submissions_must_be_disabled_before_it_is_deleted() {
    let batch = Batch::new();
    queue(&batch).await;

    let mut dead: Vec<(&'static str, String)> = Vec::new();
    for row in rows() {
        let case = row.label;
        let reference = share(&batch, row.name, row.extra.clone()).await;
        if row.enabled_by_update {
            batch
                .ok(
                    UPDATE,
                    json!({"quotaShareArn": reference, "state": "ENABLED"}),
                )
                .await;
        }

        let before = described(&batch, case, &reference).await;
        assert_eq!(
            before["state"].as_str(),
            Some(state::STATE_ENABLED),
            "case {case}: the row must start out accepting submissions, got {before}"
        );

        let (status, refused) = batch
            .call(DELETE, json!({"quotaShareArn": reference}))
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled ClientException 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case {case}: expected the modelled ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert_eq!(
            message, MUST_BE_DISABLED,
            "case {case}: expected the precondition's own sentence, got {message:?}"
        );
        assert!(
            !message.starts_with(ENVELOPE_PREFIX) && !message.contains(ENVELOPE_REQUEST_ID),
            "case {case}: this refusal is stated bare, got {message:?}"
        );

        // The refusal wrote nothing: the share keeps its name and its state, no removal was
        // started, and the queue still lists it.
        let after = described(&batch, case, &reference).await;
        assert_eq!(
            after["quotaShareName"].as_str(),
            before["quotaShareName"].as_str(),
            "case {case}: a refused delete must leave the name, got {after} after {before}"
        );
        assert_eq!(
            after["state"].as_str(),
            Some(state::STATE_ENABLED),
            "case {case}: a refused delete must leave the state, got {after}"
        );
        assert_ne!(
            after["status"].as_str(),
            Some(state::STATUS_DELETING),
            "case {case}: a refused delete must not start a removal, got {after}"
        );
        assert!(
            listed_arns(&batch).await.contains(&reference),
            "case {case}: a share whose delete was refused is still listed"
        );

        // The complement: once submissions are disabled and that update settles, the SAME
        // call is accepted.
        batch
            .ok(
                UPDATE,
                json!({"quotaShareArn": reference, "state": "DISABLED"}),
            )
            .await;
        wait_for_the_clock().await;
        let settled = described(&batch, case, &reference).await;
        assert_eq!(
            (
                settled["state"].as_str(),
                settled["status"].as_str()
            ),
            (Some(state::STATE_DISABLED), Some(state::STATUS_VALID)),
            "case {case}: expected the disabling update to settle, got {settled}"
        );

        let (status, accepted) = batch
            .call(DELETE, json!({"quotaShareArn": reference}))
            .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the delete of a DISABLED share to be accepted, got {status} \
             {accepted}"
        );
        let empty = accepted.is_null()
            || accepted
                .as_object()
                .is_some_and(serde_json::Map::is_empty);
        assert!(
            empty,
            "case {case}: DeleteQuotaShare models no output member, got {accepted}"
        );
        dead.push((case, reference));
    }

    // The removal is owed by the clock; once it falls due the dead identifier names nothing,
    // for the reader and the updater alike.
    wait_for_the_clock().await;
    let listed = listed_arns(&batch).await;
    for (case, reference) in &dead {
        assert!(
            !listed.contains(reference),
            "case {case}: a removed share must leave the listing, got {listed:?}"
        );
        for taker in takers() {
            if taker.uri == DELETE {
                // The remover is idempotent in its identifier: the removal it asks for is
                // already done, so it is the same empty success.
                let (status, accepted) = batch
                    .call(DELETE, json!({"quotaShareArn": reference}))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case} on DeleteQuotaShare: deleting a removed share asks for a \
                     removal already done, got {status} {accepted}"
                );
                continue;
            }
            let (status, refused) = batch
                .call(taker.uri, (taker.body)(reference))
                .await;
            assert_eq!(
                status, 400,
                "case {case} on {}: expected the dead identifier to be refused, got {status} \
                 {refused}",
                taker.label
            );
            assert_eq!(
                error_code(&refused),
                "ClientException",
                "case {case} on {}: expected the modelled ClientException, got {refused}",
                taker.label
            );
            let message = error_message(&refused);
            assert_eq!(
                message,
                format!("{reference} does not exist."),
                "case {case} on {}: expected the nonexistence sentence naming the identifier \
                 the request submitted, got {message:?}",
                taker.label
            );
        }
    }
    assert!(
        listed.is_empty(),
        "case after every removal: the queue holds no share this suite deleted, got {listed:?}"
    );
}
