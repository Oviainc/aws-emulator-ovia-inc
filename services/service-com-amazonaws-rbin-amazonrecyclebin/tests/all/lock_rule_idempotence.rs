// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the requests `LockRule` COMPLETES against a rule that is
//! already locked, and for the body every quota refusal of this service answers with.
//!
//! The rules asserted here:
//!
//! 1. A lock request that names the unlock delay already in force on a locked rule asks
//!    for the state the rule is already in, so the service completes it: HTTP 200 with
//!    the stored rule, lock state `locked` and the lock configuration in force, changing
//!    nothing — whether the repeat arrives as a sequential retry or as one of several
//!    requests in flight at once.
//! 2. Only a lock request that would CHANGE the lock in force on a locked rule — another
//!    unlock delay — is refused, with the modelled `ConflictException` (HTTP 409), its
//!    `Reason` and a message naming the delay mismatch; the rule keeps the delay in
//!    force.
//! 3. A request past a bound this service holds a retention rule to is refused with the
//!    modelled `ServiceQuotaExceededException` (HTTP 402) whose `Message` and `Reason`
//!    are both the reason token the model declares for it, with nothing of the request
//!    rendered into the body, and persists nothing.
//!
//! Each table spans what the rules generalise over — the resource type, the unlock delay
//! across its published bounds, how the lock was taken (at create or by `LockRule`), how
//! many repeats arrive and whether they are sequential or concurrent, and for the quota
//! bodies the door the request arrives through and the form and size of the tags — and
//! each row drives the service through the real router (`MultiServiceRouter::dispatch`),
//! exactly as a signed SDK request would.

use serde_json::{json, Value};

use crate::error_response_bodies::{error_code, message};
use crate::resource_tags::{list_tags, tag_resource};
use crate::retention_rule_lifecycle::{
    create_rule, get_rule, list_of, list_rules, lock_configuration, retention, router,
    supported_retention_max, RESOURCE_TYPES, UNLOCK_DELAY_MAX, UNLOCK_DELAY_MIN,
};
use crate::rule_lock_transitions::{lock_end_seconds, lock_rule};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    MAX_RULES_PER_RESOURCE_TAG, MAX_TAGS_PER_RULE, SERVICE_QUOTA_REASON_EXCEEDED,
};
use smithy_server_runtime::MultiServiceRouter;

/// How many lock requests for one rule are kept in flight at once. More than one, so a
/// retrying client and a reconciler with several workers are both covered; the exact
/// number is not what the rule turns on.
const RACERS: usize = 6;

/// An unlock delay between the published bounds, so the table is not only its edges.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — `UnlockDelayValue` range
/// {min: 7, max: 30}.
const UNLOCK_DELAY_MIDDLE: i64 = 19;

/// How the lock under test was taken: with the create, or by `LockRule` on a rule
/// created without one. Both leave a `locked` rule, so both must answer a repeat of the
/// lock request the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockDoor {
    AtCreate,
    ByLockRule,
}

/// One row of the idempotent-relock table.
#[derive(Debug, Clone, Copy)]
struct RelockRow {
    resource_type: &'static str,
    /// The unlock delay the rule is locked with, and the one every repeat names.
    unlock_delay: i64,
    door: LockDoor,
    /// How many times the identical lock request is repeated after the lock is in force.
    repeats: usize,
}

impl RelockRow {
    fn case(&self) -> String {
        format!(
            "{} locked at an unlock delay of {} days {}, {} identical repeat(s)",
            self.resource_type,
            self.unlock_delay,
            match self.door {
                LockDoor::AtCreate => "by CreateRule",
                LockDoor::ByLockRule => "by LockRule",
            },
            self.repeats
        )
    }
}

/// The rows the idempotence rule has to hold for: every resource type, each published
/// unlock-delay bound and one value between them, both doors a lock can be taken
/// through, and one, two and several repeats.
fn relock_rows() -> Vec<RelockRow> {
    let mut rows = Vec::new();
    for (nth, resource_type) in RESOURCE_TYPES.iter().enumerate() {
        for unlock_delay in [UNLOCK_DELAY_MIN, UNLOCK_DELAY_MIDDLE, UNLOCK_DELAY_MAX] {
            for door in [LockDoor::AtCreate, LockDoor::ByLockRule] {
                rows.push(RelockRow {
                    resource_type,
                    unlock_delay,
                    door,
                    repeats: 1 + (nth % 3),
                });
            }
        }
    }
    rows
}

