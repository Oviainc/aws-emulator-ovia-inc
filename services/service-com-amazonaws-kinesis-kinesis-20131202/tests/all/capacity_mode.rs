// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's CAPACITY MODE, end to end over the wire: `CreateStream` ->
//! `UpdateStreamMode` -> `DescribeStreamSummary` / `ListShards`.
//!
//! RULE: `UpdateStreamMode` stores the `StreamModeDetails` its request carries on the stream
//! its `StreamARN` names, so that is the capacity mode every later read of that stream
//! reports, and leaves the stream `UPDATING` until the transition back to `ACTIVE` falls due.
//! It is served only while the stream is `ACTIVE`, so a call that overlaps one still in
//! flight is refused with `ResourceInUseException` and changes nothing: of any number of
//! overlapping calls on one stream, EXACTLY ONE is accepted and the stream settles on the
//! mode that one named. A request naming a stream this account and region do not hold is
//! `ResourceNotFoundException`, and one carrying a member the target mode does not admit is
//! `InvalidArgumentException`.
//!
//! The recorded flow this was derived from issues two overlapping calls for the on-demand
//! mode on one provisioned stream, which is one row of the tables below. The rows differ
//! along what the rule generalises over: the mode the stream is in when the call arrives,
//! the mode it is taken to (including the one it already has), how many calls overlap, and
//! the status the stream is in when the call arrives.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, MODE_UPDATE_DELAY, STREAM_ACTIVE, STREAM_MODE_ON_DEMAND,
    STREAM_MODE_PROVISIONED, STREAM_UPDATING,
};
use std::time::Duration;

use crate::shard_scaling::{is_open, shard_id, shard_map, summary};
use crate::stream_tags::{error_code, error_message, past_activation, stream_arn, Emulator, REGION};

/// How long a case waits before treating a stream whose mode changed as ready: past the
/// instant the change armed the `UPDATING` -> `ACTIVE` transition for.
fn past_mode_update() -> Duration {
    MODE_UPDATE_DELAY + Duration::from_millis(400)
}

/// An `UpdateStreamMode` body taking the stream `name` to `mode`.
fn change_to(name: &str, mode: &str) -> Value {
    json!({
        "StreamARN": stream_arn(REGION, name),
        "StreamModeDetails": { "StreamMode": mode },
    })
}

/// The `StreamMode` `DescribeStreamSummary` reports for `name`.
async fn reported_mode(emulator: &Emulator, case: &str, name: &str) -> String {
    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": name }))
        .await;
    described["StreamDescriptionSummary"]
        .get("StreamModeDetails")
        .and_then(|details| details.get("StreamMode"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: the summary must carry StreamModeDetails, got {described}")
        })
        .to_string()
}

/// The ids of the stream's open shards, in map order.
async fn open_shard_ids(emulator: &Emulator, case: &str, name: &str) -> Vec<String> {
    shard_map(emulator, case, name)
        .await
        .iter()
        .filter(|shard| is_open(case, shard))
        .map(|shard| shard_id(case, shard))
        .collect()
}

/// An accepted change is what every later read of the stream reports, and it leaves the
/// stream's shard map alone.
#[tokio::test]
async fn an_accepted_mode_change_is_what_a_read_reports() {
    // (the mode the stream is created in, the mode the call takes it to)
    let cases: Vec<(&str, &str)> = vec![
        (STREAM_MODE_PROVISIONED, STREAM_MODE_ON_DEMAND),
        (STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED),
        (STREAM_MODE_PROVISIONED, STREAM_MODE_PROVISIONED),
        (STREAM_MODE_ON_DEMAND, STREAM_MODE_ON_DEMAND),
    ];
    let name_of = |from: &str, to: &str| format!("mode-{from}-to-{to}").to_lowercase();

    let emulator = Emulator::new();
    for (from, to) in &cases {
        let name = name_of(from, to);
        let mut body = json!({
            "StreamName": name,
            "StreamModeDetails": { "StreamMode": from },
        });
        if *from == STREAM_MODE_PROVISIONED {
            body["ShardCount"] = json!(2);
        }
        emulator.ok(&name, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for (from, to) in &cases {
        let case = name_of(from, to);
        let before = open_shard_ids(&emulator, &case, &case).await;
        assert_eq!(
            reported_mode(&emulator, &case, &case).await,
            *from,
            "case {case}: expected the stream to start in {from}"
        );

        emulator
            .ok(&case, "UpdateStreamMode", &change_to(&case, to))
            .await;

        // The change is asynchronous, so the stream is UPDATING straight after the call and
        // already reports the mode the call stored.
        let (status, _) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_UPDATING,
            "case {case}: expected the stream to be {STREAM_UPDATING} straight after the \
             change, got {status}"
        );
        let reported = reported_mode(&emulator, &case, &case).await;
        assert_eq!(
            reported, *to,
            "case {case}: expected the stream to report {to}, got {reported}"
        );

        // The capacity mode is what changed: the shards the stream already held are still
        // its open shards, so a reader's positions survive the change.
        let after = open_shard_ids(&emulator, &case, &case).await;
        assert_eq!(
            after, before,
            "case {case}: expected the shard map to be left as it was, which held {before:?}"
        );
    }

    tokio::time::sleep(past_mode_update()).await;
    for (from, to) in &cases {
        let case = name_of(from, to);
        let (status, _) = summary(&emulator, &case, &case).await;
        assert_eq!(
            status, STREAM_ACTIVE,
            "case {case}: expected the stream to be {STREAM_ACTIVE} once the change fell \
             due, got {status}"
        );
        let reported = reported_mode(&emulator, &case, &case).await;
        assert_eq!(
            reported, *to,
            "case {case}: expected the stream to still report {to} once the change fell \
             due, got {reported}"
        );
    }
}

/// One `ACTIVE` provisioned stream of one shard, named `case`.
async fn provisioned_stream(emulator: &Emulator, case: &str) {
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": case,
                "ShardCount": 1,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
            }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
}

