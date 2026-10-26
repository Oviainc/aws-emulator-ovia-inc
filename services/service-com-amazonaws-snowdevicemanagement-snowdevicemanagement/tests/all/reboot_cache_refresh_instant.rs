// SPDX-License-Identifier: CC0-1.0

//! WHEN the refresh a rebooted device's Amazon EC2 instance cache gets is applied,
//! end to end over HTTP.
//!
//! The rule under test: a change the clock owes is materialised AT THE INSTANT IT
//! CAME DUE and never at the instant someone looked, so the cache a reboot produced
//! reports the instant the reboot FINISHED — the same instant the device's check-in
//! reports — and the states and freshness a caller reads do not depend on whether
//! anybody called earlier.
//!
//! Every case goes through the router (`CreateTask`, `DescribeTask`,
//! `DescribeDevice`, `DescribeDeviceEc2Instances`), so an answer computed from the
//! calling instant rather than from persisted state cannot pass. The ground truth is
//! read over the wire too: the finish instant comes from the rebooted device's
//! `lastReachedOutAt`, never from the test's own clock.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over:
//!
//! * the observation schedule — one look long after everything settled, versus
//!   polling throughout, which is the difference the rule says must not show;
//! * the state each entry was in when its device rebooted, spanning the documented
//!   `InstanceStateName` catalog;
//! * where the entry's OWN owed instant sat relative to the reboot's finish (never
//!   due, due before it, due after it);
//! * the device, and how many instances it runs.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, managed_device_id, EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
    INSTANCE_TRANSITION_SPAN,
};

use super::device_ec2_instance_cache::{
    assert_consistent, cache_of, call, create_task, describe_task, emulator, epoch, ok, seconds,
    stage_instance, state_after_a_restart, state_name, Emulator, INSTANCE_STATES, TOLERANCE,
};

/// The devices the task reboots. The fixture runs a different number of instances on
/// each, so the rule is checked over more than one cache size.
const REBOOTED_DEVICES: &[u64] = &[0, 1];

/// Slack added to the spans the records are settled over, so a loaded host cannot
/// read a record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(600);

/// How often the polling row reads the cache while the reboot runs.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// The observation schedules the rule must give the same answer under: `None` is a
/// caller that makes NO call at all until everything has settled, `Some(interval)`
/// one that polls the cache that often from the moment the task was created.
const OBSERVATION_SCHEDULES: &[(&str, Option<Duration>)] = &[
    ("nobody looked until long after the reboot finished", None),
    (
        "the caller polled the cache throughout the reboot",
        Some(POLL_INTERVAL),
    ),
];

/// Where a staged entry's OWN owed instant sits relative to the reboot, as an offset
/// from the instant the entry was staged. `None` holds it past the case's lifetime,
/// so the reboot is the only event that touches the entry.
///
/// A reboot's execution reaches its device after `EXECUTION_START_SPAN` and finishes
/// `EXECUTION_FINISH_SPAN` later, so an offset below that sum is owed BEFORE the
/// reboot finished and one above it AFTER.
fn due_positions() -> Vec<(&'static str, Option<Duration>)> {
    let finish = EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN;
    vec![
        ("never due", None),
        (
            "due before the reboot finished",
            Some(Duration::from_millis(150)),
        ),
        (
            "due after the reboot finished",
            Some(finish + Duration::from_millis(150)),
        ),
    ]
}

/// `DescribeDevice` for one device.
async fn described_device(em: &Emulator, device: &str, case: &str) -> Value {
    let answer = call(
        &em.router,
        "POST",
        &format!("/managed-device/{device}/describe"),
        None,
    )
    .await;
    ok(&answer, case)
}

/// What one instance reported after the reboot, as the pair the rule is about: the
/// state, and how far its freshness sits from the instant its device finished
/// rebooting, in milliseconds.
type Reported = BTreeMap<String, (String, i64)>;

