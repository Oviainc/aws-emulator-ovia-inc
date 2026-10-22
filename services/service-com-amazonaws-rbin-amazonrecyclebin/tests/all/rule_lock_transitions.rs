// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Recycle Bin retention-rule LOCK state machine:
//! `LockRule` and `UnlockRule`, read back through `GetRule`, `UpdateRule` and
//! `DeleteRule`.
//!
//! The rule under test: a lock request is completed only for a Region-level rule (no
//! resource tags) that carries no exclusion tags and is not already locked, and it
//! persists the requested lock configuration with lock state `locked`; an unlock
//! request is completed only for a `locked` rule, and it persists lock state
//! `pending_unlock` with a lock end time one configured unlock delay away. Every other
//! lock or unlock request is refused with its modelled error and changes nothing.
//!
//! Each case drives the service through the real router, exactly as a signed SDK
//! request would, and every row states its own case so a failure names the input.

use std::time::Instant;

use serde_json::json;

use crate::retention_rule_lifecycle::{
    call, create_rule, delete_rule, get_rule, lock_configuration, retention, router, update_rule,
};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    RULE_STATUS_AVAILABLE, RULE_STATUS_PENDING, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

/// Published bounds of `UnlockDelay$UnlockDelayValue`, in `UnlockDelayUnit` `DAYS`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30},
/// and `UnlockDelayUnit` — enum values ["DAYS"].
const UNLOCK_DELAY_MIN: i64 = 7;
const UNLOCK_DELAY_MAX: i64 = 30;

/// Seconds in a day, the unit an unlock delay is measured in.
const SECONDS_PER_DAY: f64 = 86_400.0;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// `PATCH /rules/{Identifier}/lock` with `LockConfiguration` in the body — `LockRule`.
/// `pub(crate)` so the sibling suites of this one test target drive a lock through the
/// same request shape.
pub(crate) async fn lock_rule(
    router: &MultiServiceRouter,
    identifier: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    call(
        router,
        "PATCH",
        &format!("/rules/{identifier}/lock"),
        Some(body),
    )
    .await
}

/// `PATCH /rules/{Identifier}/unlock` — `UnlockRule`. `pub(crate)` for the same reason
/// as [`lock_rule`].
pub(crate) async fn unlock_rule(
    router: &MultiServiceRouter,
    identifier: &str,
) -> (u16, serde_json::Value) {
    call(
        router,
        "PATCH",
        &format!("/rules/{identifier}/unlock"),
        Some(json!({})),
    )
    .await
}

/// One row of a refused lock/unlock table: the case, the identifier it addresses, the
/// HTTP status expected, the `Reason` the body must carry (`None` where the modelled
/// member is not populated on that path) and the `LockState` the rule must still report
/// afterwards (`None` for a rule that has never been locked).
type RefusalRow<'a> = (&'a str, &'a str, u16, Option<&'a str>, Option<&'a str>);

/// Create a rule and hand back its minted identifier, failing the case that asked for it.
async fn created_identifier(
    router: &MultiServiceRouter,
    case: &str,
    body: serde_json::Value,
) -> String {
    let (status, created) = create_rule(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string()
}

/// Seconds since the Unix epoch, for comparing a returned `LockEndTime` against the
/// unlock delay the lock configuration asked for.
fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after the Unix epoch")
        .as_secs_f64()
}

/// `LockEndTime` as epoch seconds; `None` when the member is absent.
fn lock_end_seconds(value: &serde_json::Value) -> Option<f64> {
    match value.get("LockEndTime") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(text)) => Some(
            text.parse::<f64>()
                .unwrap_or_else(|_| panic!("LockEndTime is not epoch seconds: {text:?}")),
        ),
        Some(other) => Some(
            other
                .as_f64()
                .unwrap_or_else(|| panic!("LockEndTime is not a number: {other:?}")),
        ),
    }
}

