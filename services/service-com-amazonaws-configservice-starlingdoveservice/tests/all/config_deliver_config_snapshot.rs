// SPDX-License-Identifier: CC0-1.0

//! `DeliverConfigSnapshot`: scheduling a configuration-snapshot delivery, end to end
//! over the wire.
//!
//! The rule pinned here, over a table of inputs rather than one recorded case:
//! scheduling a snapshot delivery for a delivery channel this account holds, while a
//! customer managed configuration recorder is recording, answers with a snapshot id
//! distinct per call and records the delivery as THAT channel's last snapshot delivery
//! attempt — whose outcome, once Config has performed the delivery, is what the channel
//! reports — while leaving the channel's configuration and its periodic schedule,
//! anchored at the channel's creation, untouched; and the call is refused with the
//! modelled 400 whenever one of its three preconditions does not hold.
//!
//! Config is served here on its own, so the write a delivery makes into the channel's
//! bucket cannot land: what this suite pins is the FAILED outcome that earns, beside the
//! control plane. The delivered object and the successful outcome of a delivery into a
//! bucket that is there are `config_snapshot_delivery`'s, which mounts S3 beside Config.
//!
//! The harness (`Emulator`, `recorder_request`) is the one `config_setup_and_rule_loop`
//! builds, and the frequency table and timestamp readers are
//! `config_delivery_channel_status`'s; every case drives the emulator the way a client
//! does, through the `MultiServiceRouter`.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::SNAPSHOT_DELIVERY_SETTLE_AFTER;

use crate::config_delivery_channel_status::{
    channel_request, epoch_seconds, instant, DEFAULT_SNAPSHOT_INTERVAL, SNAPSHOT_FREQUENCIES,
};
use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// The name Config assigns a delivery channel (and a configuration recorder) put
/// without one, which is the channel every case here delivers through.
const DEFAULT_NAME: &str = "default";

/// The `lastStatus` a delivery whose write was refused reports.
///
/// RECORDED behaviour, pinned as a LITERAL here rather than read from the service, so
/// this assertion fails if the spelling regresses to the model enum's `Failure`
/// (`com.amazonaws.configservice#DeliveryStatus` spells its members `Success`, `Failure`,
/// `Not_Applicable`; the service answers them upper-cased).
const FAILURE: &str = "FAILURE";

/// How long after the scheduling call the snapshot delivery is performed, in seconds —
/// the span the emulator documents on
/// `crate::state::SNAPSHOT_DELIVERY_SETTLE_AFTER`, read from the service itself so
/// this suite cannot drift from it.
fn settle_seconds() -> f64 {
    SNAPSHOT_DELIVERY_SETTLE_AFTER.as_secs_f64()
}

/// Long enough after a scheduling call that the delivery it scheduled has been performed.
fn past_the_delivery() -> Duration {
    SNAPSHOT_DELIVERY_SETTLE_AFTER + Duration::from_millis(200)
}

/// What a case has set up before it schedules a snapshot delivery.
#[derive(Debug, Clone, Copy)]
enum Setup {
    /// A recorder, and no delivery channel at all.
    NoChannel,
    /// A delivery channel and a recorder that is recording: the state in which a
    /// snapshot delivery can be scheduled.
    Recording,
    /// A delivery channel whose configuration recorder has been deleted.
    NoRecorder,
    /// A delivery channel and a recorder that was never started.
    RecorderNeverStarted,
    /// A delivery channel and a recorder that was started and then stopped.
    RecorderStopped,
}

/// An emulator in the state `setup` describes, with the channel delivering snapshots
/// at `frequency` (the channel's own default when `None`).
async fn emulator_for(setup: Setup, frequency: Option<&str>) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;

    if matches!(setup, Setup::NoChannel) {
        // A recorder cannot be started without a delivery channel, so this setup is
        // exactly "a recorder and nothing to deliver through".
        return emulator;
    }

    emulator
        .ok("PutDeliveryChannel", &channel_request(frequency))
        .await;

    let start = json!({ "ConfigurationRecorderName": DEFAULT_NAME });
    match setup {
        Setup::NoChannel | Setup::RecorderNeverStarted => {}
        Setup::Recording => {
            emulator.ok("StartConfigurationRecorder", &start).await;
        }
        Setup::RecorderStopped => {
            emulator.ok("StartConfigurationRecorder", &start).await;
            emulator.ok("StopConfigurationRecorder", &start).await;
        }
        Setup::NoRecorder => {
            emulator.ok("StartConfigurationRecorder", &start).await;
            emulator.ok("StopConfigurationRecorder", &start).await;
            emulator.ok("DeleteConfigurationRecorder", &start).await;
        }
    }
    emulator
}

/// Schedule a snapshot delivery through `channel`, returning the `configSnapshotId`
/// and the wall-clock seconds read just before and just after the call.
async fn deliver(emulator: &Emulator, case: &str, channel: &str) -> (String, f64, f64) {
    let before = epoch_seconds();
    let response = emulator
        .ok(
            "DeliverConfigSnapshot",
            &json!({ "deliveryChannelName": channel }),
        )
        .await;
    let after = epoch_seconds();
    let id = response["configSnapshotId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry a configSnapshotId, got {response}")
        })
        .to_string();
    assert!(
        !id.is_empty(),
        "case {case:?}: the configSnapshotId must identify the snapshot being created, \
         got {id:?}"
    );
    (id, before, after)
}

