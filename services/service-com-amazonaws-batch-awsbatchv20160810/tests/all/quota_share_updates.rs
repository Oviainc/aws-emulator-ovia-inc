// SPDX-License-Identifier: CC0-1.0

//! `UpdateQuotaShare`, end to end over the wire.
//!
//! Every case drives a real restJson1 request through the router the emulator mounts, so
//! what is asserted is the response a `batch` client gets.
//!
//! The RULES under test, each asserted over a SET of inputs rather than one request:
//!
//! * an update is a PARTIAL write of the persisted share: every configuration member the
//!   request carries REPLACES the stored one, every member it omits leaves the stored value
//!   exactly as it was, and the share's identity (its name, its ARN, the job queue it hangs
//!   off and its tags — none of which the request can name) is untouched; the share's own
//!   reader answers the merged record;
//! * an accepted update answers the stored name and ARN, and nothing else;
//! * an accepted update leaves the share in the transient `UPDATING` status, with the move
//!   back to `VALID` owed by the CLOCK rather than by the call;
//! * applying one update twice leaves the same record (the write is idempotent in its
//!   inputs);
//! * a request this operation cannot serve is the modelled `ClientException` (HTTP 400) and
//!   changes NOTHING: a `quotaShareArn` that is no quota-share ARN at all and one that is
//!   well-formed but names nothing are DIFFERENT refusals, an empty `capacityLimits`
//!   allocates no capacity, and a `state` outside the closed set its enum declares is
//!   refused rather than stored.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    arn, assert_clock_owed_status, assert_reads_agree, bracketed, compute_environment, error_code,
    error_message, job_queue, Batch,
};

/// `UpdateQuotaShare`'s route (`smithy_query(query="operation",
/// operation="UpdateQuotaShare")`: POST /v1/updatequotashare).
const UPDATE: &str = "/v1/updatequotashare";

/// `CreateQuotaShare`'s route.
const CREATE: &str = "/v1/createquotashare";

/// `DescribeQuotaShare`'s route — the reader an update is proven through.
const DESCRIBE: &str = "/v1/describequotashare";

/// The queue the shares under test hang off.
const QUEUE: &str = "jq-updated-shares";

/// The members `UpdateQuotaShare` answers with (`smithy_query(query="operation",
/// operation="UpdateQuotaShare")`), so the response is checked to invent nothing beyond
/// them.
const OUTPUT_MEMBERS: &[&str] = &["quotaShareName", "quotaShareArn"];

/// The configuration members an `UpdateQuotaShare` request can carry, each optional, from
/// the same model query: these are the members an update replaces and `DescribeQuotaShare`
/// reports back.
const CONFIG_MEMBERS: &[&str] = &[
    "capacityLimits",
    "resourceSharingConfiguration",
    "preemptionConfiguration",
    "state",
];

/// How long to wait for a clock-owed transition to fall due: the span the clock owes it
/// ([`state::JOB_PHASE_MILLIS`]), with margin for a slow machine.
async fn wait_for_the_clock() {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    tokio::time::sleep(Duration::from_millis(span * 3)).await;
}

/// The create body every share under test starts from: one capacity limit, the `RESERVE`
/// strategy, preemption off, `ENABLED`, and tags (which no update can name).
fn create_body(name: &str) -> Value {
    json!({
        "quotaShareName": name,
        "jobQueue": QUEUE,
        "capacityLimits": [{"maxCapacity": 4, "capacityUnit": "ml.m5.large"}],
        "resourceSharingConfiguration": {"strategy": "RESERVE"},
        "preemptionConfiguration": {"inSharePreemption": "DISABLED"},
        "state": "ENABLED",
        "tags": {"team": "research"},
    })
}

/// The ARN a share of `name` on [`QUEUE`] is stored under.
fn share_arn(name: &str) -> String {
    arn(&format!("job-queue/{QUEUE}/quota-share/{name}"))
}

