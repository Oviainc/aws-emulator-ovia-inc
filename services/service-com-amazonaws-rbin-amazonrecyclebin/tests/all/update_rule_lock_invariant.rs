// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `UpdateRule` against the one kind of retention rule a lock
//! can hold on.
//!
//! The rule under test: a lock belongs only to a Region-level retention rule carrying no
//! exclusion tags, so no operation may leave a rule HOLDING a lock configuration while
//! its resource selection makes it one that cannot be locked. `CreateRule` and
//! `LockRule` are held to that invariant by their own suite
//! (`tests/all/lockable_rule_invariant.rs`); this suite holds the third door to it —
//! `UpdateRule`, the only other operation that can change the two members the invariant
//! reads (`ResourceTags`, `ExcludeResourceTags`).
//!
//! So a rule that holds a lock is refused a selection of either forbidden kind with HTTP
//! 400 `ValidationException` naming the selection member the request sent, and nothing
//! else the same request carried is applied; while the members a lock does not forbid
//! (description, retention period) are still updated, and a rule that has never held a
//! lock takes either selection happily.
//!
//! The guard is observable only while the rule is `unlocked`: a `locked` rule and one
//! still inside its unlock delay are refused EVERY modification with HTTP 409 (pinned by
//! `tests/all/rule_lock_transitions.rs`). An unlock delay is at least 7 days, so reaching
//! `unlocked` means moving the instant the clock owes that transition at into the past —
//! the one thing no request can bring forward — and letting the next request settle the
//! rule on its own.
//!
//! Every case drives the service through the real router, exactly as a signed SDK request
//! would, and every row states its own case so a failure names the input.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};

use crate::error_response_bodies::{message, one_validation_error};
use crate::retention_rule_lifecycle::{
    create_rule, get_rule, lock_configuration, retention, update_rule,
};
use crate::rule_lock_transitions::{lock_rule, unlock_rule};
use service_com_amazonaws_rbin_amazonrecyclebin::state::service_state;
use service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// `ResourceType` accepts exactly these values.
/// Source: smithy_query(query="shape", shape="ResourceType") — enum values.
const RESOURCE_TYPES: [&str; 3] = ["EBS_SNAPSHOT", "EBS_VOLUME", "EC2_IMAGE"];

/// Published bounds of `UnlockDelay$UnlockDelayValue`, in `UnlockDelayUnit` `DAYS`.
/// Source: smithy_query(query="shape", shape="UnlockDelay") — range {min: 7, max: 30}.
const UNLOCK_DELAY_MIN: i64 = 7;
const UNLOCK_DELAY_MAX: i64 = 30;

/// The published maximum number of exclusion tags one rule may carry.
/// Source: smithy_query(query="operation", operation="UpdateRule") —
/// `ExcludeResourceTags`, length {min: 0, max: 5}.
const MAX_EXCLUDE_RESOURCE_TAGS: usize = 5;

/// The member paths a refused selection is named by, as a validation clause spells a
/// member: the modelled member name, lower camel case.
/// Source: smithy_query(query="operation", operation="UpdateRule") — input members
/// `ResourceTags` and `ExcludeResourceTags`.
const RESOURCE_TAGS_MEMBER: &str = "resourceTags";
const EXCLUDE_RESOURCE_TAGS_MEMBER: &str = "excludeResourceTags";

/// What a selection member refused on a rule that holds a lock must state as the
/// requirement it failed: a lock holds only on a Region-level rule with no exclusion tags
/// (smithy_query(query="doc", shape="LockRule") — "You can't lock tag-level retention
/// rules, or Region-level retention rules that have exclusion tags").
const LOCK_HOLDING_REQUIREMENT: &str =
    "Member must not be specified for a retention rule with a lock configuration";

/// The description every rule in this suite is created with, so a refused update that
/// also carried a description is shown not to have applied it.
const CREATED_DESCRIPTION: &str = "created description";

/// The retention period every rule in this suite is created with, in days.
const CREATED_RETENTION: i64 = 30;

