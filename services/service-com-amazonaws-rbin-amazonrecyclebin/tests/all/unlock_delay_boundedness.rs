// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the one thing an unlock delay must always be: BOUNDED.
//!
//! The rule under test: a retention rule reports `pending_unlock` only while it is inside
//! an unlock delay whose end instant the service knows, so an unlock request that can
//! measure no delay from the rule's stored lock configuration serves none — the rule is
//! `unlocked` at once, reports no `LockEndTime`, and can be modified and deleted again —
//! and, read from the other side, a stored `pending_unlock` carrying no lock end time
//! names no delay period at all and is settled to `unlocked` by every reader of the lock
//! state. No input can leave a rule inside a delay period it never leaves.
//!
//! The table spans what that rule generalises over: a locked rule holding no lock
//! configuration, an unlock delay measured in a unit the model does not define, a delay
//! of zero and of a negative length, and the measurable delays (one day, the published
//! bounds, a value between them and one far beyond them) whose end the service does
//! serve. The inputs past the published range and past the modelled unit are reachable
//! only from the store, since the boundary validator refuses them on the wire, so each
//! row seeds the stored lock configuration a locked rule carries and then drives the
//! unlock through the real router, exactly as a signed SDK request would. Every row
//! states its own case so a failure names the input.

use std::time::{Duration, SystemTime};

use serde_json::json;

use crate::retention_rule_lifecycle::{
    create_rule, delete_rule, get_rule, list_of, list_rules, lock_configuration, retention,
    update_rule, RESOURCE_TYPES, UNLOCK_DELAY_MAX, UNLOCK_DELAY_MIN,
};
use crate::rule_lock_transitions::{lock_end_seconds, lock_rule, now_epoch_seconds, unlock_rule};
use crate::update_rule_lock_invariant::{fixture, Fixture};
use service_com_amazonaws_rbin_amazonrecyclebin::ops::types::{LockConfiguration, UnlockDelay};
use service_com_amazonaws_rbin_amazonrecyclebin::state::{
    service_state, unlock_delay_end, LOCK_STATE_PENDING_UNLOCK, SECONDS_PER_DAY,
};

/// The published bounds of `UnlockDelay$UnlockDelayValue` in the member's OWN type, the
/// one a stored lock configuration carries; [`UNLOCK_DELAY_MIN`] and [`UNLOCK_DELAY_MAX`]
/// are the same two bounds as a JSON body spells them. Both bounds, and their source, are
/// the shared suite's: `crate::retention_rule_lifecycle`.
const MIN_DELAY_VALUE: i32 = UNLOCK_DELAY_MIN as i32;
const MAX_DELAY_VALUE: i32 = UNLOCK_DELAY_MAX as i32;

/// The only unit an unlock delay can be measured in — "Currently, the unlock delay can
/// be measured only in days".
/// Source: smithy_query(query="shape", shape="UnlockDelayUnit") — enum values ["DAYS"].
const UNLOCK_DELAY_UNIT: &str = "DAYS";

/// The retention period every rule in this suite is created with, in days — one every
/// modelled resource type supports.
const CREATED_RETENTION: i64 = 7;

