// SPDX-License-Identifier: CC0-1.0

//! What `DescribeDeliveryChannelStatus` reports for the delivery channel in state.
//!
//! The rule pinned here, over a table of inputs rather than one recorded case: the
//! status of a delivery channel is DERIVED from the periodic delivery schedule the
//! channel's creation anchors, so for every published snapshot `deliveryFrequency` the
//! reported `configSnapshotDeliveryInfo.nextDeliveryTime` falls strictly after the
//! call and no later than one frequency interval after it; the configuration-history
//! schedule is the fixed six-hour one; and a channel whose delivery has not happened
//! yet reports no attempt. Reading the status writes nothing, so two reads of an
//! untouched channel agree.
//!
//! The harness (`Emulator`, `recorder_request`) is the one
//! `config_setup_and_rule_loop` already builds; every case drives the emulator the way
//! a client does, through the `MultiServiceRouter`.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::config_setup_and_rule_loop::{recorder_request, Emulator, ROLE_ARN};

/// Every published `MaximumExecutionFrequency` value and the interval it names, from
/// `smithy_query(query="shape", shape="MaximumExecutionFrequency")` on this service's
/// model: `One_Hour`, `Three_Hours`, `Six_Hours`, `Twelve_Hours`, `TwentyFour_Hours`.
/// Each value spells its own number of hours between two snapshot deliveries.
const SNAPSHOT_FREQUENCIES: [(&str, u64); 5] = [
    ("One_Hour", 3_600),
    ("Three_Hours", 3 * 3_600),
    ("Six_Hours", 6 * 3_600),
    ("Twelve_Hours", 12 * 3_600),
    ("TwentyFour_Hours", 24 * 3_600),
];

/// The interval a channel that names no `deliveryFrequency` delivers snapshots at:
/// the emulator's documented default, the least frequent published value
/// (`crate::state::DEFAULT_SNAPSHOT_DELIVERY_FREQUENCY` is `TwentyFour_Hours`).
const DEFAULT_SNAPSHOT_INTERVAL: u64 = 24 * 3_600;

/// How often Config delivers a configuration history file (AWS Config Developer
/// Guide: a new configuration history file every six hours), the schedule the
/// emulator reports independently of the snapshot frequency.
const HISTORY_INTERVAL: u64 = 6 * 3_600;

/// The `DeliveryStatus` value a channel with SNS delivery turned off reports, from
/// `smithy_query(query="shape", shape="DeliveryStatus")`: `Success`, `Failure`,
/// `Not_Applicable`.
const NOT_APPLICABLE: &str = "Not_Applicable";

/// Wall-clock seconds since the epoch, the unit `awsJson1_1` puts a modelled
/// timestamp on the wire in.
fn epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch")
        .as_secs_f64()
}

/// `value` read as an epoch-seconds instant.
fn instant(case: &str, path: &str, value: &Value) -> f64 {
    value.as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: {path} must be an epoch-seconds timestamp, got {value}")
    })
}

/// The delivery channel body for `PutDeliveryChannel`, with the snapshot frequency
/// `frequency` when one is given.
fn channel_request(frequency: Option<&str>) -> Value {
    let mut channel = json!({ "name": "default", "s3BucketName": "config-bucket" });
    if let Some(frequency) = frequency {
        channel["configSnapshotDeliveryProperties"] =
            json!({ "deliveryFrequency": frequency });
    }
    json!({ "DeliveryChannel": channel })
}

/// An emulator holding a recorder and one delivery channel delivering snapshots at
/// `frequency` (the channel's own default when `None`).
async fn with_channel(frequency: Option<&str>) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    emulator
        .ok("PutDeliveryChannel", &channel_request(frequency))
        .await;
    emulator
}

