// SPDX-License-Identifier: CC0-1.0

//! `DisableAlarmActions`, end to end over the wire: what turning an alarm's actions OFF does
//! to the alarm RECORD and to its history (`alarm_action_dispatch.rs` asserts the dispatch the
//! switch governs).
//!
//! Four rules are asserted here, each over a TABLE whose rows differ along what the rule
//! generalises over — the kind of alarm, the setting the alarm already carried, how many names
//! one call carries, whether a name names an alarm at all, how many times the switch is
//! toggled — rather than along one recorded request:
//!
//! 1. `DisableAlarmActions` turns `ActionsEnabled` OFF for every alarm it names, whichever
//!    kind of alarm the name carries, and the setting is read back through `DescribeAlarms`.
//!    Turning it off is a CONFIGURATION update: it refreshes
//!    `AlarmConfigurationUpdatedTimestamp` and records one `ConfigurationUpdate` history item,
//!    while a call asking for the setting the alarm already carries updates no configuration
//!    and records nothing.
//! 2. The call changes no alarm's STATE: actions are executed on a state change, and this call
//!    makes none.
//! 3. The switch is a plain boolean member, so writing it is idempotent and round-trips: a
//!    second disable changes nothing further, and `EnableAlarmActions` puts the member back —
//!    each call that CHANGES it recording exactly one more configuration update.
//! 4. `DisableAlarmActions` declares no errors
//!    (`smithy_query(query="operation",operation="DisableAlarmActions")`), so a name no alarm
//!    carries is ignored — the alarms that do exist are disabled, the call succeeds and nothing
//!    is minted for the name that matched none — while an `AlarmNames` list past the modelled
//!    `@length 0..100` is refused at the boundary.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    alarm_history_items, alarm_stamp, build_router, call, described_alarm, error_code, AlarmKind,
};

/// Long enough for the instant the disable is stamped with to be DISTINGUISHABLE from the one
/// the creating write was stamped with.
///
/// The stored instant is held to the millisecond (`crate::state::to_timestamp`), so two writes
/// inside one millisecond would carry the same stamp and a refresh would be
/// indistinguishable from no refresh. Waiting past one millisecond is what lets the assertion
/// below be STRICT, so a handler that never refreshes the member fails it.
const PAST_ONE_STAMP: Duration = Duration::from_millis(5);

/// The modelled maximum number of names one call may carry: `AlarmNames` is `@length 0..100`
/// (`smithy_query(query="operation",operation="DisableAlarmActions")`).
const MAX_ALARM_NAMES: usize = 100;

/// The alarm name every row of the unknown-name table writes to.
const KNOWN_ALARM: &str = "carries-an-alarm";

/// Turn the named alarms' actions off, requiring a 200.
async fn disable(router: &MultiServiceRouter, names: &[&str]) {
    let (status, body) = call(router, "DisableAlarmActions", &json!({"AlarmNames": names})).await;
    assert_eq!(
        status, 200,
        "DisableAlarmActions {names:?}: expected HTTP 200, got {status} with {body}"
    );
}

/// Turn the named alarms' actions on, requiring a 200.
async fn enable(router: &MultiServiceRouter, names: &[&str]) {
    let (status, body) = call(router, "EnableAlarmActions", &json!({"AlarmNames": names})).await;
    assert_eq!(
        status, 200,
        "EnableAlarmActions {names:?}: expected HTTP 200, got {status} with {body}"
    );
}

/// How many `ConfigurationUpdate` history items `name` carries.
async fn configuration_updates(router: &MultiServiceRouter, kind: AlarmKind, name: &str) -> usize {
    alarm_history_items(router, kind, name, "ConfigurationUpdate")
        .await
        .len()
}