/// Create the queue the shares hang off, over a compute environment of its own.
async fn queue(batch: &Batch) {
    let environment = compute_environment(batch, &format!("ce-for-{QUEUE}")).await;
    job_queue(batch, QUEUE, &environment).await;
}

/// Create a share called `name` from [`create_body`] and answer its ARN.
async fn share(batch: &Batch, name: &str) -> String {
    let created = batch.ok(CREATE, create_body(name)).await;
    created["quotaShareArn"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// `DescribeQuotaShare` over the wire, requiring the 200.
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

/// One update this suite applies: the patch it sends, and what the row varies.
struct Update {
    label: &'static str,
    patch: Value,
}

/// The table of updates. Rows differ in WHICH members the request carries (one at a time,
/// all of them, and none of them), in how many capacity limits the share is left with and
/// of which units and quantities, in the sharing strategy and whether it carries a
/// `borrowLimit`, in the preemption state and in the requested `state` — including a value
/// the share already carries.
///
/// The enum values are the sets the model declares
/// (`smithy_query(query="shape", shape="QuotaShareState,QuotaShareResourceSharingStrategy,
/// QuotaShareInSharePreemptionState")`: `ENABLED | DISABLED`, `RESERVE | LEND |
/// LEND_AND_BORROW`, `ENABLED | DISABLED`), and the capacity units are of the form
/// `QuotaShareCapacityLimit$capacityUnit` documents ("For example, `ml.m5.large`").
fn updates() -> Vec<Update> {
    vec![
        Update {
            label: "capacityLimits alone: one limit, another unit and quantity",
            patch: json!({"capacityLimits": [{"maxCapacity": 11, "capacityUnit": "ml.c5.xlarge"}]}),
        },
        Update {
            label: "capacityLimits alone: three limits, one of them zero",
            patch: json!({"capacityLimits": [
                {"maxCapacity": 0, "capacityUnit": "ml.m5.large"},
                {"maxCapacity": 3, "capacityUnit": "ml.m5.2xlarge"},
                {"maxCapacity": 9, "capacityUnit": "ml.p4d.24xlarge"},
            ]}),
        },
        Update {
            label: "state alone: DISABLED",
            patch: json!({"state": "DISABLED"}),
        },
        Update {
            label: "state alone: ENABLED, the value the share already carries",
            patch: json!({"state": "ENABLED"}),
        },
        Update {
            label: "resourceSharingConfiguration alone: LEND",
            patch: json!({"resourceSharingConfiguration": {"strategy": "LEND"}}),
        },
        Update {
            label: "resourceSharingConfiguration alone: LEND_AND_BORROW with a borrowLimit",
            patch: json!({"resourceSharingConfiguration": {
                "strategy": "LEND_AND_BORROW",
                "borrowLimit": 200,
            }}),
        },
        Update {
            label: "resourceSharingConfiguration alone: LEND_AND_BORROW borrowing without limit",
            patch: json!({"resourceSharingConfiguration": {
                "strategy": "LEND_AND_BORROW",
                "borrowLimit": -1,
            }}),
        },
        Update {
            label: "preemptionConfiguration alone: ENABLED",
            patch: json!({"preemptionConfiguration": {"inSharePreemption": "ENABLED"}}),
        },
        Update {
            label: "every configuration member at once",
            patch: json!({
                "capacityLimits": [{"maxCapacity": 1, "capacityUnit": "ml.g5.8xlarge"}],
                "resourceSharingConfiguration": {"strategy": "LEND_AND_BORROW", "borrowLimit": 50},
                "preemptionConfiguration": {"inSharePreemption": "ENABLED"},
                "state": "DISABLED",
            }),
        },
        Update {
            label: "no configuration member at all",
            patch: json!({}),
        },
    ]
}

/// The body of an `UpdateQuotaShare` for `reference` carrying `patch`.
fn update_body(reference: &str, patch: &Value) -> Value {
    let mut body = json!({"quotaShareArn": reference});
    for (member, value) in patch.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    body
}

/// RULE: every configuration member an update carries replaces the stored one, every member
/// it omits keeps its stored value, and the share's identity is untouched — read back
/// through the share's own reader.
#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    let batch = Batch::new();
    queue(&batch).await;

    for (index, update) in updates().into_iter().enumerate() {
        let case = update.label;
        // Each row updates a share of its OWN, created from the same base body, so what a
        // row asserts is its patch and not the previous row's.
        let name = format!("qs-partial-{index:02}");
        let reference = share(&batch, &name).await;
        let before = described(&batch, case, &reference).await;

        let answered = batch
            .ok(UPDATE, update_body(&reference, &update.patch))
            .await;
        assert_eq!(
            answered["quotaShareName"].as_str(),
            Some(name.as_str()),
            "case {case}: the update answers the stored name, got {answered}"
        );
        assert_eq!(
            answered["quotaShareArn"].as_str(),
            Some(reference.as_str()),
            "case {case}: the update answers the stored ARN, got {answered}"
        );
        for member in answered
            .as_object()
            .unwrap_or_else(|| panic!("case {case}: expected a response object, got {answered}"))
            .keys()
        {
            assert!(
                OUTPUT_MEMBERS.contains(&member.as_str()),
                "case {case}: {member} is not a member UpdateQuotaShare answers with, got \
                 {answered}"
            );
        }

        let after = described(&batch, case, &reference).await;
        for member in CONFIG_MEMBERS {
            let expected = update
                .patch
                .get(member)
                .or_else(|| before.get(member))
                .cloned()
                .unwrap_or(Value::Null);
            let actual = after.get(member).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expected,
                "case {case}: expected {member} {expected}, got {actual} (whole record {after})"
            );
        }
        // The identity of the share is not updatable: no member of the request can name it.
        for member in ["quotaShareName", "quotaShareArn", "jobQueueArn", "tags"] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case}: {member} must survive an update, got {after} after {before}"
            );
        }
    }
}