/// The shared engine and the router built on it: the engine is kept so a test can move
/// the instant a clock-owed lock transition falls due, which no request can bring forward.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

fn fixture() -> Fixture {
    // The single shared engine the production router builds and hands to every operation
    // of this service.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    // Pin authentication off so the suite does not depend on the process-wide switch.
    router.set_request_authentication(false);
    Fixture { router, engine }
}

/// Which door the rule's lock arrived through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Door {
    /// `CreateRule` with a `LockConfiguration`.
    Create,
    /// `LockRule` on a rule created without one.
    Lock,
}

/// Bring the unlock delay of `identifier` to its end.
///
/// The delay is at least 7 days, so it cannot elapse inside a test: the instant the
/// transition falls due is moved into the past instead, and the next request settles the
/// rule to `unlocked` by itself — exactly as it would once the delay had really run out.
fn end_unlock_delay(fixture: &Fixture, case: &str, identifier: &str) {
    let state = service_state(&*fixture.engine)
        .unwrap_or_else(|| panic!("case {case:?}: the rbin retention-rule store is unavailable"));
    let mut store = state.lock();
    let rule = store
        .rules
        .get_mut(identifier)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no rule {identifier:?}"));
    let past = SystemTime::now()
        .checked_sub(Duration::from_secs(1))
        .unwrap_or(SystemTime::UNIX_EPOCH);
    rule.lock_end_time = Some(past);
}