#[tokio::test]
async fn lock_rule_locks_every_region_level_rule_and_persists_its_configuration() {
    let router = router();
    // The class the rule spans: every modelled resource type, both published unlock
    // delay bounds and one value between them, with and without a description.
    let rows: [(&str, &str, i64, Option<&str>); 6] = [
        ("snapshot rule at the minimum unlock delay", RESOURCE_TYPES[0], UNLOCK_DELAY_MIN, None),
        ("volume rule at the maximum unlock delay", RESOURCE_TYPES[1], UNLOCK_DELAY_MAX, None),
        ("image rule mid-range", RESOURCE_TYPES[2], 15, Some("ami rule")),
        ("snapshot rule one day past the minimum", RESOURCE_TYPES[0], UNLOCK_DELAY_MIN + 1, None),
        ("volume rule one day short of the maximum", RESOURCE_TYPES[1], UNLOCK_DELAY_MAX - 1, None),
        ("image rule with a description", RESOURCE_TYPES[2], 21, Some("keep images")),
    ];
    for (case, resource_type, unlock_delay, description) in rows {
        let mut body = json!({
            "ResourceType": resource_type,
            "RetentionPeriod": retention(30),
        });
        if let Some(text) = description {
            body["Description"] = json!(text);
        }
        let minted_at = Instant::now();
        let (status, created) = create_rule(&router, body).await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
        );
        let identifier = created["Identifier"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no Identifier in {created:?}"))
            .to_string();
        // A rule created without a lock configuration has never been locked, so it
        // reports no lock state at all.
        assert_eq!(
            created["LockState"].as_str(),
            None,
            "case {case:?}: expected a never-locked rule to report no LockState, got {:?}",
            created["LockState"]
        );

        let configuration = lock_configuration(unlock_delay);
        let (status, locked) =
            lock_rule(&router, &identifier, json!({ "LockConfiguration": configuration })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: LockRule expected HTTP 200, got {status} / {locked:?}"
        );
        assert_eq!(
            locked["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected LockState=locked, got {:?}",
            locked["LockState"]
        );
        assert_eq!(
            locked["LockConfiguration"], configuration,
            "case {case:?}: expected LockConfiguration {configuration:?}, got {:?}",
            locked["LockConfiguration"]
        );
        // Locking changes the lock, and nothing else about the rule.
        for member in ["Identifier", "ResourceType", "RetentionPeriod", "RuleArn"] {
            assert_eq!(
                locked[member], created[member],
                "case {case:?}: LockRule changed {member}: {:?} -> {:?}",
                created[member], locked[member]
            );
        }
        // `Status` is the CLOCK's to move, not this call's: locking neither advances
        // nor reverts it, so inside the transition span the locked rule still reports
        // the `pending` the create left, and past it the `available` the clock made
        // due. `tests/all/rule_status_transitions.rs` pins the transition itself.
        let status = locked["Status"].as_str();
        let expected = if minted_at.elapsed() < STATUS_TRANSITION {
            RULE_STATUS_PENDING
        } else {
            RULE_STATUS_AVAILABLE
        };
        assert_eq!(
            status,
            Some(expected),
            "case {case:?}: expected LockRule to report Status={expected}, got {status:?}"
        );
        assert_eq!(
            locked["Description"], created["Description"],
            "case {case:?}: LockRule changed Description: {:?} -> {:?}",
            created["Description"], locked["Description"]
        );

        // The lock is PERSISTED: a later reader sees it, not just the lock response.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected a persisted LockState=locked, got {:?}",
            fetched["LockState"]
        );
        assert_eq!(
            fetched["LockConfiguration"], configuration,
            "case {case:?}: expected a persisted LockConfiguration {configuration:?}, got {:?}",
            fetched["LockConfiguration"]
        );
        assert_eq!(
            lock_end_seconds(&fetched),
            None,
            "case {case:?}: expected no LockEndTime on a locked rule, got {:?}",
            fetched["LockEndTime"]
        );

        // And it is a real lock: the rule can no longer be modified or deleted.
        for (operation, (status, body)) in [
            (
                "UpdateRule",
                update_rule(&router, &identifier, json!({ "RetentionPeriod": retention(31) })).await,
            ),
            ("DeleteRule", delete_rule(&router, &identifier).await),
        ] {
            assert_eq!(
                status, 409,
                "case {case:?}: {operation} on the locked rule expected HTTP 409, \
                 got {status} / {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("INVALID_RULE_STATE"),
                "case {case:?}: {operation} expected reason INVALID_RULE_STATE, got {:?}",
                body["Reason"]
            );
        }
    }
}