/// RULE: an accepted update leaves the share in the transient `UPDATING` status and the move
/// back to `VALID` is owed by the clock, not by the call that reads it.
#[tokio::test]
async fn an_accepted_update_leaves_the_share_in_the_transient_status() {
    let batch = Batch::new();
    queue(&batch).await;

    let mut references: Vec<(String, String)> = Vec::new();
    for (index, update) in updates().into_iter().enumerate() {
        let case = update.label.to_string();
        let name = format!("qs-status-{index:02}");
        let reference = share(&batch, &name).await;
        // The share settles first, so the `UPDATING` below is the update's own write and
        // not the `CREATING` the create left behind.
        wait_for_the_clock().await;
        let settled = described(&batch, &case, &reference).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(state::STATUS_VALID),
            "case {case}: a share whose create the clock has settled is VALID, got {settled}"
        );

        let accepted = bracketed(batch.ok(UPDATE, update_body(&reference, &update.patch))).await;
        // The span is never zero, so the transient status is observable — and the bracket
        // of the update that started it is what makes this read's answer certain.
        let fresh = bracketed(described(&batch, &case, &reference)).await;
        assert_clock_owed_status(
            &case,
            &fresh,
            accepted.due_window(),
            state::STATUS_UPDATING,
            Some(state::STATUS_VALID),
        );
        // A read does not advance it: two reads before the clock owes anything answer alike.
        let again = bracketed(described(&batch, &case, &reference)).await;
        assert_reads_agree(&case, &fresh, &again, accepted.due_window());
        references.push((case, reference));
    }

    wait_for_the_clock().await;
    for (case, reference) in &references {
        let settled = described(&batch, case, reference).await;
        assert_eq!(
            settled["status"].as_str(),
            Some(state::STATUS_VALID),
            "case {case}: the clock owes the move back to the settled status, got {settled}"
        );
    }
}