/// The `configSnapshotDeliveryInfo` the emulator reports for the single delivery
/// channel, with the wall-clock seconds read just after the call.
async fn snapshot_info(emulator: &Emulator, case: &str) -> (Value, f64) {
    let response = emulator
        .ok("DescribeDeliveryChannelStatus", &json!({}))
        .await;
    let after = epoch_seconds();
    let reported = response["DeliveryChannelsStatus"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: DeliveryChannelsStatus must be a list, got {response}")
        });
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: one delivery channel exists, got {response}"
    );
    (reported[0]["configSnapshotDeliveryInfo"].clone(), after)
}

/// Every row of the frequency table: each published snapshot `deliveryFrequency` and
/// the interval it names, plus the channel that names none and so falls back to the
/// emulator's documented default.
fn frequency_cases() -> Vec<(String, Option<&'static str>, u64)> {
    let mut cases: Vec<(String, Option<&'static str>, u64)> = SNAPSHOT_FREQUENCIES
        .iter()
        .map(|(frequency, interval)| ((*frequency).to_string(), Some(*frequency), *interval))
        .collect();
    cases.push((
        "no configSnapshotDeliveryProperties".to_string(),
        None,
        DEFAULT_SNAPSHOT_INTERVAL,
    ));
    cases
}

#[tokio::test]
async fn a_scheduled_snapshot_is_recorded_as_the_channels_last_delivery_attempt() {
    for (case, frequency, interval) in frequency_cases() {
        let emulator = emulator_for(Setup::Recording, frequency).await;
        let channels_before = emulator.ok("DescribeDeliveryChannels", &json!({})).await;

        let (_, before, after) = deliver(&emulator, &case, DEFAULT_NAME).await;
        let (info, read_after) = snapshot_info(&emulator, &case).await;

        let attempt = instant(
            &case,
            "configSnapshotDeliveryInfo.lastAttemptTime",
            &info["lastAttemptTime"],
        );
        assert!(
            attempt >= before && attempt <= after,
            "case {case:?}: the delivery was attempted when it was scheduled, expected \
             an instant in [{before}, {after}], got {attempt}"
        );

        // A delivery that is still in flight has no status yet: `DeliveryStatus`
        // publishes no in-progress value. The guard keeps the assertion sound — it is
        // made only when the read itself finished before the delivery was due.
        if read_after < attempt + settle_seconds() {
            for member in ["lastStatus", "lastSuccessfulTime"] {
                assert!(
                    info.get(member).is_none(),
                    "case {case:?}: a snapshot delivery still in flight reports no \
                     {member}, got {info}"
                );
            }
        }

        // The periodic schedule is measured from the last delivery that COMPLETED, so
        // while this one is in flight the schedule the channel's creation anchors still
        // stands: still ahead, and still within one interval.
        let next = instant(
            &case,
            "configSnapshotDeliveryInfo.nextDeliveryTime",
            &info["nextDeliveryTime"],
        );
        assert!(
            next > read_after && next <= read_after + interval as f64,
            "case {case:?}: the next scheduled snapshot delivery stays within one \
             {interval}s interval after {read_after}, got {next}"
        );

        // Scheduling a delivery changes nothing about the channel itself.
        let channels_after = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
        assert_eq!(
            channels_before, channels_after,
            "case {case:?}: scheduling a snapshot delivery leaves the channel's \
             configuration alone, got {channels_before} then {channels_after}"
        );
    }
}

#[tokio::test]
async fn a_delivery_whose_write_fails_is_reported_as_a_failed_delivery() {
    // This emulator serves Config alone, so the channel's bucket is not there to be
    // written: the delivery is PERFORMED and refused, which is what the channel reports.
    // Rows differ along the channel's snapshot frequency: the delivery a CALL schedules
    // is performed on its own span, whatever schedule the channel carries.
    for (case, frequency, interval) in frequency_cases() {
        let emulator = emulator_for(Setup::Recording, frequency).await;
        // The schedule the channel's creation anchors, before any delivery is performed.
        let (anchored, _) = snapshot_info(&emulator, &case).await;
        let next_before = instant(
            &case,
            "configSnapshotDeliveryInfo.nextDeliveryTime",
            &anchored["nextDeliveryTime"],
        );
        let (_, before, after) = deliver(&emulator, &case, DEFAULT_NAME).await;
        tokio::time::sleep(past_the_delivery()).await;

        let (info, _) = snapshot_info(&emulator, &case).await;
        assert_eq!(
            info["lastStatus"],
            json!(FAILURE),
            "case {case:?}: a snapshot delivery whose write failed reports {FAILURE}, \
             got {info}"
        );
        for member in ["lastErrorCode", "lastErrorMessage"] {
            let reported = info[member].as_str().unwrap_or_default();
            assert!(
                !reported.is_empty(),
                "case {case:?}: a failed delivery reports why, expected a non-empty \
                 {member}, got {info}"
            );
        }
        let attempt = instant(
            &case,
            "configSnapshotDeliveryInfo.lastAttemptTime",
            &info["lastAttemptTime"],
        );
        assert!(
            attempt >= before && attempt <= after,
            "case {case:?}: the failed delivery keeps the instant it was attempted, \
             expected an instant in [{before}, {after}], got {attempt}"
        );

        // Nothing was delivered, so there is no successful delivery to report and the
        // next delivery is still the one the channel's creation anchored.
        assert!(
            info.get("lastSuccessfulTime").is_none(),
            "case {case:?}: a delivery that wrote nothing is no successful delivery, \
             got {info}"
        );
        let next = instant(
            &case,
            "configSnapshotDeliveryInfo.nextDeliveryTime",
            &info["nextDeliveryTime"],
        );
        assert!(
            (next - next_before).abs() < 0.01,
            "case {case:?}: a failed delivery leaves the {interval}s schedule the \
             channel's creation anchored, expected {next_before}, got {next}"
        );

        // Reading the status writes nothing, so a second read of a settled channel
        // reports exactly the same delivery.
        let (again, _) = snapshot_info(&emulator, &case).await;
        assert_eq!(
            info, again,
            "case {case:?}: reading the status changes nothing a later caller sees, got \
             {info} then {again}"
        );
    }
}

#[tokio::test]
async fn every_scheduled_delivery_gets_its_own_snapshot_id_and_supersedes_the_one_before() {
    // Rows differ along how many snapshot deliveries one channel is asked for.
    for scheduled in [1_usize, 2, 5] {
        let case = format!("{scheduled} deliveries");
        let emulator = emulator_for(Setup::Recording, Some("Six_Hours")).await;

        let mut ids: Vec<String> = Vec::new();
        let mut last_attempt_window = (0.0, 0.0);
        for _ in 0..scheduled {
            let (id, before, after) = deliver(&emulator, &case, DEFAULT_NAME).await;
            ids.push(id);
            last_attempt_window = (before, after);
        }

        let mut distinct = ids.clone();
        distinct.sort();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            ids.len(),
            "case {case:?}: every snapshot delivery gets its own id, got {ids:?}"
        );

        // The channel reports the LAST delivery scheduled for it, not the first.
        let (info, _) = snapshot_info(&emulator, &case).await;
        let attempt = instant(
            &case,
            "configSnapshotDeliveryInfo.lastAttemptTime",
            &info["lastAttemptTime"],
        );
        let (before, after) = last_attempt_window;
        assert!(
            attempt >= before && attempt <= after,
            "case {case:?}: the reported attempt is the last delivery scheduled, \
             expected an instant in [{before}, {after}], got {attempt}"
        );
    }
}