/// One row of the unlock table.
struct Row {
    case: &'static str,
    resource_type: &'static str,
    /// The unlock delay the STORE holds when the unlock is requested, as
    /// `(UnlockDelayValue, UnlockDelayUnit)`; `None` for a locked rule that holds no lock
    /// configuration at all.
    stored_delay: Option<(i32, &'static str)>,
    /// How long the delay the service must serve is, in seconds, or `None` when no delay
    /// can be measured from the stored configuration, so the rule has none to serve.
    expected_delay_seconds: Option<u64>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a locked rule holding no lock configuration",
            resource_type: RESOURCE_TYPES[0],
            stored_delay: None,
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay in a unit the model does not define",
            resource_type: RESOURCE_TYPES[1],
            stored_delay: Some((MIN_DELAY_VALUE, "HOURS")),
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay in an empty unit",
            resource_type: RESOURCE_TYPES[2],
            stored_delay: Some((MAX_DELAY_VALUE, "")),
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay in the modelled unit spelled in lower case",
            resource_type: RESOURCE_TYPES[0],
            stored_delay: Some((MIN_DELAY_VALUE, "days")),
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay of zero days",
            resource_type: RESOURCE_TYPES[1],
            stored_delay: Some((0, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay of a negative number of days",
            resource_type: RESOURCE_TYPES[2],
            stored_delay: Some((-5, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: None,
        },
        Row {
            case: "an unlock delay of one day",
            resource_type: RESOURCE_TYPES[0],
            stored_delay: Some((1, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: Some(SECONDS_PER_DAY),
        },
        Row {
            case: "an unlock delay at the published minimum",
            resource_type: RESOURCE_TYPES[1],
            stored_delay: Some((MIN_DELAY_VALUE, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: Some(UNLOCK_DELAY_MIN as u64 * SECONDS_PER_DAY),
        },
        Row {
            case: "an unlock delay between the published bounds",
            resource_type: RESOURCE_TYPES[2],
            stored_delay: Some((19, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: Some(19 * SECONDS_PER_DAY),
        },
        Row {
            case: "an unlock delay at the published maximum",
            resource_type: RESOURCE_TYPES[0],
            stored_delay: Some((MAX_DELAY_VALUE, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: Some(UNLOCK_DELAY_MAX as u64 * SECONDS_PER_DAY),
        },
        Row {
            case: "an unlock delay far past the published maximum",
            resource_type: RESOURCE_TYPES[1],
            stored_delay: Some((3650, UNLOCK_DELAY_UNIT)),
            expected_delay_seconds: Some(3650 * SECONDS_PER_DAY),
        },
    ]
}

/// The lock configuration a `(UnlockDelayValue, UnlockDelayUnit)` pair names.
fn stored_lock(value: i32, unit: &str) -> LockConfiguration {
    LockConfiguration {
        unlock_delay: UnlockDelay {
            unlock_delay_value: value,
            unlock_delay_unit: unit.to_string(),
        },
    }
}

/// Create a rule and lock it on the wire, and hand back its minted identifier.
async fn locked_rule(fixture: &Fixture, case: &str, resource_type: &str) -> String {
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
        .unwrap_or_else(|| panic!("case {case:?}: CreateRule returned no Identifier: {created:?}"))
        .to_string();
    let (status, locked) = lock_rule(
        &fixture.router,
        &identifier,
        json!({ "LockConfiguration": lock_configuration(UNLOCK_DELAY_MIN) }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: LockRule expected HTTP 200, got {status} / {locked:?}"
    );
    assert_eq!(
        locked["LockState"].as_str(),
        Some("locked"),
        "case {case:?}: expected LockState=locked before the unlock, got {:?}",
        locked["LockState"]
    );
    identifier
}

/// Put `stored_delay` in the store as the lock configuration the rule carries.
///
/// The boundary validator refuses a value outside 7..=30 and a unit other than the one
/// the model defines before a handler runs, so an unlock whose delay cannot be measured
/// is reached by seeding the stored record — the same record a handler reads.
fn store_lock_configuration(
    engine: &engine::InMemoryEngine,
    case: &str,
    identifier: &str,
    stored_delay: Option<(i32, &str)>,
) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: the rbin retention-rule store is unavailable"));
    let mut store = state.lock();
    let rule = store
        .rules
        .get_mut(identifier)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no rule {identifier:?}"));
    rule.lock_configuration = stored_delay.map(|(value, unit)| stored_lock(value, unit));
}

/// Leave the rule `pending_unlock` with NO lock end time, the record a reader must not
/// read as a delay period without an end.
fn store_pending_unlock_without_an_end(
    engine: &engine::InMemoryEngine,
    case: &str,
    identifier: &str,
) {
    let state = service_state(engine)
        .unwrap_or_else(|| panic!("case {case:?}: the rbin retention-rule store is unavailable"));
    let mut store = state.lock();
    let rule = store
        .rules
        .get_mut(identifier)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no rule {identifier:?}"));
    rule.lock_state = Some(LOCK_STATE_PENDING_UNLOCK.to_string());
    rule.lock_end_time = None;
}

#[tokio::test]
async fn unlock_rule_serves_a_delay_only_while_it_can_measure_one() {
    let fixture = fixture();
    for row in rows() {
        let case = row.case;
        let identifier = locked_rule(&fixture, case, row.resource_type).await;
        store_lock_configuration(&fixture.engine, case, &identifier, row.stored_delay);

        let before = now_epoch_seconds();
        let (status, unlocked) = unlock_rule(&fixture.router, &identifier).await;
        let after = now_epoch_seconds();
        assert_eq!(
            status, 200,
            "case {case:?}: UnlockRule expected HTTP 200, got {status} / {unlocked:?}"
        );

        match row.expected_delay_seconds {
            // A measurable delay is served: the rule is inside it, reports the instant it
            // ends, and is refused every modification until then.
            Some(delay) => {
                assert_eq!(
                    unlocked["LockState"].as_str(),
                    Some("pending_unlock"),
                    "case {case:?}: expected LockState=pending_unlock, got {:?}",
                    unlocked["LockState"]
                );
                let lock_end = lock_end_seconds(&unlocked).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a LockEndTime inside the unlock delay, got \
                         {unlocked:?}"
                    )
                });
                let delay = delay as f64;
                let (lower, upper) = (before + delay, after + delay);
                assert!(
                    lock_end >= lower && lock_end <= upper,
                    "case {case:?}: expected LockEndTime in [{lower}, {upper}] (the call plus \
                     the stored delay), got {lock_end}"
                );
                let (status, fetched) = get_rule(&fixture.router, &identifier).await;
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
                        update_rule(
                            &fixture.router,
                            &identifier,
                            json!({ "Description": "inside the delay" }),
                        )
                        .await,
                    ),
                    ("DeleteRule", delete_rule(&fixture.router, &identifier).await),
                ] {
                    assert_eq!(
                        status, 409,
                        "case {case:?}: {operation} inside the unlock delay expected HTTP 409, \
                         got {status} / {body:?}"
                    );
                }
            }
            // No delay can be measured, so there is none to serve: the rule leaves
            // `pending_unlock` at once, reports no end time, and is writable again.
            None => {
                assert_eq!(
                    unlocked["LockState"].as_str(),
                    Some("unlocked"),
                    "case {case:?}: expected LockState=unlocked when no delay can be measured, \
                     got {:?}",
                    unlocked["LockState"]
                );
                assert_eq!(
                    lock_end_seconds(&unlocked),
                    None,
                    "case {case:?}: expected no LockEndTime outside a delay period, got {:?}",
                    unlocked["LockEndTime"]
                );
                let (status, fetched) = get_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
                );
                assert_eq!(
                    fetched["LockState"].as_str(),
                    Some("unlocked"),
                    "case {case:?}: expected a persisted LockState=unlocked, got {:?}",
                    fetched["LockState"]
                );
                assert_eq!(
                    lock_end_seconds(&fetched),
                    None,
                    "case {case:?}: expected no persisted LockEndTime, got {:?}",
                    fetched["LockEndTime"]
                );

                // ROUND TRIP: the rule takes a modification again, and then the deletion
                // an unbounded delay would refuse for ever.
                let (status, updated) = update_rule(
                    &fixture.router,
                    &identifier,
                    json!({ "Description": "writable again" }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
                );
                let (status, fetched) = get_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetRule after the update expected HTTP 200, got {status}"
                );
                assert_eq!(
                    fetched["Description"].as_str(),
                    Some("writable again"),
                    "case {case:?}: expected the update to be stored, got {:?}",
                    fetched["Description"]
                );
                let (status, body) = delete_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: DeleteRule expected HTTP 204, got {status} / {body:?}"
                );
                let (status, body) = get_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: GetRule after the delete expected HTTP 404, got {status} / \
                     {body:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_pending_unlock_rule_with_no_end_time_is_inside_no_delay_period() {
    let fixture = fixture();
    // The same record read through every operation that reads a lock state: the absent
    // end time names no delay period, so each of them answers `unlocked` and none of them
    // refuses the write a delay would.
    for reader in ["GetRule", "ListRules", "UpdateRule", "DeleteRule"] {
        let case = format!("{reader} on a pending_unlock rule with no lock end time");
        let identifier = locked_rule(&fixture, &case, RESOURCE_TYPES[0]).await;
        store_pending_unlock_without_an_end(&fixture.engine, &case, &identifier);

        match reader {
            "GetRule" => {
                let (status, fetched) = get_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: GetRule expected HTTP 200, got {status} / {fetched:?}"
                );
                assert_eq!(
                    fetched["LockState"].as_str(),
                    Some("unlocked"),
                    "case {case:?}: expected LockState=unlocked, got {:?}",
                    fetched["LockState"]
                );
                assert_eq!(
                    lock_end_seconds(&fetched),
                    None,
                    "case {case:?}: expected no LockEndTime, got {:?}",
                    fetched["LockEndTime"]
                );
            }
            "ListRules" => {
                let (status, body) = list_rules(
                    &fixture.router,
                    json!({ "ResourceType": RESOURCE_TYPES[0] }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: ListRules expected HTTP 200, got {status} / {body:?}"
                );
                let reported = list_of(&body, "Rules")
                    .iter()
                    .find(|summary| summary["Identifier"].as_str() == Some(identifier.as_str()))
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: no summary for {identifier:?} in {body:?}")
                    });
                assert_eq!(
                    reported["LockState"].as_str(),
                    Some("unlocked"),
                    "case {case:?}: expected the summary to report LockState=unlocked, got {:?}",
                    reported["LockState"]
                );
            }
            "UpdateRule" => {
                let (status, updated) = update_rule(
                    &fixture.router,
                    &identifier,
                    json!({ "Description": "no delay is running" }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateRule expected HTTP 200, got {status} / {updated:?}"
                );
                assert_eq!(
                    updated["LockState"].as_str(),
                    Some("unlocked"),
                    "case {case:?}: expected LockState=unlocked, got {:?}",
                    updated["LockState"]
                );
            }
            _ => {
                let (status, body) = delete_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 204,
                    "case {case:?}: DeleteRule expected HTTP 204, got {status} / {body:?}"
                );
                let (status, body) = get_rule(&fixture.router, &identifier).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: GetRule after the delete expected HTTP 404, got {status} / \
                     {body:?}"
                );
            }
        }
    }
}

/// The latest instant this platform's clock type can represent, reached by doubling down
/// from the epoch: the one input on which adding any delay at all overflows, which no
/// wire request can produce.
fn latest_representable_instant() -> SystemTime {
    let mut found = SystemTime::UNIX_EPOCH;
    let mut step = Duration::from_secs(1 << 62);
    while step >= Duration::from_secs(1) {
        while let Some(next) = found.checked_add(step) {
            found = next;
        }
        step /= 2;
    }
    found
}

/// One row of the delay-measurement table: the case, the stored
/// `(UnlockDelayValue, UnlockDelayUnit)` pair (`None` for a rule holding no lock
/// configuration), and the number of seconds the delay it measures must last.
type DelayRow<'a> = (&'a str, Option<(i32, &'a str)>, u64);

#[test]
fn the_end_of_an_unlock_delay_is_the_delay_the_configuration_measures_or_no_delay() {
    // A fixed instant, so every row's expected end is exact rather than bracketed.
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    // `(case, stored delay, the seconds the delay must last)`: every unit and value a
    // stored configuration can carry, including the extremes of the member's own type.
    let rows: [DelayRow<'_>; 10] = [
        ("no lock configuration", None, 0),
        ("one day", Some((1, UNLOCK_DELAY_UNIT)), SECONDS_PER_DAY),
        (
            "the published minimum",
            Some((MIN_DELAY_VALUE, UNLOCK_DELAY_UNIT)),
            UNLOCK_DELAY_MIN as u64 * SECONDS_PER_DAY,
        ),
        (
            "the published maximum",
            Some((MAX_DELAY_VALUE, UNLOCK_DELAY_UNIT)),
            UNLOCK_DELAY_MAX as u64 * SECONDS_PER_DAY,
        ),
        (
            "one day past the published maximum",
            Some((MAX_DELAY_VALUE + 1, UNLOCK_DELAY_UNIT)),
            (UNLOCK_DELAY_MAX as u64 + 1) * SECONDS_PER_DAY,
        ),
        (
            "the largest value the member's type can carry",
            Some((i32::MAX, UNLOCK_DELAY_UNIT)),
            i32::MAX as u64 * SECONDS_PER_DAY,
        ),
        ("zero days", Some((0, UNLOCK_DELAY_UNIT)), 0),
        (
            "the smallest value the member's type can carry",
            Some((i32::MIN, UNLOCK_DELAY_UNIT)),
            0,
        ),
        ("a unit the model does not define", Some((7, "HOURS")), 0),
        ("the modelled unit in lower case", Some((7, "days")), 0),
    ];
    for (case, stored, seconds) in rows {
        let configuration = stored.map(|(value, unit)| stored_lock(value, unit));
        let end = unlock_delay_end(now, configuration.as_ref());
        let expected = now + Duration::from_secs(seconds);
        assert_eq!(
            end, expected,
            "case {case:?}: expected the unlock delay to end at {expected:?}, got {end:?}"
        );
    }
}

#[test]
fn an_unlock_delay_that_cannot_be_represented_is_over_at_once() {
    // A clock so far ahead that no delay fits after it: the delay is over at the instant
    // it starts, rather than an end no caller can ever reach.
    let case = "a clock at the end of representable time";
    let far = latest_representable_instant();
    for value in [1, MIN_DELAY_VALUE, MAX_DELAY_VALUE, i32::MAX] {
        let configuration = stored_lock(value, UNLOCK_DELAY_UNIT);
        let end = unlock_delay_end(far, Some(&configuration));
        assert_eq!(
            end, far,
            "case {case:?}: expected a {value}-day delay to be over at {far:?}, got {end:?}"
        );
    }
}
