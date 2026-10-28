// SPDX-License-Identifier: CC0-1.0

//! WHEN a managed device's own record reports that its cloud-side cache was
//! refreshed, end to end over HTTP.
//!
//! The rules under test, from the two instants `DescribeDevice` reports — one says
//! "when the device last contacted the Amazon Web Services Cloud", the other "when
//! the device last pushed an update ... Indicates when the device cache was
//! refreshed":
//!
//! 1. `lastUpdatedAt` advances to the instant ANY part of the cloud-side record of
//!    that device changed — a member of the device itself, or an entry of the
//!    Amazon EC2 instance cache held under it — and it reports the instant of the
//!    push that changed it, never the instant of the call that looked;
//! 2. a check-in that left every part of that record as it stood advances
//!    `lastReachedOutAt` alone: the device is online without having pushed
//!    anything new;
//! 3. a push is a contact, so `lastUpdatedAt` never runs ahead of
//!    `lastReachedOutAt`.
//!
//! Every case goes through the router (`CreateTask`, `DescribeTask`,
//! `DescribeDevice`, `DescribeDeviceEc2Instances`), so an answer computed at the
//! calling instant rather than from persisted state cannot pass, and the instant
//! each row compares against is read over the wire too — the device's own check-in
//! instant — never from the test's own clock.
//!
//! Each case is a TABLE whose rows differ along what the rules generalise over:
//! which command the device performed and therefore which part of the record it
//! touched, how many entries that device's cache holds (including none at all), and
//! the state those entries were parked in, spanning resting states and states that
//! name work in flight.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, managed_device_id, EXECUTION_FINISH_SPAN, EXECUTION_START_SPAN,
};

use super::device_ec2_instance_cache::{
    assert_consistent, cache_of, call, create_task, describe_task, emulator, epoch, ok, seconds,
    stage_instance, Emulator, INSTANCE_STATES, TOLERANCE,
};

/// Slack added to the spans a command is settled over, so a loaded host cannot read
/// a record before the due instant it stores has passed.
const SETTLE_SLACK: Duration = Duration::from_millis(600);

/// The devices a row targets, each with how many instances the fixture runs on it.
/// The counts differ, and one device runs none, so the rules are checked over more
/// than one cache size — including the one where a reboot finds nothing to refresh.
const DEVICES: &[(u64, usize)] = &[(0, 4), (1, 2), (2, 0)];

/// The commands a task can give a device, each with whether performing it leaves a
/// member of the DEVICE record itself different: an `unlock` moves the device's
/// `UnlockState`, a `reboot` touches only the instance cache under it.
const COMMANDS: &[(&str, bool)] = &[("unlock", true), ("reboot", false)];

/// The `InstanceStateName` values a row parks the target's cache entries in before
/// the command runs, cycled over its entries. The first row is states an instance
/// RESTS in, the second states that name work in flight — the rules must not depend
/// on which.
const STAGED_SETS: &[(&str, &[&str])] = &[
    ("entries resting", &["STOPPED", "TERMINATED"]),
    ("entries with work in flight", &["PENDING", "STOPPING"]),
];

/// Whether `state_name` names work already under way, which is what arms an owed
/// instant on the entry. Read from this suite's own catalog of the documented
/// states, so a row stages an entry the way the device would have pushed it.
fn names_work_in_flight(state_name: &str) -> bool {
    INSTANCE_STATES
        .iter()
        .any(|(name, _, outcome)| *name == state_name && outcome.is_some())
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

/// The instant the record reports its cache was refreshed at.
fn refreshed_at(record: &Value, case: &str) -> f64 {
    epoch(record, &["lastUpdatedAt"], case)
}

/// The instant the record reports the device last contacted the Cloud at.
fn reached_out_at(record: &Value, case: &str) -> f64 {
    epoch(record, &["lastReachedOutAt"], case)
}

/// Park every entry of `device`'s cache in one of `states`, with its freshness at
/// `staged_at` and whatever it is owed held past the case's lifetime, so the only
/// thing that can move an entry during the case is the command the row gives.
fn stage_cache(em: &Emulator, device_index: u64, states: &[&str], staged_at: SystemTime) {
    let held = device_instance_ids(device_index);
    let held_far_out = SystemTime::now() + Duration::from_secs(3600);
    for (slot, instance) in held.iter().enumerate() {
        let state = states[slot % states.len()];
        let due = names_work_in_flight(state).then_some(held_far_out);
        stage_instance(
            em,
            &managed_device_id(device_index),
            instance,
            state,
            staged_at - Duration::from_secs(300),
            staged_at,
            due,
        );
    }
}

/// Give `device` one command and wait for the task it created to be terminal.
async fn perform(em: &Emulator, device: &str, command: &str, case: &str) {
    let created = ok(
        &create_task(
            em,
            json!({ "targets": [device], "command": { command: {} } }),
        )
        .await,
        case,
    );
    let task_id = created["taskId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a taskId, got {created}"))
        .to_string();
    tokio::time::sleep(EXECUTION_START_SPAN + EXECUTION_FINISH_SPAN + SETTLE_SLACK).await;
    let settled = ok(&describe_task(em, &task_id).await, case);
    assert_eq!(
        settled["state"].as_str(),
        Some("COMPLETED"),
        "case {case:?}: the command must have been performed for its effect on the \
         record to be settled, got {settled}"
    );
}