/// Assert that exactly ONE of the overlapping calls was accepted and every other was refused
/// because the stream was no longer `ACTIVE` while the accepted one was in flight.
fn assert_one_winner(case: &str, answers: &[(u16, Value)]) {
    let accepted = answers.iter().filter(|(status, _)| *status == 200).count();
    assert_eq!(
        accepted, 1,
        "case {case}: expected exactly one winner among {} overlapping calls, got {answers:?}",
        answers.len()
    );
    for (status, response) in answers.iter().filter(|(status, _)| *status != 200) {
        assert_eq!(
            *status, 400,
            "case {case}: expected the refused calls to answer 400, got {status} with body \
             {response}"
        );
        assert_eq!(
            error_code(response),
            "ResourceInUseException",
            "case {case}: expected the loser to be refused because the stream is in use, \
             got {response}"
        );
        let message = error_message(response);
        assert!(
            message.contains(STREAM_UPDATING),
            "case {case}: expected the message to name the status that refused the call, \
             got {message:?}"
        );
    }
}

/// Assert that the stream settles `ACTIVE` on `mode` once the change the winner armed falls
/// due.
async fn assert_settled_on(emulator: &Emulator, case: &str, mode: &str) {
    tokio::time::sleep(past_mode_update()).await;
    let (status, _) = summary(emulator, case, case).await;
    assert_eq!(
        status, STREAM_ACTIVE,
        "case {case}: expected the stream to settle {STREAM_ACTIVE}, got {status}"
    );
    let reported = reported_mode(emulator, case, case).await;
    assert_eq!(
        reported, mode,
        "case {case}: expected the stream to settle on {mode}, got {reported}"
    );
}

/// The recorded race: TWO overlapping calls, of which exactly one is accepted.
#[tokio::test]
async fn two_overlapping_mode_changes_admit_exactly_one() {
    let emulator = Emulator::new();
    let case = "overlap-two";
    provisioned_stream(&emulator, case).await;

    // The calls are polled CONCURRENTLY, which is what two SDK clients racing each other
    // does to one stream.
    let body = change_to(case, STREAM_MODE_ON_DEMAND);
    let (first, second) = tokio::join!(
        emulator.call("UpdateStreamMode", &body),
        emulator.call("UpdateStreamMode", &body),
    );
    assert_one_winner(case, &[first, second]);
    assert_settled_on(&emulator, case, STREAM_MODE_ON_DEMAND).await;
}

/// The same rule for MORE overlapping calls: one winner, and every other caller is told the
/// stream is in use.
#[tokio::test]
async fn five_overlapping_mode_changes_admit_exactly_one() {
    let emulator = Emulator::new();
    let case = "overlap-five";
    provisioned_stream(&emulator, case).await;

    let body = change_to(case, STREAM_MODE_ON_DEMAND);
    let (first, second, third, fourth, fifth) = tokio::join!(
        emulator.call("UpdateStreamMode", &body),
        emulator.call("UpdateStreamMode", &body),
        emulator.call("UpdateStreamMode", &body),
        emulator.call("UpdateStreamMode", &body),
        emulator.call("UpdateStreamMode", &body),
    );
    assert_one_winner(case, &[first, second, third, fourth, fifth]);
    assert_settled_on(&emulator, case, STREAM_MODE_ON_DEMAND).await;
}