/// RULE: applying one update twice leaves the same record.
#[tokio::test]
async fn applying_one_update_twice_leaves_the_same_record() {
    let batch = Batch::new();
    queue(&batch).await;

    for (index, update) in updates().into_iter().enumerate() {
        let case = update.label;
        let name = format!("qs-idempotent-{index:02}");
        let reference = share(&batch, &name).await;

        batch
            .ok(UPDATE, update_body(&reference, &update.patch))
            .await;
        wait_for_the_clock().await;
        let once = described(&batch, case, &reference).await;

        batch
            .ok(UPDATE, update_body(&reference, &update.patch))
            .await;
        wait_for_the_clock().await;
        let twice = described(&batch, case, &reference).await;

        assert_eq!(
            twice, once,
            "case {case}: the same update applied twice must leave the same record, got {twice} \
             after {once}"
        );
    }
}

/// RULE: a request `UpdateQuotaShare` cannot serve is the modelled `ClientException` (HTTP
/// 400) and changes nothing; the two unusable-identifier refusals are DIFFERENT.
#[tokio::test]
async fn an_update_refuses_what_it_cannot_serve_and_changes_nothing() {
    let batch = Batch::new();
    queue(&batch).await;
    let reference = share(&batch, "qs-refusals").await;
    // Let the create settle, so the record compared across each refusal is stable.
    wait_for_the_clock().await;
    let before = described(&batch, "before any refusal", &reference).await;

    // A value that is no quota-share ARN at all: rows differ in HOW it fails the pattern —
    // not an ARN, empty, an ARN of another resource, a queue ARN with no share path, a share
    // path with no name.
    for malformed in [
        "not-an-arn".to_string(),
        String::new(),
        arn(&format!("job-queue/{QUEUE}")),
        arn("quota-share/qs-refusals"),
        arn(&format!("job-queue/{QUEUE}/quota-share/")),
        arn("compute-environment/ce-for-updated-shares"),
    ] {
        let (status, refused) = batch
            .call(UPDATE, update_body(&malformed, &json!({"state": "DISABLED"})))
            .await;
        assert_eq!(
            status, 400,
            "case update by {malformed:?}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case update by {malformed:?}: expected the modelled ClientException, got {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains(state::QUOTA_SHARE_ARN_NOT_VALID),
            "case update by {malformed:?}: expected the ARN-pattern refusal, got {message:?}"
        );
    }

    // A well-formed ARN naming nothing stored is the OTHER refusal.
    for unknown in [
        share_arn("qs-absent"),
        arn("job-queue/no-such-queue/quota-share/qs-refusals"),
    ] {
        let (status, refused) = batch
            .call(UPDATE, update_body(&unknown, &json!({"state": "DISABLED"})))
            .await;
        assert_eq!(
            status, 400,
            "case update the unknown share {unknown:?}: expected 400, got {status} {refused}"
        );
        let message = error_message(&refused);
        assert!(
            message.contains("does not exist"),
            "case update the unknown share {unknown:?}: expected the nonexistence refusal, got \
             {message:?}"
        );
        assert!(
            !message.contains(state::QUOTA_SHARE_ARN_NOT_VALID),
            "case update the unknown share {unknown:?}: a well-formed ARN must not be reported \
             as malformed, got {message:?}"
        );
    }

    // A patch the service cannot store: no capacity at all, and a `state` outside the closed
    // set its enum declares (lower case, and a value no `QuotaShareState` names).
    let unusable: Vec<(&str, Value)> = vec![
        ("an empty capacityLimits", json!({"capacityLimits": []})),
        ("a state in the wrong case", json!({"state": "disabled"})),
        ("a state no enum value names", json!({"state": "PAUSED"})),
    ];
    for (case, patch) in unusable {
        let (status, refused) = batch.call(UPDATE, update_body(&reference, &patch)).await;
        assert_eq!(
            status, 400,
            "case update with {case}: expected 400, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "ClientException",
            "case update with {case}: expected the modelled ClientException, got {refused}"
        );
    }

    // Nothing any refusal above named was written: the share is exactly as it was.
    let after = described(&batch, "after every refusal", &reference).await;
    assert_eq!(
        after, before,
        "case after every refusal: a refused update must leave the share untouched, got {after} \
         after {before}"
    );
}