#[tokio::test]
async fn a_devices_refresh_instant_advances_exactly_when_its_cached_record_changed() {
    for (device_index, held_count) in DEVICES {
        for (command, changes_a_device_member) in COMMANDS {
            for (staging, states) in STAGED_SETS {
                let case =
                    format!("{command} on a device running {held_count} instances, {staging}");
                let em = emulator();
                let device = managed_device_id(*device_index);
                let held = device_instance_ids(*device_index);
                assert_eq!(
                    held.len(),
                    *held_count,
                    "case {case:?}: the inventory must run {held_count} instances on \
                     {device}, got {held:?}"
                );

                // The entries, pushed well before the case starts, so a freshness
                // that moved is unmistakably this command's doing.
                let staged_at = SystemTime::now() - Duration::from_secs(300);
                stage_cache(&em, *device_index, states, staged_at);

                let before = described_device(&em, &device, &case).await;
                let before_refreshed = refreshed_at(&before, &case);
                let before_reached = reached_out_at(&before, &case);

                perform(&em, &device, command, &case).await;

                let after = described_device(&em, &device, &case).await;
                let after_refreshed = refreshed_at(&after, &case);
                let after_reached = reached_out_at(&after, &case);

                // The device answered the command, so it contacted the Cloud; and a
                // push is a contact, so the refresh instant cannot sit after it.
                assert!(
                    after_reached > before_reached,
                    "case {case:?}: a device that answered the command contacted the \
                     Cloud, so lastReachedOutAt must advance past {before_reached}, got \
                     {after_reached}"
                );
                assert!(
                    after_refreshed <= after_reached + TOLERANCE,
                    "case {case:?}: a device that pushed an update contacted the Cloud \
                     to push it, so lastUpdatedAt ({after_refreshed}) cannot sit after \
                     lastReachedOutAt ({after_reached})"
                );

                // Whether any part of the cached record is different afterwards: the
                // device's own members for a command that moves one, and the instance
                // cache for a reboot of a device that holds any entry to refresh.
                let changed = *changes_a_device_member || *held_count > 0;
                if changed {
                    assert!(
                        after_refreshed > before_refreshed,
                        "case {case:?}: the {command} left the cached record different, \
                         so lastUpdatedAt must advance past {before_refreshed}, got \
                         {after_refreshed}"
                    );
                    assert!(
                        (after_refreshed - after_reached).abs() < TOLERANCE,
                        "case {case:?}: the record was refreshed by the push the device \
                         made when it answered the {command}, so lastUpdatedAt reports \
                         that instant ({after_reached}), got {after_refreshed}"
                    );
                } else {
                    assert!(
                        (after_refreshed - before_refreshed).abs() < TOLERANCE,
                        "case {case:?}: no part of the cached record changed, so \
                         lastUpdatedAt must still report {before_refreshed}, got \
                         {after_refreshed}"
                    );
                    assert!(
                        after_refreshed < after_reached - TOLERANCE,
                        "case {case:?}: the device checked in without pushing anything \
                         new, so its contact instant ({after_reached}) must sit after \
                         its refresh instant, got {after_refreshed}"
                    );
                }

                // And the cache the record speaks for agrees with it: a reboot pushed
                // every entry afresh at or after the record's refresh instant, while a
                // command that never restarted the device left the entries exactly as
                // they were pushed.
                if held.is_empty() {
                    continue;
                }
                let summaries = cache_of(&em, &device, &held, &case).await;
                for instance in &held {
                    let case = format!("{case}, {instance}");
                    let summary = summaries
                        .get(instance)
                        .unwrap_or_else(|| panic!("case {case:?}: the cache reports the entry"));
                    assert_consistent(summary, &case);
                    let entry_refreshed = epoch(summary, &["lastUpdatedAt"], &case);
                    if *command == "reboot" {
                        assert!(
                            entry_refreshed + TOLERANCE >= after_refreshed,
                            "case {case:?}: the entries a refresh produced are at least \
                             as fresh as the record reporting that refresh \
                             ({after_refreshed}), got {entry_refreshed}"
                        );
                    } else {
                        assert!(
                            (entry_refreshed - seconds(staged_at)).abs() < TOLERANCE,
                            "case {case:?}: a {command} does not refresh the instance \
                             cache, so the entry must still report {}, got \
                             {entry_refreshed}",
                            seconds(staged_at)
                        );
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn every_refresh_moves_the_instant_again_and_only_on_the_device_that_pushed() {
    let em = emulator();
    let case = "two reboots of one device, with a bystander";
    let target = managed_device_id(0);
    let bystander = managed_device_id(1);
    stage_cache(&em, 0, STAGED_SETS[0].1, SystemTime::now() - Duration::from_secs(300));

    let untouched = described_device(&em, &bystander, case).await;
    let mut refreshes = vec![refreshed_at(&described_device(&em, &target, case).await, case)];
    for round in 1..=2 {
        let case = format!("{case}, reboot {round}");
        perform(&em, &target, "reboot", &case).await;
        let record = described_device(&em, &target, &case).await;
        let refreshed = refreshed_at(&record, &case);
        let previous = *refreshes
            .last()
            .expect("the first instant was read before the first reboot");
        assert!(
            refreshed > previous,
            "case {case:?}: each refresh of the cache is its own push, so lastUpdatedAt \
             must advance past {previous}, got {refreshed}"
        );
        assert!(
            (refreshed - reached_out_at(&record, &case)).abs() < TOLERANCE,
            "case {case:?}: the refresh instant is the instant of the push, got \
             {refreshed} against a check-in at {}",
            reached_out_at(&record, &case)
        );
        refreshes.push(refreshed);
    }

    // The device no task targeted pushed nothing, so neither instant of its record
    // moved: these are per-device records and not one clock the whole inventory
    // shares.
    let still = described_device(&em, &bystander, case).await;
    assert!(
        (refreshed_at(&still, case) - refreshed_at(&untouched, case)).abs() < TOLERANCE
            && (reached_out_at(&still, case) - reached_out_at(&untouched, case)).abs() < TOLERANCE,
        "case {case:?}: a device no command reached must report the instants it had, \
         expected {untouched}, got {still}"
    );
}