#[tokio::test]
async fn a_snapshot_delivery_is_refused_when_a_precondition_does_not_hold() {
    // One row per precondition the operation models, plus the channel names no channel
    // of this account carries (an absent name, a differently cased one and one with
    // trailing space): a channel is identified by its exact stored name.
    let cases: [(&str, Setup, &str, &str); 7] = [
        (
            "no delivery channel at all",
            Setup::NoChannel,
            DEFAULT_NAME,
            "NoSuchDeliveryChannelException",
        ),
        (
            "a name no channel carries",
            Setup::Recording,
            "absent",
            "NoSuchDeliveryChannelException",
        ),
        (
            "a differently cased name",
            Setup::Recording,
            "Default",
            "NoSuchDeliveryChannelException",
        ),
        (
            "a name with a trailing space",
            Setup::Recording,
            "default ",
            "NoSuchDeliveryChannelException",
        ),
        (
            "the configuration recorder was deleted",
            Setup::NoRecorder,
            DEFAULT_NAME,
            "NoAvailableConfigurationRecorderException",
        ),
        (
            "the recorder was never started",
            Setup::RecorderNeverStarted,
            DEFAULT_NAME,
            "NoRunningConfigurationRecorderException",
        ),
        (
            "the recorder was stopped",
            Setup::RecorderStopped,
            DEFAULT_NAME,
            "NoRunningConfigurationRecorderException",
        ),
    ];

    for (case, setup, channel, expected) in cases {
        let emulator = emulator_for(setup, Some("Twelve_Hours")).await;
        let (status, code) = emulator
            .refused(
                "DeliverConfigSnapshot",
                &json!({ "deliveryChannelName": channel }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, expected),
            "case {case:?}: expected HTTP 400 and {expected}, got {status} and {code:?}"
        );

        // A refused call schedules nothing, so the channel (where one exists) still
        // reports no delivery attempt.
        if !matches!(setup, Setup::NoChannel) {
            let (info, _) = snapshot_info(&emulator, case).await;
            assert!(
                info.get("lastAttemptTime").is_none(),
                "case {case:?}: a refused call records no delivery attempt, got {info}"
            );
        }
    }
}