#[tokio::test]
async fn disabling_actions_records_one_configuration_update_only_when_the_setting_changes() {
    let router = build_router();

    // Rows differ in the KIND of alarm the name carries and in the setting the alarm already
    // carried — including the documented default of an omitted `ActionsEnabled` ("Indicates
    // whether actions should be executed during any changes to the alarm state. The default is
    // TRUE."), which a disable therefore always changes.
    for (index, (case, kind, initial)) in [
        (
            "a metric alarm whose actions are on",
            AlarmKind::Metric,
            Some(true),
        ),
        (
            "a metric alarm whose actions are already off",
            AlarmKind::Metric,
            Some(false),
        ),
        (
            "a metric alarm that took the default",
            AlarmKind::Metric,
            None,
        ),
        (
            "a composite alarm whose actions are on",
            AlarmKind::Composite,
            Some(true),
        ),
        (
            "a composite alarm whose actions are already off",
            AlarmKind::Composite,
            Some(false),
        ),
        (
            "a composite alarm that took the default",
            AlarmKind::Composite,
            None,
        ),
        (
            "a log alarm whose actions are on",
            AlarmKind::Log,
            Some(true),
        ),
        (
            "a log alarm whose actions are already off",
            AlarmKind::Log,
            Some(false),
        ),
        ("a log alarm that took the default", AlarmKind::Log, None),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("disable-row-{index}");
        kind.create(&router, &name, *initial).await;

        let before = described_alarm(&router, *kind, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the created alarm must be described"));
        let updates_before = configuration_updates(&router, *kind, &name).await;
        // Whether this call CHANGES the member is read off the alarm as stored rather than off
        // the row, so the documented default is whatever the writer actually recorded.
        let changes = before.get("ActionsEnabled").and_then(Value::as_bool) != Some(false);
        std::thread::sleep(PAST_ONE_STAMP);

        disable(&router, &[name.as_str()]).await;

        let after = described_alarm(&router, *kind, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the alarm must still be described"));
        assert_eq!(
            after.get("ActionsEnabled").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected ActionsEnabled false after the call, got {after}"
        );

        let updates_after = configuration_updates(&router, *kind, &name).await;
        let expected_updates = updates_before + usize::from(changes);
        assert_eq!(
            updates_after, expected_updates,
            "case {case:?}: expected {expected_updates} configuration-update history items \
             (one more only when the setting changed), got {updates_after}"
        );

        let configured_before = alarm_stamp(&before, "AlarmConfigurationUpdatedTimestamp");
        let configured_after = alarm_stamp(&after, "AlarmConfigurationUpdatedTimestamp");
        if changes {
            // STRICTLY later: the write is stamped with its own instant, which the wait above
            // made distinguishable from the creating write's, so equality here would mean the
            // member was never refreshed.
            assert!(
                configured_after.is_some() && configured_after > configured_before,
                "case {case:?}: a changed setting must refresh the configuration timestamp to a \
                 strictly later instant, got {configured_after:?} after {configured_before:?}"
            );
        } else {
            assert_eq!(
                configured_after, configured_before,
                "case {case:?}: a call that changes no setting must leave the configuration \
                 timestamp alone, got {configured_after:?} after {configured_before:?}"
            );
        }
    }
}

#[tokio::test]
async fn disabling_actions_leaves_the_alarm_state_alone() {
    let router = build_router();

    // Rows differ in the state the alarm is left sitting in when the switch is written: the
    // actions a transition executes are governed by the switch, and writing the switch is not
    // itself a transition.
    for (index, (case, forced)) in [
        ("an alarm forced into ALARM", "ALARM"),
        ("an alarm forced into OK", "OK"),
        (
            "an alarm forced into INSUFFICIENT_DATA",
            "INSUFFICIENT_DATA",
        ),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("state-row-{index}");
        AlarmKind::Metric.create(&router, &name, Some(true)).await;
        // A state a caller forced, so the state under test is one nothing else is about to
        // recompute: `SetAlarmState`'s state stands until this alarm's next evaluation, a short
        // span away, and this row reads it back inside that span.
        let (status, body) = call(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": name,
                "StateValue": forced,
                "StateReason": "driven by the test",
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: SetAlarmState must succeed, got {status} with {body}"
        );
        let before = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the forced alarm must be described"));

        disable(&router, &[name.as_str()]).await;

        let after = described_alarm(&router, AlarmKind::Metric, &name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the alarm must still be described"));
        for member in [
            "StateValue",
            "StateReason",
            "StateReasonData",
            "StateUpdatedTimestamp",
            "StateTransitionedTimestamp",
        ] {
            assert_eq!(
                after.get(member),
                before.get(member),
                "case {case:?}: expected disabling the actions to leave {member:?} alone, \
                 got {after}"
            );
        }
        assert_eq!(
            after.get("ActionsEnabled").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: the switch itself must be off, got {after}"
        );
    }
}

#[tokio::test]
async fn toggling_the_switch_round_trips_and_repeating_a_call_changes_nothing() {
    let router = build_router();

    // Rows differ in the kind of alarm, because every kind carries the same `ActionsEnabled`
    // member and so the same toggle.
    for (index, (case, kind)) in [
        ("a metric alarm", AlarmKind::Metric),
        ("a composite alarm", AlarmKind::Composite),
        ("a log alarm", AlarmKind::Log),
    ]
    .iter()
    .enumerate()
    {
        let name = format!("toggle-row-{index}");
        kind.create(&router, &name, Some(true)).await;
        let mut updates = configuration_updates(&router, *kind, &name).await;

        // Each step states the call, the setting the alarm must report afterwards, and whether
        // the call changed the member — a repeated call changes nothing, so it records nothing.
        for (step, call_name, expected, changed) in [
            (1, "DisableAlarmActions", false, true),
            (2, "DisableAlarmActions", false, false),
            (3, "EnableAlarmActions", true, true),
            (4, "EnableAlarmActions", true, false),
            (5, "DisableAlarmActions", false, true),
        ] {
            if call_name == "DisableAlarmActions" {
                disable(&router, &[name.as_str()]).await;
            } else {
                enable(&router, &[name.as_str()]).await;
            }

            let alarm = described_alarm(&router, *kind, &name)
                .await
                .unwrap_or_else(|| panic!("case {case:?} step {step}: the alarm must exist"));
            assert_eq!(
                alarm.get("ActionsEnabled").and_then(Value::as_bool),
                Some(expected),
                "case {case:?} step {step} ({call_name}): expected ActionsEnabled {expected}, \
                 got {alarm}"
            );

            updates += usize::from(changed);
            let recorded = configuration_updates(&router, *kind, &name).await;
            assert_eq!(
                recorded, updates,
                "case {case:?} step {step} ({call_name}): expected {updates} \
                 configuration-update history items, got {recorded}"
            );
        }
    }
}

#[tokio::test]
async fn a_name_no_alarm_carries_is_ignored_and_a_list_past_the_bound_is_refused() {
    let router = build_router();
    AlarmKind::Metric
        .create(&router, KNOWN_ALARM, Some(true))
        .await;

    // Rows differ in how many names the call carries and how many of them name an alarm: the
    // empty list the model allows, a name nothing carries, a mix, the same name twice, and the
    // modelled maximum number of names.
    let at_the_bound: Vec<String> = (0..MAX_ALARM_NAMES)
        .map(|index| {
            if index == 0 {
                KNOWN_ALARM.to_string()
            } else {
                format!("absent-{index}")
            }
        })
        .collect();
    for (case, names) in [
        ("no name at all", Vec::new()),
        ("only a name nothing carries", vec!["absent".to_string()]),
        (
            "a known name beside an unknown one",
            vec!["absent".to_string(), KNOWN_ALARM.to_string()],
        ),
        (
            "the same name twice",
            vec![KNOWN_ALARM.to_string(), KNOWN_ALARM.to_string()],
        ),
        ("the modelled maximum number of names", at_the_bound),
    ] {
        let (status, body) =
            call(&router, "DisableAlarmActions", &json!({"AlarmNames": names})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the call to succeed with HTTP 200, got {status} with {body}"
        );
        // Nothing is created for a name no alarm carries: the call disables alarms, it does not
        // mint them.
        for absent in names.iter().filter(|name| *name != KNOWN_ALARM) {
            let created = described_alarm(&router, AlarmKind::Metric, absent).await;
            assert!(
                created.is_none(),
                "case {case:?}: expected no alarm to exist for {absent:?}, got {created:?}"
            );
        }
        if names.iter().any(|name| name == KNOWN_ALARM) {
            let alarm = described_alarm(&router, AlarmKind::Metric, KNOWN_ALARM)
                .await
                .unwrap_or_else(|| panic!("case {case:?}: the known alarm must be described"));
            assert_eq!(
                alarm.get("ActionsEnabled").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: the alarm the call did name must be disabled, got {alarm}"
            );
        }
    }

    // A name the request repeats names ONE alarm, so the repeat records nothing beyond the
    // single change the whole table made.
    let updates = configuration_updates(&router, AlarmKind::Metric, KNOWN_ALARM).await;
    assert_eq!(
        updates, 2,
        "expected the creating write plus the one setting change to be the only configuration \
         updates, got {updates}"
    );

    // One name past the modelled bound is refused at the boundary, so nothing is written.
    let past_bound: Vec<String> = (0..=MAX_ALARM_NAMES)
        .map(|index| format!("absent-{index}"))
        .collect();
    let (status, body) = call(
        &router,
        "DisableAlarmActions",
        &json!({"AlarmNames": past_bound}),
    )
    .await;
    assert_eq!(
        status, 400,
        "a list of {} names is past the modelled maximum of {MAX_ALARM_NAMES} and must be \
         refused, got {status} with {body}",
        MAX_ALARM_NAMES + 1
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "a list past the modelled maximum is refused at the boundary, got {body}"
    );
}