/// Reboot [`REBOOTED_DEVICES`] in one emulator, observing it on `schedule`, and
/// report what the cache held once everything had settled.
async fn reboot_and_read(row: &str, schedule: Option<Duration>) -> Reported {
    let em = emulator();
    let devices: Vec<String> = REBOOTED_DEVICES.iter().copied().map(managed_device_id).collect();
    let held: Vec<Vec<String>> = REBOOTED_DEVICES
        .iter()
        .copied()
        .map(device_instance_ids)
        .collect();
    assert!(
        held.iter().all(|ids| !ids.is_empty()) && held[0].len() != held[1].len(),
        "case {row:?}: the inventory must run a different, non-zero number of \
         instances on the rebooted devices, got {:?}",
        held.iter().map(Vec::len).collect::<Vec<_>>()
    );

    // The rows: the state each entry is in before the reboot, spanning the
    // documented catalog, crossed with where its own owed instant sits.
    let staged = SystemTime::now();
    let created_at = staged - Duration::from_secs(600);
    let positions = due_positions();
    let mut before_states: BTreeMap<String, String> = BTreeMap::new();
    for (slot, (device, instance)) in devices
        .iter()
        .zip(held.iter())
        .flat_map(|(device, ids)| ids.iter().map(move |id| (device, id)))
        .enumerate()
    {
        let (name, _, outcome) = INSTANCE_STATES[slot % INSTANCE_STATES.len()];
        let (_, offset) = positions[slot % positions.len()];
        let due = outcome.map(|_| match offset {
            Some(offset) => staged + offset,
            None => staged + Duration::from_secs(3600),
        });
        stage_instance(&em, device, instance, name, created_at, staged, due);
        before_states.insert(instance.clone(), name.to_string());
    }

    // "Reboots the device." — one task over both devices, so each device's own
    // finish instant is what its own cache must report.
    let created = ok(
        &create_task(
            &em,
            json!({ "targets": devices, "command": { "reboot": {} } }),
        )
        .await,
        row,
    );
    let task_id = created["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?}: expected a taskId, got {created}"))
        .to_string();

    // Everything the clock owes is due within the two execution spans plus the
    // instance transition the restart arms. The rows differ ONLY in what the caller
    // did while that passed: nothing at all, or polling.
    let settles_in =
        EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + INSTANCE_TRANSITION_SPAN + SETTLE_SLACK;
    match schedule {
        None => tokio::time::sleep(settles_in).await,
        Some(interval) => {
            let until = Instant::now() + settles_in;
            while Instant::now() < until {
                cache_of(&em, &devices[0], &held[0], row).await;
                tokio::time::sleep(interval).await;
            }
        }
    }

    let looked_at = seconds(SystemTime::now());
    let mut reported = Reported::new();
    for (device, ids) in devices.iter().zip(held.iter()) {
        let case = format!("{row}, {device}");
        let summaries = cache_of(&em, device, ids, &case).await;
        // The instant the device finished rebooting, as the device itself reports
        // it: "when the device last contacted the Amazon Web Services Cloud", which
        // the answer to the command is.
        let record = described_device(&em, device, &case).await;
        let finished = epoch(&record, &["lastReachedOutAt"], &case);
        assert!(
            finished > seconds(staged) && finished < looked_at - INSTANCE_TRANSITION_SPAN.as_secs_f64(),
            "case {case:?}: the reboot must have finished well before this look, got \
             a check-in at {finished} and a look at {looked_at}"
        );

        for instance in ids {
            let was = before_states
                .get(instance)
                .expect("every staged instance has a recorded state")
                .clone();
            let case = format!("{case}, {instance} was {was}");
            let summary = summaries
                .get(instance)
                .unwrap_or_else(|| panic!("case {case:?}: the cache still reports the instance"));
            assert_consistent(summary, &case);

            // The restart ended whatever was under way and brought back up what
            // would then be up; an instance it brought back up is owed RUNNING one
            // transition span after the restart, and the whole wait above has
            // passed, so that is owed too.
            let restarted_to = state_after_a_restart(&was);
            let (expected_state, expected_at) = if restarted_to == "PENDING" {
                ("RUNNING", finished + INSTANCE_TRANSITION_SPAN.as_secs_f64())
            } else {
                (restarted_to, finished)
            };
            let got_state = state_name(summary, &case);
            assert_eq!(
                got_state, expected_state,
                "case {case:?}: expected {expected_state:?} once the reboot and the \
                 restart it armed were both due, got {got_state:?}"
            );
            let refreshed = epoch(summary, &["lastUpdatedAt"], &case);
            assert!(
                (refreshed - expected_at).abs() < TOLERANCE,
                "case {case:?}: freshness reports the instant the change was owed \
                 (the reboot finished at {finished}), not the instant of the call \
                 that looked ({looked_at}), expected {expected_at} got {refreshed}"
            );
            let reported_created = epoch(summary, &["instance", "createdAt"], &case);
            assert!(
                (reported_created - seconds(created_at)).abs() < TOLERANCE,
                "case {case:?}: a restart does not re-create an instance, expected \
                 createdAt {} got {reported_created}",
                seconds(created_at)
            );

            let offset_ms = ((refreshed - finished) * 1000.0).round() as i64;
            reported.insert(instance.clone(), (got_state, offset_ms));
        }

        // The answer is materialised state and not a per-call computation: an
        // identical second call reports exactly the same entries.
        let again = cache_of(&em, device, ids, &case).await;
        assert_eq!(
            again, summaries,
            "case {case:?}: a settled cache is state, so a later identical call \
             reports it unchanged"
        );
    }

    // The task is terminal, so the whole reboot it stands for is behind us.
    let settled = ok(&describe_task(&em, &task_id).await, row);
    assert_eq!(
        settled["state"].as_str(),
        Some("COMPLETED"),
        "case {row:?}: the reboot must have finished for its effect to be settled, \
         got {settled}"
    );

    reported
}

#[tokio::test]
async fn a_reboots_cache_refresh_is_applied_when_the_reboot_finished() {
    let mut answers: Vec<(&str, Reported)> = Vec::new();
    for (row, schedule) in OBSERVATION_SCHEDULES {
        answers.push((row, reboot_and_read(row, *schedule).await));
    }

    // And the whole point: the two schedules read the SAME cache. Every entry's
    // state, and its freshness measured from its device's own finish instant, are
    // identical whether or not anybody looked while the reboot ran.
    let (first_row, first) = &answers[0];
    for (row, answer) in &answers[1..] {
        assert_eq!(
            answer, first,
            "case {row:?}: the cache a reboot produced must not depend on who called \
             or when, and it differs from case {first_row:?}"
        );
    }
}