/// The single `DeliveryChannelStatus` the emulator reports, with the wall-clock
/// seconds read just before and just after the call.
async fn status_of(emulator: &Emulator, body: &Value) -> (Value, f64, f64) {
    let before = epoch_seconds();
    let response = emulator.ok("DescribeDeliveryChannelStatus", body).await;
    let after = epoch_seconds();
    let reported = response["DeliveryChannelsStatus"]
        .as_array()
        .unwrap_or_else(|| panic!("DeliveryChannelsStatus must be a list, got {response}"));
    assert_eq!(
        reported.len(),
        1,
        "one delivery channel exists, got {response}"
    );
    (reported[0].clone(), before, after)
}

#[tokio::test]
async fn the_snapshot_frequency_bounds_the_next_delivery_time() {
    // One row per published `MaximumExecutionFrequency` value, plus the row that names
    // no frequency at all and so falls back to the channel's default interval.
    let mut cases: Vec<(String, Option<&str>, u64)> = SNAPSHOT_FREQUENCIES
        .iter()
        .map(|(frequency, interval)| ((*frequency).to_string(), Some(*frequency), *interval))
        .collect();
    cases.push((
        "no configSnapshotDeliveryProperties".to_string(),
        None,
        DEFAULT_SNAPSHOT_INTERVAL,
    ));

    for (case, frequency, interval) in cases {
        let emulator = with_channel(frequency).await;
        let (status, before, after) = status_of(&emulator, &json!({})).await;

        assert_eq!(
            status["name"], "default",
            "case {case:?}: the status names the stored channel, got {status}"
        );

        let snapshot = &status["configSnapshotDeliveryInfo"];
        let next = instant(
            &case,
            "configSnapshotDeliveryInfo.nextDeliveryTime",
            &snapshot["nextDeliveryTime"],
        );
        assert!(
            next > after,
            "case {case:?}: the next snapshot delivery is still ahead, expected more \
             than {after}, got {next}"
        );
        assert!(
            next <= after + interval as f64,
            "case {case:?}: the next snapshot delivery is at most one {interval}s \
             interval away, expected at most {}, got {next}",
            after + interval as f64
        );
        assert!(
            next > before + (interval as f64) / 2.0,
            "case {case:?}: the next snapshot delivery follows this channel's \
             frequency, expected more than {}, got {next}",
            before + (interval as f64) / 2.0
        );

        // Nothing has been delivered yet, so there is no attempt to report.
        for member in ["lastStatus", "lastAttemptTime", "lastSuccessfulTime"] {
            assert!(
                snapshot.get(member).is_none(),
                "case {case:?}: a channel whose first snapshot delivery is still due \
                 reports no {member}, got {snapshot}"
            );
        }

        // The configuration-history schedule is the fixed six-hour one, whatever the
        // snapshot frequency is.
        let history = &status["configHistoryDeliveryInfo"];
        let next_history = instant(
            &case,
            "configHistoryDeliveryInfo.nextDeliveryTime",
            &history["nextDeliveryTime"],
        );
        assert!(
            next_history > after && next_history <= after + HISTORY_INTERVAL as f64,
            "case {case:?}: the next history delivery falls within one {HISTORY_INTERVAL}s \
             interval after {after}, got {next_history}"
        );
    }
}

#[tokio::test]
async fn the_next_delivery_time_follows_the_frequency_the_channel_now_carries() {
    // Updating the channel's frequency re-derives its schedule: each row is read back
    // from the SAME channel after it was updated to that frequency.
    let emulator = with_channel(Some("One_Hour")).await;

    for (frequency, interval) in SNAPSHOT_FREQUENCIES {
        emulator
            .ok("PutDeliveryChannel", &channel_request(Some(frequency)))
            .await;
        let (status, _, after) = status_of(&emulator, &json!({})).await;
        let next = instant(
            frequency,
            "configSnapshotDeliveryInfo.nextDeliveryTime",
            &status["configSnapshotDeliveryInfo"]["nextDeliveryTime"],
        );
        assert!(
            next > after && next <= after + interval as f64,
            "case {frequency:?}: after the update the next snapshot delivery falls \
             within one {interval}s interval after {after}, got {next}"
        );
        // The frequency the channel now carries is the one it was updated to.
        let channels = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
        assert_eq!(
            channels["DeliveryChannels"][0]["configSnapshotDeliveryProperties"]
                ["deliveryFrequency"],
            json!(frequency),
            "case {frequency:?}: the updated frequency is persisted, got {channels}"
        );
    }
}