/// A stream taken to the provisioned mode drops the members only an on-demand stream
/// carries, so it reports no warm throughput configuration at all.
#[tokio::test]
async fn a_change_to_provisioned_drops_the_on_demand_only_members() {
    let emulator = Emulator::new();
    let case = "mode-on-demand-members";
    emulator
        .ok(
            case,
            "CreateStream",
            &json!({
                "StreamName": case,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                "WarmThroughputMiBps": 8,
                "RecordDistributionStrategy": "AUTO",
            }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": case }))
        .await;
    let summary = &described["StreamDescriptionSummary"];
    assert_eq!(
        summary
            .get("WarmThroughput")
            .and_then(|throughput| throughput.get("TargetMiBps"))
            .and_then(Value::as_i64),
        Some(8),
        "case {case}: expected the on-demand stream to report the target it was created \
         with, got {described}"
    );
    assert_eq!(
        summary
            .get("RecordDistributionStrategy")
            .and_then(Value::as_str),
        Some("AUTO"),
        "case {case}: expected the on-demand stream to report its distribution strategy, \
         got {described}"
    );

    emulator
        .ok(
            case,
            "UpdateStreamMode",
            &change_to(case, STREAM_MODE_PROVISIONED),
        )
        .await;
    tokio::time::sleep(past_mode_update()).await;

    let described = emulator
        .ok(case, "DescribeStreamSummary", &json!({ "StreamName": case }))
        .await;
    let summary = &described["StreamDescriptionSummary"];
    // "Represents the warm throughput configuration on the stream. This is only present for
    // On-Demand Kinesis Data Streams" (`WarmThroughputObject`), so the stream carries none
    // once its shard map is its owner's to size.
    assert!(
        summary.get("WarmThroughput").is_none_or(Value::is_null),
        "case {case}: expected the provisioned stream to carry no warm throughput \
         configuration, got {described}"
    );
    assert!(
        summary
            .get("RecordDistributionStrategy")
            .is_none_or(Value::is_null),
        "case {case}: expected the provisioned stream to carry no distribution strategy, \
         got {described}"
    );
}

/// A refused change answers the modelled client error and leaves the stored mode as it was.
#[tokio::test]
async fn a_refused_mode_change_changes_nothing() {
    let emulator = Emulator::new();

    // A stream whose create has not fallen due yet: its mode is nothing's to change while
    // the transition the create armed is still in flight.
    let creating = "mode-still-creating";
    emulator
        .ok(
            creating,
            "CreateStream",
            &json!({ "StreamName": creating, "ShardCount": 1 }),
        )
        .await;
    let (status, response) = emulator
        .call(
            "UpdateStreamMode",
            &change_to(creating, STREAM_MODE_ON_DEMAND),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {creating}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceInUseException",
        "case {creating}: expected ResourceInUseException, got {response}"
    );
    tokio::time::sleep(past_activation()).await;
    let reported = reported_mode(&emulator, creating, creating).await;
    assert_eq!(
        reported, STREAM_MODE_PROVISIONED,
        "case {creating}: expected the refused change to leave the mode as it was, got \
         {reported}"
    );

    // A `StreamARN` this account and region hold no stream under.
    let absent = stream_arn(REGION, "mode-no-such-stream");
    let (status, response) = emulator
        .call(
            "UpdateStreamMode",
            &json!({
                "StreamARN": absent,
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case absent: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "case absent: expected ResourceNotFoundException, got {response}"
    );
    let message = error_message(&response);
    let says = stream_not_found_message(&absent);
    assert!(
        message.contains(&says),
        "case absent: expected the message to name {says:?}, got {message:?}"
    );

    // "This field is only valid when the stream mode is being updated to on-demand"
    // (`UpdateStreamModeInput$WarmThroughputMiBps`).
    let provisioned = "mode-warm-throughput";
    emulator
        .ok(
            provisioned,
            "CreateStream",
            &json!({ "StreamName": provisioned, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let mut body = change_to(provisioned, STREAM_MODE_PROVISIONED);
    body["WarmThroughputMiBps"] = json!(8);
    let (status, response) = emulator.call("UpdateStreamMode", &body).await;
    assert_eq!(
        status, 400,
        "case {provisioned}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "InvalidArgumentException",
        "case {provisioned}: expected InvalidArgumentException, got {response}"
    );
    let (reported_status, _) = summary(&emulator, provisioned, provisioned).await;
    assert_eq!(
        reported_status, STREAM_ACTIVE,
        "case {provisioned}: a refused change must leave the stream {STREAM_ACTIVE}, got \
         {reported_status}"
    );
}