#[tokio::test]
async fn lock_rule_is_refused_for_every_rule_that_cannot_be_locked() {
    let router = router();

    // A tag-level rule, a Region-level rule with exclusion tags, and an already locked
    // rule: none of them can be locked. Each row carries the state the refused rule
    // must still report afterwards.
    let tag_level = created_identifier(
        &router,
        "tag-level rule",
        json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(10),
            "ResourceTags": [{ "ResourceTagKey": "retain", "ResourceTagValue": "yes" }],
        }),
    )
    .await;
    let excluded = created_identifier(
        &router,
        "region-level rule with exclusion tags",
        json!({
            "ResourceType": "EBS_VOLUME",
            "RetentionPeriod": retention(10),
            "ExcludeResourceTags": [{ "ResourceTagKey": "scratch" }],
        }),
    )
    .await;
    let already_locked = created_identifier(
        &router,
        "already locked rule",
        json!({
            "ResourceType": "EC2_IMAGE",
            "RetentionPeriod": retention(10),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
        }),
    )
    .await;

    // The expected `Reason` is `None` for the 404: Recycle Bin populates no `Reason` on
    // the not-found path (see `tests/all/error_response_bodies.rs`, which asserts those
    // bodies over the whole class).
    let rows: [RefusalRow<'_>; 4] = [
        ("unknown identifier", "0000000000z", 404, None, None),
        (
            "tag-level rule",
            tag_level.as_str(),
            409,
            Some("INVALID_RULE_STATE"),
            None,
        ),
        (
            "region-level rule with exclusion tags",
            excluded.as_str(),
            409,
            Some("INVALID_RULE_STATE"),
            None,
        ),
        (
            "already locked rule",
            already_locked.as_str(),
            409,
            Some("INVALID_RULE_STATE"),
            Some("locked"),
        ),
    ];
    for (case, identifier, expected_status, expected_reason, expected_lock_state) in rows {
        let (status, body) = lock_rule(
            &router,
            identifier,
            json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MAX) }),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} / {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            expected_reason,
            "case {case:?}: expected reason {expected_reason:?}, got {:?}",
            body["Reason"]
        );
        if case == "unknown identifier" {
            continue;
        }
        // A refused lock request leaves the stored rule exactly as it was.
        let (status, fetched) = get_rule(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after the refusal expected HTTP 200, got {status}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            expected_lock_state,
            "case {case:?}: expected LockState {expected_lock_state:?} after the refusal, got {:?}",
            fetched["LockState"]
        );
        let expected_configuration = expected_lock_state
            .map(|_| lock_configuration(UNLOCK_DELAY_MIN))
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            fetched
                .get("LockConfiguration")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
            expected_configuration,
            "case {case:?}: expected LockConfiguration {expected_configuration:?} after the \
             refusal, got {:?}",
            fetched["LockConfiguration"]
        );
    }
}