#[tokio::test]
async fn reading_the_status_changes_nothing_a_later_caller_sees() {
    let emulator = with_channel(Some("Six_Hours")).await;
    let (first, _, _) = status_of(&emulator, &json!({})).await;
    let (second, _, _) = status_of(&emulator, &json!({})).await;
    assert_eq!(
        first, second,
        "case \"two reads\": a status read writes nothing, so the second read reports \
         the same schedule; got {first} then {second}"
    );
}

#[tokio::test]
async fn the_stream_delivery_status_follows_whether_a_topic_is_configured() {
    // `ConfigStreamDeliveryInfo$lastStatus`: with SNS delivery turned off the last
    // status is `Not_Applicable`; with a topic configured nothing has been streamed,
    // so the emulator reports no stream status at all.
    let cases: [(&str, Option<&str>); 2] = [
        ("no snsTopicARN", None),
        (
            "snsTopicARN",
            Some("arn:aws:sns:us-east-1:000000000000:config-topic"),
        ),
    ];

    for (case, topic) in cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                "PutConfigurationRecorder",
                &recorder_request(json!({ "roleARN": ROLE_ARN })),
            )
            .await;
        let mut channel = json!({ "name": "default", "s3BucketName": "config-bucket" });
        if let Some(topic) = topic {
            channel["snsTopicARN"] = json!(topic);
        }
        emulator
            .ok("PutDeliveryChannel", &json!({ "DeliveryChannel": channel }))
            .await;

        let (status, _, _) = status_of(&emulator, &json!({})).await;
        let stream = status.get("configStreamDeliveryInfo");
        match topic {
            None => assert_eq!(
                stream.map(|info| info["lastStatus"].clone()),
                Some(json!(NOT_APPLICABLE)),
                "case {case:?}: SNS delivery is turned off, so the last stream status \
                 is {NOT_APPLICABLE}, got {status}"
            ),
            Some(_) => assert!(
                stream.is_none(),
                "case {case:?}: no notification has been streamed, so no stream status \
                 is reported, got {status}"
            ),
        }
    }
}

#[tokio::test]
async fn a_named_channel_is_selected_and_an_absent_one_is_refused() {
    let emulator = with_channel(Some("Twelve_Hours")).await;

    // Naming the stored channel selects it.
    let (status, _, after) = status_of(&emulator, &json!({ "DeliveryChannelNames": ["default"] }))
        .await;
    let next = instant(
        "named",
        "configSnapshotDeliveryInfo.nextDeliveryTime",
        &status["configSnapshotDeliveryInfo"]["nextDeliveryTime"],
    );
    assert!(
        next > after,
        "case \"named\": the named channel reports its own schedule, expected more \
         than {after}, got {next}"
    );

    // A name no channel carries is the modelled refusal.
    for case in ["absent", "Default", "default "] {
        let (code_status, code) = emulator
            .refused(
                "DescribeDeliveryChannelStatus",
                &json!({ "DeliveryChannelNames": [case] }),
            )
            .await;
        assert_eq!(
            (code_status, code.as_str()),
            (400, "NoSuchDeliveryChannelException"),
            "case {case:?}: a channel this account does not hold is refused with \
             NoSuchDeliveryChannelException, got {code_status} and {code:?}"
        );
    }
}

#[tokio::test]
async fn an_account_with_no_delivery_channel_reports_an_empty_list() {
    let emulator = Emulator::new();
    let response = emulator
        .ok("DescribeDeliveryChannelStatus", &json!({}))
        .await;
    assert_eq!(
        response["DeliveryChannelsStatus"],
        json!([]),
        "case \"no channel\": an account with no delivery channel reports an empty \
         list, got {response}"
    );
}