/// Create a rule that HOLDS a lock through `door` and bring it to `unlocked`, the one
/// lock state in which a modification is not refused outright, and hand back its
/// identifier.
async fn unlocked_rule_holding_a_lock(
    fixture: &Fixture,
    case: &str,
    resource_type: &str,
    door: Door,
    unlock_delay: i64,
) -> String {
    let configuration = lock_configuration(unlock_delay);
    let mut body = json!({
        "ResourceType": resource_type,
        "RetentionPeriod": retention(CREATED_RETENTION),
        "Description": CREATED_DESCRIPTION,
    });
    if door == Door::Create {
        body["LockConfiguration"] = configuration.clone();
    }
    let (status, created) = create_rule(&fixture.router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
    );
    let identifier = created["Identifier"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();

    if door == Door::Lock {
        let (status, locked) = lock_rule(
            &fixture.router,
            &identifier,
            json!({ "LockConfiguration": configuration.clone() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: LockRule expected HTTP 200, got {status} / {locked:?}"
        );
    }

    let (status, unlocking) = unlock_rule(&fixture.router, &identifier).await;
    assert_eq!(
        status, 200,
        "case {case:?}: UnlockRule expected HTTP 200, got {status} / {unlocking:?}"
    );
    assert_eq!(
        unlocking["LockState"].as_str(),
        Some("pending_unlock"),
        "case {case:?}: expected LockState=pending_unlock from UnlockRule, got {:?}",
        unlocking["LockState"]
    );
    end_unlock_delay(fixture, case, &identifier);

    // The rule is now what the guard under test is about: `unlocked`, so no longer
    // lock-protected, and still holding the lock configuration it was locked under.
    let (status, fetched) = get_rule(&fixture.router, &identifier).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
    );
    assert_eq!(
        fetched["LockState"].as_str(),
        Some("unlocked"),
        "case {case:?}: expected LockState=unlocked once the unlock delay ended, got {:?}",
        fetched["LockState"]
    );
    assert_eq!(
        fetched["LockConfiguration"], configuration,
        "case {case:?}: expected the rule to still hold {configuration:?}, got {:?}",
        fetched["LockConfiguration"]
    );
    identifier
}

/// One row of the refused-selection table: how the rule got its lock, the `UpdateRule`
/// body, and the selection member a refusal must name.
struct Row {
    case: &'static str,
    resource_type: &'static str,
    door: Door,
    unlock_delay: i64,
    update: Value,
    member: &'static str,
}

/// Every selection a rule holding a lock cannot take, varied along what the invariant
/// generalises over: which member carries the tags, how many there are (one, three, the
/// published maximum), whether a tag carries a value, whether the OTHER selection member
/// is absent or sent empty, which other members ride along in the same request, which
/// resource type the rule is for, which door the lock arrived through, and how long the
/// unlock delay was.
fn refused_rows() -> Vec<Row> {
    let mut exclusion_tags = Vec::new();
    for nth in 0..MAX_EXCLUDE_RESOURCE_TAGS {
        exclusion_tags.push(json!({ "ResourceTagKey": format!("scratch-{nth}") }));
    }
    vec![
        Row {
            case: "one resource tag with a value, lock created with the rule",
            resource_type: RESOURCE_TYPES[0],
            door: Door::Create,
            unlock_delay: UNLOCK_DELAY_MIN,
            update: json!({
                "ResourceTags": [{ "ResourceTagKey": "team", "ResourceTagValue": "ops" }],
            }),
            member: RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "one resource tag with no value, lock taken by LockRule",
            resource_type: RESOURCE_TYPES[1],
            door: Door::Lock,
            unlock_delay: UNLOCK_DELAY_MAX,
            update: json!({ "ResourceTags": [{ "ResourceTagKey": "tier" }] }),
            member: RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "three resource tags alongside a description and a retention period",
            resource_type: RESOURCE_TYPES[2],
            door: Door::Create,
            unlock_delay: 19,
            update: json!({
                "ResourceTags": [
                    { "ResourceTagKey": "team", "ResourceTagValue": "platform" },
                    { "ResourceTagKey": "env", "ResourceTagValue": "prod" },
                    { "ResourceTagKey": "keep" },
                ],
                "Description": "rewritten alongside the selection",
                "RetentionPeriod": { "RetentionPeriodValue": 11, "RetentionPeriodUnit": "DAYS" },
            }),
            member: RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "resource tags sent with an empty exclusion-tag list",
            resource_type: RESOURCE_TYPES[0],
            door: Door::Lock,
            unlock_delay: UNLOCK_DELAY_MIN + 1,
            update: json!({
                "ResourceTags": [{ "ResourceTagKey": "archive", "ResourceTagValue": "quarterly" }],
                "ExcludeResourceTags": [],
            }),
            member: RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "one exclusion tag with a value, lock created with the rule",
            resource_type: RESOURCE_TYPES[1],
            door: Door::Create,
            unlock_delay: UNLOCK_DELAY_MAX - 1,
            update: json!({
                "ExcludeResourceTags": [{ "ResourceTagKey": "skip", "ResourceTagValue": "yes" }],
            }),
            member: EXCLUDE_RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "one exclusion tag with no value, sent with an empty resource-tag list",
            resource_type: RESOURCE_TYPES[2],
            door: Door::Lock,
            unlock_delay: UNLOCK_DELAY_MIN,
            update: json!({
                "ResourceTags": [],
                "ExcludeResourceTags": [{ "ResourceTagKey": "ephemeral" }],
            }),
            member: EXCLUDE_RESOURCE_TAGS_MEMBER,
        },
        Row {
            case: "exclusion tags at the published maximum",
            resource_type: RESOURCE_TYPES[0],
            door: Door::Create,
            unlock_delay: UNLOCK_DELAY_MAX,
            update: json!({ "ExcludeResourceTags": Value::Array(exclusion_tags) }),
            member: EXCLUDE_RESOURCE_TAGS_MEMBER,
        },
    ]
}

/// Every resource tag key and value the row's update names, which a refusal must not
/// quote back.
fn submitted_tag_text(update: &Value) -> Vec<String> {
    let mut text = Vec::new();
    for member in ["ResourceTags", "ExcludeResourceTags"] {
        for tag in update
            .get(member)
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            for key in ["ResourceTagKey", "ResourceTagValue"] {
                if let Some(value) = tag.get(key).and_then(Value::as_str) {
                    if !value.is_empty() {
                        text.push(value.to_string());
                    }
                }
            }
        }
    }
    text
}

#[tokio::test]
async fn update_rule_refuses_every_selection_a_rule_holding_a_lock_cannot_take() {
    let fixture = fixture();
    for row in refused_rows() {
        let case = row.case;
        let identifier = unlocked_rule_holding_a_lock(
            &fixture,
            case,
            row.resource_type,
            row.door,
            row.unlock_delay,
        )
        .await;

        let (status, refused) = update_rule(&fixture.router, &identifier, row.update.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} / {refused:?}"
        );
        assert_eq!(
            refused["Reason"].as_str(),
            Some("INVALID_PARAMETER_VALUE"),
            "case {case:?}: expected Reason INVALID_PARAMETER_VALUE, got {:?}",
            refused["Reason"]
        );
        let expected = one_validation_error(row.member, LOCK_HOLDING_REQUIREMENT);
        assert_eq!(
            message(&refused),
            expected,
            "case {case:?}: expected the message {expected:?}, got {:?}",
            message(&refused)
        );
        // The refusal states the member and the requirement; nothing the request named is
        // quoted back.
        for submitted in submitted_tag_text(&row.update) {
            assert!(
                !message(&refused).contains(&submitted),
                "case {case:?}: the refusal echoed the submitted {submitted:?}: {:?}",
                message(&refused)
            );
        }

        // The refused update changed NOTHING: not the selection it named, not the other
        // members it carried, and not the lock the rule holds.
        let (status, fetched) = get_rule(&fixture.router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule after the refusal expected HTTP 200, got {status} / {fetched:?}"
        );
        for member in ["ResourceTags", "ExcludeResourceTags"] {
            assert_eq!(
                fetched[member],
                json!([]),
                "case {case:?}: expected the stored {member} to stay empty, got {:?}",
                fetched[member]
            );
        }
        assert_eq!(
            fetched["Description"].as_str(),
            Some(CREATED_DESCRIPTION),
            "case {case:?}: expected the stored Description to survive the refusal, got {:?}",
            fetched["Description"]
        );
        assert_eq!(
            fetched["RetentionPeriod"],
            retention(CREATED_RETENTION),
            "case {case:?}: expected the stored RetentionPeriod to survive the refusal, got {:?}",
            fetched["RetentionPeriod"]
        );
        assert_eq!(
            fetched["LockConfiguration"],
            lock_configuration(row.unlock_delay),
            "case {case:?}: expected the stored LockConfiguration to survive the refusal, got {:?}",
            fetched["LockConfiguration"]
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            Some("unlocked"),
            "case {case:?}: expected the stored LockState to survive the refusal, got {:?}",
            fetched["LockState"]
        );

        // And the rule can still be locked again: it never stopped being a rule a lock
        // can hold on, which is exactly what the refusal protects.
        let (status, relocked) = lock_rule(
            &fixture.router,
            &identifier,
            json!({ "LockConfiguration": lock_configuration(row.unlock_delay) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: LockRule after the refusal expected HTTP 200, got {status} / \
             {relocked:?}"
        );
        assert_eq!(
            relocked["LockState"].as_str(),
            Some("locked"),
            "case {case:?}: expected LockState=locked after relocking, got {:?}",
            relocked["LockState"]
        );
    }
}

#[tokio::test]
async fn update_rule_still_changes_what_a_rule_holding_a_lock_may_change() {
    let fixture = fixture();
    // The complement of the refusal table: on the SAME unlocked lock-holding rule, every
    // member the invariant says nothing about is updated, and a selection member sent
    // EMPTY is accepted — an empty selection leaves the rule Region-level with no
    // exclusion tags, the one kind a lock holds on.
    let accepted: [(&str, Value); 5] = [
        ("description alone", json!({ "Description": "still editable" })),
        (
            "retention period alone",
            json!({ "RetentionPeriod": { "RetentionPeriodValue": 7, "RetentionPeriodUnit": "DAYS" } }),
        ),
        (
            "description and retention period together",
            json!({
                "Description": "both at once",
                "RetentionPeriod": { "RetentionPeriodValue": 3650, "RetentionPeriodUnit": "DAYS" },
            }),
        ),
        ("an empty resource-tag list", json!({ "ResourceTags": [] })),
        (
            "both selection members sent empty",
            json!({ "ResourceTags": [], "ExcludeResourceTags": [] }),
        ),
    ];
    for (case, update) in accepted {
        let identifier = unlocked_rule_holding_a_lock(
            &fixture,
            case,
            RESOURCE_TYPES[0],
            Door::Create,
            UNLOCK_DELAY_MIN,
        )
        .await;
        let (status, updated) = update_rule(&fixture.router, &identifier, update.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
        );
        // ROUND TRIP: whatever the request carried is what a later reader sees, and the
        // lock the rule holds is untouched.
        let (status, fetched) = get_rule(&fixture.router, &identifier).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
        );
        for member in ["Description", "RetentionPeriod", "ResourceTags", "ExcludeResourceTags"] {
            if let Some(sent) = update.get(member) {
                assert_eq!(
                    &fetched[member], sent,
                    "case {case:?}: expected the stored {member} to be {sent:?}, got {:?}",
                    fetched[member]
                );
            }
        }
        assert_eq!(
            fetched["LockConfiguration"],
            lock_configuration(UNLOCK_DELAY_MIN),
            "case {case:?}: expected the update to leave the LockConfiguration alone, got {:?}",
            fetched["LockConfiguration"]
        );
        assert_eq!(
            fetched["LockState"].as_str(),
            Some("unlocked"),
            "case {case:?}: expected the update to leave the LockState alone, got {:?}",
            fetched["LockState"]
        );
    }
}

#[tokio::test]
async fn update_rule_gives_a_rule_that_holds_no_lock_either_selection() {
    let fixture = fixture();
    // A rule that has never been locked holds no lock configuration, so the invariant
    // says nothing about it: either selection is accepted and read back, for every
    // modelled resource type.
    let selections: [(&str, Value); 4] = [
        (
            "resource tags on a never-locked rule",
            json!({ "ResourceTags": [{ "ResourceTagKey": "team", "ResourceTagValue": "ops" }] }),
        ),
        (
            "exclusion tags on a never-locked rule",
            json!({ "ExcludeResourceTags": [{ "ResourceTagKey": "skip", "ResourceTagValue": "yes" }] }),
        ),
        (
            "a resource tag with no value on a never-locked rule",
            json!({ "ResourceTags": [{ "ResourceTagKey": "tier" }] }),
        ),
        (
            "an exclusion tag with no value on a never-locked rule",
            json!({ "ExcludeResourceTags": [{ "ResourceTagKey": "ephemeral" }] }),
        ),
    ];
    for resource_type in RESOURCE_TYPES {
        for (label, update) in &selections {
            let case = format!("{label} ({resource_type})");
            let (status, created) = create_rule(
                &fixture.router,
                json!({
                    "ResourceType": resource_type,
                    "RetentionPeriod": retention(CREATED_RETENTION),
                }),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateRule expected HTTP 201, got {status} / {created:?}"
            );
            let identifier = created["Identifier"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier"))
                .to_string();
            assert_eq!(
                created["LockState"].as_str(),
                None,
                "case {case:?}: expected a never-locked rule to report no LockState, got {:?}",
                created["LockState"]
            );

            let (status, updated) =
                update_rule(&fixture.router, &identifier, update.clone()).await;
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
            );
            let (status, fetched) = get_rule(&fixture.router, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
            );
            for member in ["ResourceTags", "ExcludeResourceTags"] {
                if let Some(sent) = update.get(member) {
                    assert_eq!(
                        &fetched[member], sent,
                        "case {case:?}: expected the stored {member} to be {sent:?}, got {:?}",
                        fetched[member]
                    );
                }
            }
        }
    }
}