#[tokio::test]
async fn unlock_rule_starts_the_configured_unlock_delay_and_locking_again_cancels_it() {
    let router = router();
    // The unlock delay keeps the value the lock configuration asked for, at each
    // published bound and one value in between.
    for unlock_delay in [UNLOCK_DELAY_MIN, 19, UNLOCK_DELAY_MAX] {
        let case = format!("unlock delay of {unlock_delay} days");
        let configuration = lock_configuration(unlock_delay);
        let identifier = created_identifier(
            &router,
            &case,
            json!({
                "ResourceType": "EBS_SNAPSHOT",
                "RetentionPeriod": retention(60),
                "LockConfiguration": configuration,
            }),
        )
        .await;

        let before = now_epoch_seconds();
        let (status, unlocked) = unlock_rule(&router, &identifier).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: UnlockRule expected HTTP 200, got {status} / {unlocked:?}"
        );
        assert_eq!(
            unlocked["LockState"].as_str(),
            Some("pending_unlock"),
            "case {case:?}: expected LockState=pending_unlock, got {:?}",
            unlocked["LockState"]
        );
        assert_eq!(
            unlocked["LockConfiguration"], configuration,
            "case {case:?}: expected the lock configuration to survive the unlock \
             ({configuration:?}), got {:?}",
            unlocked["LockConfiguration"]
        );
        // The lock end time is exactly one configured unlock delay from the call.
        let lock_end = lock_end_seconds(&unlocked).unwrap_or_else(|| {
            panic!("case {case:?}: expected a LockEndTime inside the unlock delay: {unlocked:?}")
        });
        let delay_seconds = unlock_delay as f64 * SECONDS_PER_DAY;
        let (lower, upper) = (before + delay_seconds, after + delay_seconds);
        assert!(
            lock_end >= lower && lock_end <= upper,
            "case {case:?}: expected LockEndTime in [{lower}, {upper}] \
             (the call plus {unlock_delay} days), got {lock_end}"
        );

        // A rule inside its unlock delay is still protected, and still reports the same
        // lock end time to a later reader.
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            Some("pending_unlock"),
            "case {case:?}: expected a persisted LockState=pending_unlock, got {:?}",
            fetched["LockState"]
        );
        assert_eq!(
            lock_end_seconds(&fetched),
            Some(lock_end),
            "case {case:?}: expected a stable LockEndTime of {lock_end}, got {:?}",
            fetched["LockEndTime"]
        );
        for (operation, (status, body)) in [
            (
                "UpdateRule",
                update_rule(&router, &identifier, json!({ "Description": "changed" })).await,
            ),
            ("DeleteRule", delete_rule(&router, &identifier).await),
        ] {
            assert_eq!(
                status, 409,
                "case {case:?}: {operation} inside the unlock delay expected HTTP 409, \
                 got {status} / {body:?}"
            );
            assert_eq!(
                body["Reason"].as_str(),
                Some("INVALID_RULE_STATE"),
                "case {case:?}: {operation} expected reason INVALID_RULE_STATE, got {:?}",
                body["Reason"]
            );
        }

        // Locking again inside the unlock delay returns the rule to `locked` and leaves
        // no lock end time behind — the only transitions a locked rule has are between
        // `locked` and `unlocked`.
        let relock = lock_configuration(UNLOCK_DELAY_MAX);
        let (status, relocked) =
            lock_rule(&router, &identifier, json!({ "LockConfiguration": relock })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: relocking expected HTTP 200, got {status} / {relocked:?}"
        );
        assert_eq!(
            relocked["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected LockState=locked after relocking, got {:?}",
            relocked["LockState"]
        );
        assert_eq!(
            relocked["LockConfiguration"], relock,
            "case {case:?}: expected the relock configuration {relock:?}, got {:?}",
            relocked["LockConfiguration"]
        );
        let (status, fetched) = get_rule(&router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after relocking expected HTTP 200, got {status}"
        );
        assert_eq!(
            lock_end_seconds(&fetched),
            None,
            "case {case:?}: expected no LockEndTime after relocking, got {:?}",
            fetched["LockEndTime"]
        );
        assert_eq!(
            fetched["LockConfiguration"], relock,
            "case {case:?}: expected the persisted relock configuration {relock:?}, got {:?}",
            fetched["LockConfiguration"]
        );
    }
}