/// Create a rule of `resource_type`, optionally carrying `lock`, and answer its
/// identifier.
async fn created(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    lock: Option<Value>,
) -> String {
    let mut body = json!({
        "ResourceType": resource_type,
        "RetentionPeriod": retention(supported_retention_max(resource_type)),
    });
    if let Some(configuration) = lock {
        body["LockConfiguration"] = configuration;
    }
    let (status, created) = create_rule(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: fixture CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no Identifier: {created:?}"))
        .to_string()
}

/// A rule of `resource_type` that is `locked` under `unlock_delay`, locked through
/// `door`. Answers its identifier.
async fn locked_rule(
    router: &MultiServiceRouter,
    case: &str,
    resource_type: &str,
    unlock_delay: i64,
    door: LockDoor,
) -> String {
    let configuration = lock_configuration(unlock_delay);
    match door {
        LockDoor::AtCreate => created(router, case, resource_type, Some(configuration)).await,
        LockDoor::ByLockRule => {
            let identifier = created(router, case, resource_type, None).await;
            let (status, locked) = lock_rule(
                router,
                &identifier,
                json!({ "LockConfiguration": configuration }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: fixture LockRule expected HTTP 200, got {status} / {locked:?}"
            );
            identifier
        }
    }
}

/// Assert `body` is the stored locked rule: lock state `locked`, the lock configuration
/// in force, and no lock end time (a locked rule is inside no unlock delay period).
fn assert_locked(case: &str, body: &Value, identifier: &str, unlock_delay: i64) {
    assert_eq!(
        body["Identifier"].as_str(),
        Some(identifier),
        "case {case:?}: expected Identifier {identifier:?}, got {:?}",
        body["Identifier"]
    );
    assert_eq!(
        body["LockState"].as_str(),
        Some("locked"),
        "case {case:?}: expected LockState=locked, got {:?}",
        body["LockState"]
    );
    assert_eq!(
        body["LockConfiguration"],
        lock_configuration(unlock_delay),
        "case {case:?}: expected the unlock delay in force ({unlock_delay} days), got {:?}",
        body["LockConfiguration"]
    );
    assert_eq!(
        lock_end_seconds(body),
        None,
        "case {case:?}: a locked rule is inside no unlock delay period, got {:?}",
        body["LockEndTime"]
    );
}

#[tokio::test]
async fn a_lock_request_naming_the_unlock_delay_in_force_is_completed_unchanged() {
    let router = router();
    for row in relock_rows() {
        let case = row.case();
        let identifier = locked_rule(
            &router,
            &case,
            row.resource_type,
            row.unlock_delay,
            row.door,
        )
        .await;
        let (_, before) = get_rule(&router, &identifier).await;

        for repeat in 1..=row.repeats {
            let case = format!("{case}: repeat {repeat}");
            let (status, relocked) = lock_rule(
                &router,
                &identifier,
                json!({ "LockConfiguration": lock_configuration(row.unlock_delay) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: a lock request for the lock in force expected HTTP 200, \
                 got {status} / {relocked:?}"
            );
            assert_locked(&case, &relocked, &identifier, row.unlock_delay);
            assert_eq!(
                relocked["ResourceType"].as_str(),
                Some(row.resource_type),
                "case {case:?}: expected ResourceType {:?}, got {:?}",
                row.resource_type,
                relocked["ResourceType"]
            );
        }

        // The repeats changed nothing a reader can see: the stored rule is the rule the
        // first lock left behind.
        let case = format!("{case}: the stored rule after every repeat");
        let (status, after) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {after:?}"
        );
        assert_locked(&case, &after, &identifier, row.unlock_delay);
        assert_eq!(
            after["LockConfiguration"], before["LockConfiguration"],
            "case {case:?}: expected the stored LockConfiguration {:?}, got {:?}",
            before["LockConfiguration"], after["LockConfiguration"]
        );
        assert_eq!(
            after["RetentionPeriod"], before["RetentionPeriod"],
            "case {case:?}: expected the stored RetentionPeriod {:?}, got {:?}",
            before["RetentionPeriod"], after["RetentionPeriod"]
        );
    }
}

#[tokio::test]
async fn identical_lock_requests_in_flight_at_once_all_succeed() {
    for unlock_delay in [UNLOCK_DELAY_MIN, UNLOCK_DELAY_MIDDLE, UNLOCK_DELAY_MAX] {
        let router = router();
        let case = format!("{RACERS} lock requests at once, unlock delay {unlock_delay} days");
        let identifier = created(&router, &case, RESOURCE_TYPES[0], None).await;
        let body = json!({ "LockConfiguration": lock_configuration(unlock_delay) });

        // Every racer is in flight before any of them is awaited to completion, so the
        // winner of the first lock does not decide the answer the others get.
        let answers = tokio::join!(
            lock_rule(&router, &identifier, body.clone()),
            lock_rule(&router, &identifier, body.clone()),
            lock_rule(&router, &identifier, body.clone()),
            lock_rule(&router, &identifier, body.clone()),
            lock_rule(&router, &identifier, body.clone()),
            lock_rule(&router, &identifier, body.clone()),
        );
        let answers = [
            answers.0, answers.1, answers.2, answers.3, answers.4, answers.5,
        ];
        assert_eq!(
            answers.len(),
            RACERS,
            "case {case:?}: expected {RACERS} answers, got {}",
            answers.len()
        );
        for (racer, (status, body)) in answers.iter().enumerate() {
            let case = format!("{case}: racer {racer}");
            assert_eq!(
                *status, 200,
                "case {case:?}: expected HTTP 200, got {status} / {body:?}"
            );
            assert_locked(&case, body, &identifier, unlock_delay);
        }

        let case = format!("{case}: the stored rule afterwards");
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_locked(&case, &fetched, &identifier, unlock_delay);
    }
}

#[tokio::test]
async fn a_lock_request_naming_another_unlock_delay_is_refused_and_changes_nothing() {
    let router = router();
    // Each row is a delay in force and a delay the refused request asks for: up, down,
    // and one day either side of the delay in force, across the published bounds.
    let rows: [(i64, i64); 6] = [
        (UNLOCK_DELAY_MIN, UNLOCK_DELAY_MAX),
        (UNLOCK_DELAY_MAX, UNLOCK_DELAY_MIN),
        (UNLOCK_DELAY_MIDDLE, UNLOCK_DELAY_MIDDLE + 1),
        (UNLOCK_DELAY_MIDDLE, UNLOCK_DELAY_MIDDLE - 1),
        (UNLOCK_DELAY_MIN, UNLOCK_DELAY_MIN + 1),
        (UNLOCK_DELAY_MAX, UNLOCK_DELAY_MAX - 1),
    ];
    for (nth, (in_force, requested)) in rows.into_iter().enumerate() {
        let case =
            format!("locked at {in_force} days, a lock request asking for {requested} days");
        let resource_type = RESOURCE_TYPES[nth % RESOURCE_TYPES.len()];
        let door = if nth % 2 == 0 {
            LockDoor::AtCreate
        } else {
            LockDoor::ByLockRule
        };
        let identifier = locked_rule(&router, &case, resource_type, in_force, door).await;

        let (status, refused) = lock_rule(
            &router,
            &identifier,
            json!({ "LockConfiguration": lock_configuration(requested) }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected HTTP 409, got {status} / {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ConflictException",
            "case {case:?}: expected the modelled ConflictException, got {refused:?}"
        );
        assert_eq!(
            refused["Reason"].as_str(),
            Some("INVALID_RULE_STATE"),
            "case {case:?}: expected Reason INVALID_RULE_STATE, got {:?}",
            refused["Reason"]
        );
        let expected = format!(
            "Cannot lock Recycle Bin rule {identifier} because it has a different unlock \
             delay already set."
        );
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected Message {expected:?}, got {:?}",
            message(&refused)
        );

        // The refusal left the lock in force exactly as it was.
        let case = format!("{case}: the stored rule after the refusal");
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_locked(&case, &fetched, &identifier, in_force);
    }
}

/// `(key, value)` pairs as the tag helpers take them.
type TagPairs = Vec<(String, String)>;

/// `n` tags, `(key0, value0) … (key{n-1}, value{n-1})`, in `form`.
fn tags(n: usize, form: fn(usize) -> (String, String)) -> TagPairs {
    (0..n).map(form).collect()
}

/// Plain ASCII tags.
fn plain(nth: usize) -> (String, String) {
    (format!("key{nth}"), format!("value{nth}"))
}

/// Tags whose keys and values are not ASCII, so the bound is not read off byte lengths.
fn non_ascii(nth: usize) -> (String, String) {
    (format!("clé{nth}"), format!("naïve 日本 {nth}"))
}

/// Assert `body` is this service's quota refusal: HTTP 402, the modelled error code, and
/// the reason token as the `Message` as well as the `Reason` — the body states the fault
/// class and nothing of the request.
fn assert_quota_refusal(case: &str, status: u16, body: &Value) {
    assert_eq!(
        status, 402,
        "case {case:?}: expected HTTP 402, got {status} / {body:?}"
    );
    assert_eq!(
        error_code(body),
        "ServiceQuotaExceededException",
        "case {case:?}: expected the modelled ServiceQuotaExceededException, got {body:?}"
    );
    assert_eq!(
        message(body),
        SERVICE_QUOTA_REASON_EXCEEDED,
        "case {case:?}: expected Message {SERVICE_QUOTA_REASON_EXCEEDED:?}, got {:?}",
        message(body)
    );
    assert_eq!(
        body["Reason"].as_str(),
        Some(SERVICE_QUOTA_REASON_EXCEEDED),
        "case {case:?}: expected Reason {SERVICE_QUOTA_REASON_EXCEEDED:?}, got {:?}",
        body["Reason"]
    );
}

#[tokio::test]
async fn every_quota_refusal_states_the_reason_token_alone() {
    let router = router();
    // The tags-per-resource bound, reached through the create door at several sizes past
    // it and in two forms of tag.
    let over_quota: [(&str, TagPairs); 4] = [
        (
            "one tag past the tags-per-resource quota",
            tags(MAX_TAGS_PER_RULE + 1, plain),
        ),
        (
            "ten tags past the tags-per-resource quota",
            tags(MAX_TAGS_PER_RULE + 10, plain),
        ),
        (
            "twice the tags-per-resource quota",
            tags(2 * MAX_TAGS_PER_RULE, plain),
        ),
        (
            "non-ASCII tags past the tags-per-resource quota",
            tags(MAX_TAGS_PER_RULE + 1, non_ascii),
        ),
    ];
    for (case, over) in over_quota {
        let (status, refused) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPES[0],
                "RetentionPeriod": retention(7),
                "Description": case,
                "Tags": over.iter().map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect::<Vec<Value>>(),
            }),
        )
        .await;
        assert_quota_refusal(case, status, &refused);
        // A refused create persists nothing: no rule of the Region carries the
        // description the refused request named.
        let (status, listed) =
            list_rules(&router, json!({ "ResourceType": RESOURCE_TYPES[0] })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListRules expected HTTP 200, got {status} / {listed:?}"
        );
        assert!(
            list_of(&listed, "Rules")
                .iter()
                .all(|summary| summary["Description"].as_str() != Some(case)),
            "case {case:?}: a refused create must mint no rule, got {listed:?}"
        );
    }

    // Exactly the quota is accepted, and the rule holds every one of those tags — the
    // bound refuses the request past it, not the request at it.
    let case = "exactly the tags-per-resource quota";
    let at_quota = tags(MAX_TAGS_PER_RULE, plain);
    let (status, created) = create_rule(
        &router,
        json!({
            "ResourceType": RESOURCE_TYPES[1],
            "RetentionPeriod": retention(7),
            "Tags": at_quota.iter().map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected HTTP 201, got {status} / {created:?}"
    );
    let rule_arn = created["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule reported no RuleArn: {created:?}"))
        .to_string();
    let (status, listed) = list_tags(&router, &rule_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected HTTP 200, got {status} / {listed:?}"
    );
    assert_eq!(
        list_of(&listed, "Tags").len(),
        MAX_TAGS_PER_RULE,
        "case {case:?}: expected the rule to hold {MAX_TAGS_PER_RULE} tags, got {:?}",
        listed["Tags"]
    );

    // The SAME bound reached through the assignment door answers the same body, and the
    // rule keeps the tags it held.
    let case = "one tag past the tags-per-resource quota, by TagResource";
    let (status, refused) =
        tag_resource(&router, &rule_arn, &[("one-too-many".into(), "x".into())]).await;
    assert_quota_refusal(case, status, &refused);
    let (status, listed) = list_tags(&router, &rule_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected HTTP 200, got {status} / {listed:?}"
    );
    assert_eq!(
        list_of(&listed, "Tags").len(),
        MAX_TAGS_PER_RULE,
        "case {case:?}: a refused assignment must store nothing, got {:?}",
        listed["Tags"]
    );

    // A DIFFERENT bound — the rules one resource tag key and value pair may select —
    // answers the very same body, so a quota refusal never carries the bound it reached.
    let case = "one rule past the rules-per-resource-tag-pair quota";
    let pair = json!([{ "ResourceTagKey": "project", "ResourceTagValue": "atlas" }]);
    for nth in 0..MAX_RULES_PER_RESOURCE_TAG {
        let (status, body) = create_rule(
            &router,
            json!({
                "ResourceType": RESOURCE_TYPES[2],
                "RetentionPeriod": retention(7),
                "ResourceTags": pair,
            }),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: holder {nth} of the pair expected HTTP 201, got {status} / {body:?}"
        );
    }
    let (status, refused) = create_rule(
        &router,
        json!({
            "ResourceType": RESOURCE_TYPES[2],
            "RetentionPeriod": retention(7),
            "ResourceTags": pair,
        }),
    )
    .await;
    assert_quota_refusal(case, status, &refused);
}