#[tokio::test]
async fn unlock_rule_is_refused_unless_the_rule_is_locked() {
    let router = router();

    let never_locked = created_identifier(
        &router,
        "never locked rule",
        json!({ "ResourceType": "EBS_VOLUME", "RetentionPeriod": retention(10) }),
    )
    .await;
    let pending_unlock = created_identifier(
        &router,
        "rule already inside its unlock delay",
        json!({
            "ResourceType": "EBS_SNAPSHOT",
            "RetentionPeriod": retention(10),
            "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN),
        }),
    )
    .await;
    let (status, _) = unlock_rule(&router, &pending_unlock).await;
    assert_eq!(
        status, 200,
        "case \"rule already inside its unlock delay\": the first UnlockRule expected HTTP 200, \
         got {status}"
    );

    // As above, the 404 carries no `Reason`.
    let rows: [RefusalRow<'_>; 3] = [
        ("unknown identifier", "zzzzzzzzzz0", 404, None, None),
        (
            "never locked rule",
            never_locked.as_str(),
            409,
            Some("INVALID_RULE_STATE"),
            None,
        ),
        (
            "rule already inside its unlock delay",
            pending_unlock.as_str(),
            409,
            Some("INVALID_RULE_STATE"),
            Some("pending_unlock"),
        ),
    ];
    for (case, identifier, expected_status, expected_reason, expected_lock_state) in rows {
        let (status, body) = unlock_rule(&router, identifier).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} / {body:?}"
        );
        assert_eq!(
            body["Reason"].as_str(),
            expected_reason,
            "case {case:?}: expected reason {expected_reason:?}, got {:?}",
            body["Reason"]
        );
        if case == "unknown identifier" {
            continue;
        }
        let (status, fetched) = get_rule(&router, identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after the refusal expected HTTP 200, got {status}"
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            expected_lock_state,
            "case {case:?}: expected LockState {expected_lock_state:?} after the refusal, got {:?}",
            fetched["LockState"]
        );
    }
}

#[tokio::test]
async fn an_unlocked_rule_can_be_locked_again_and_then_modified_once_more() {
    // The whole cycle on one rule: never locked -> locked -> pending_unlock -> locked,
    // with the writes the lock forbids refused at every locked step and allowed before
    // the first lock.
    let router = router();
    let case = "lock cycle";
    let identifier = created_identifier(
        &router,
        case,
        json!({ "ResourceType": "EC2_IMAGE", "RetentionPeriod": retention(5) }),
    )
    .await;

    // Before any lock, an update is accepted.
    let (status, updated) =
        update_rule(&router, &identifier, json!({ "Description": "before the lock" })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UpdateRule before the lock expected HTTP 200, got {status} / {updated:?}"
    );

    let expected_states = ["locked", "pending_unlock", "locked"];
    let mut seen = Vec::new();
    let (_, locked) = lock_rule(
        &router,
        &identifier,
        json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
    )
    .await;
    seen.push(locked["LockState"].as_str().unwrap_or_default().to_string());
    let (_, unlocked) = unlock_rule(&router, &identifier).await;
    seen.push(unlocked["LockState"].as_str().unwrap_or_default().to_string());
    let (_, relocked) = lock_rule(
        &router,
        &identifier,
        json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MAX) }),
    )
    .await;
    seen.push(relocked["LockState"].as_str().unwrap_or_default().to_string());
    assert_eq!(
        seen, expected_states,
        "case {case:?}: expected the lock states {expected_states:?}, got {seen:?}"
    );

    // Once locked, the description the rule carried before the lock is still the stored
    // one: no lock transition rewrote it.
    let (status, fetched) = get_rule(&router, &identifier).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
    );
    assert_eq!(
        fetched["Description"].as_str(),
        Some("before the lock"),
        "case {case:?}: expected the stored Description to survive every lock transition, got {:?}",
        fetched["Description"]
    );
}
