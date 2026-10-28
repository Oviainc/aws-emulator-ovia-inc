// SPDX-License-Identifier: CC0-1.0

//! The account's shard and on-demand stream QUOTAS and the usage held against them, end to
//! end over the wire: `CreateStream` / `SplitShard` / `DeleteStream` / `UpdateStreamMode` ->
//! `DescribeLimits`.
//!
//! RULE: `DescribeLimits` reports, for the account and the region the request was signed
//! for, the quotas the service enforces beside the usage the store holds against them —
//! `OpenShardCount` is the number of OPEN shards summed over every stream that region
//! holds, `OnDemandStreamCount` is how many of those streams carry the on-demand capacity
//! mode, and `ShardLimit` / `OnDemandStreamCountLimit` are the ceilings past which a call
//! that would open another shard or give another stream that mode is refused with
//! `LimitExceededException`. It reports and never writes, so two calls with nothing between
//! them report the same usage, and a refused call leaves the usage exactly as it was.
//!
//! The recorded flows this was derived from call `describe_limits()` on an account holding
//! streams, which is one row of the tables below. The rows differ along what the rule
//! generalises over: how many streams the region holds, the shard count each was created
//! with, the capacity mode each carries, whether a shard is open or was closed by a
//! re-tiling, the lifecycle status the stream is in, the region the stream lives in, and
//! which side of the ceiling the call sits on.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    DELETION_DELAY, ON_DEMAND_INITIAL_SHARDS, STREAM_DELETING, STREAM_MODE_ON_DEMAND,
    STREAM_MODE_PROVISIONED,
};

use crate::shard_scaling::{hash_key_range, is_open, shard_id, shard_map, summary};
use crate::stream_tags::{error_code, past_activation, stream_arn, Emulator, REGION};

/// A second region, so a case can show that the usage reported is the usage of the region
/// the request was signed for and no other.
const OTHER_REGION: &str = "us-west-2";

/// How long a case waits before treating a deleted stream as gone: past the instant the
/// delete armed the `DELETING` -> gone transition for.
fn past_deletion() -> Duration {
    DELETION_DELAY + Duration::from_millis(400)
}

/// The quota-and-usage members `DescribeLimits` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Limits {
    shard_limit: i64,
    open_shard_count: i64,
    on_demand_stream_count: i64,
    on_demand_stream_count_limit: i64,
    channel_count: Option<i64>,
}

impl Limits {
    /// The response's members, where every member the model marks `@required` must be
    /// there: a `DescribeLimits` that answers without one answers nothing.
    fn of(case: &str, response: &Value) -> Self {
        let required = |member: &str| -> i64 {
            response
                .get(member)
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case}: DescribeLimits must carry {member}, got {response}")
                })
        };
        Self {
            shard_limit: required("ShardLimit"),
            open_shard_count: required("OpenShardCount"),
            on_demand_stream_count: required("OnDemandStreamCount"),
            on_demand_stream_count_limit: required("OnDemandStreamCountLimit"),
            channel_count: response.get("ChannelCount").and_then(Value::as_i64),
        }
    }
}

/// What `DescribeLimits` reports in `region`.
async fn limits_in(emulator: &Emulator, case: &str, region: &str) -> Limits {
    let response = emulator
        .ok_in(case, region, "DescribeLimits", &json!({}))
        .await;
    let limits = Limits::of(case, &response);
    // Nothing creates a channel in this emulator, so a channel count it reported could only
    // be zero.
    assert!(
        limits.channel_count.is_none_or(|count| count == 0),
        "case {case}: expected no channels to be counted, got {response}"
    );
    limits
}

/// [`limits_in`] in the default test region.
async fn limits(emulator: &Emulator, case: &str) -> Limits {
    limits_in(emulator, case, REGION).await
}

/// Create the stream `name` in `region`, in `mode`, with the shard count a provisioned
/// stream's request names (an on-demand stream's is the service's to manage).
async fn create_in(
    emulator: &Emulator,
    case: &str,
    region: &str,
    name: &str,
    mode: &str,
    shards: Option<u32>,
) {
    let mut body = json!({
        "StreamName": name,
        "StreamModeDetails": { "StreamMode": mode },
    });
    if let Some(shards) = shards {
        body["ShardCount"] = json!(shards);
    }
    emulator.ok_in(case, region, "CreateStream", &body).await;
}

/// [`create_in`] in the default test region, under the case's own name.
async fn create(emulator: &Emulator, case: &str, mode: &str, shards: Option<u32>) {
    create_in(emulator, case, REGION, case, mode, shards).await;
}

/// The open shards of every stream named, summed from what each stream's OWN read reports:
/// the figure `DescribeLimits` has to agree with.
async fn open_shards_of(emulator: &Emulator, case: &str, names: &[String]) -> i64 {
    let mut open = 0i64;
    for name in names {
        let (_, stream_open) = summary(emulator, case, name).await;
        open += i64::try_from(stream_open).unwrap_or(i64::MAX);
    }
    open
}

/// The usage reported is the usage every stream's own read reports, whatever the region
/// holds: one stream or several, of any shard count, in either capacity mode.
#[tokio::test]
async fn reported_usage_is_the_usage_the_streams_of_the_region_report() {
    // (the stream's name, its capacity mode, the shard count its create names)
    let cases: Vec<(&str, &str, Option<u32>)> = vec![
        ("limits-one-shard", STREAM_MODE_PROVISIONED, Some(1)),
        ("limits-three-shards", STREAM_MODE_PROVISIONED, Some(3)),
        ("limits-on-demand", STREAM_MODE_ON_DEMAND, None),
        ("limits-ten-shards", STREAM_MODE_PROVISIONED, Some(10)),
        ("limits-second-on-demand", STREAM_MODE_ON_DEMAND, None),
    ];

    let emulator = Emulator::new();

    // An account holding nothing has no usage, and its quotas are already there to report.
    let empty = limits(&emulator, "limits-empty").await;
    assert_eq!(
        empty.open_shard_count, 0,
        "case limits-empty: expected no open shards before anything is created, got {empty:?}"
    );
    assert_eq!(
        empty.on_demand_stream_count, 0,
        "case limits-empty: expected no on-demand streams before anything is created, got \
         {empty:?}"
    );
    assert!(
        empty.shard_limit > 0 && empty.on_demand_stream_count_limit > 0,
        "case limits-empty: expected the quotas to be reported for an empty account, got \
         {empty:?}"
    );

    let mut held: Vec<String> = Vec::new();
    let mut on_demand = 0i64;
    for (name, mode, shards) in &cases {
        create(&emulator, name, mode, *shards).await;
        held.push((*name).to_string());
        if *mode == STREAM_MODE_ON_DEMAND {
            on_demand += 1;
        }

        // A stream counts from the moment it is created, whatever lifecycle status it is
        // in, so the usage is asserted BEFORE the create's transition falls due and again
        // after it.
        let expected_open = open_shards_of(&emulator, name, &held).await;
        let reported = limits(&emulator, name).await;
        assert_eq!(
            reported.open_shard_count, expected_open,
            "case {name}: expected the {} open shards the {} streams report, got {reported:?}",
            expected_open,
            held.len()
        );
        assert_eq!(
            reported.on_demand_stream_count, on_demand,
            "case {name}: expected {on_demand} on-demand streams, got {reported:?}"
        );
        assert!(
            reported.shard_limit >= reported.open_shard_count,
            "case {name}: expected the usage to sit under the quota, got {reported:?}"
        );
        assert!(
            reported.on_demand_stream_count_limit >= reported.on_demand_stream_count,
            "case {name}: expected the on-demand usage to sit under the quota, got {reported:?}"
        );

        // The call reports and writes nothing, so repeating it reports the same usage.
        let again = limits(&emulator, name).await;
        assert_eq!(
            again, reported,
            "case {name}: expected a second read to report the same limits, got {again:?}"
        );
    }

    tokio::time::sleep(past_activation()).await;
    let expected_open = open_shards_of(&emulator, "limits-settled", &held).await;
    let settled = limits(&emulator, "limits-settled").await;
    assert_eq!(
        settled.open_shard_count, expected_open,
        "case limits-settled: expected the usage to be unchanged once every create fell \
         due, got {settled:?}"
    );
    assert_eq!(
        settled.on_demand_stream_count, on_demand,
        "case limits-settled: expected {on_demand} on-demand streams once every create fell \
         due, got {settled:?}"
    );
}

/// A shard a re-tiling closed is still stored, and provides no capacity: it is the OPEN
/// shards that are the account's usage.
#[tokio::test]
async fn a_closed_shard_is_not_counted_as_open_capacity() {
    let emulator = Emulator::new();
    let case = "limits-closed-shard";
    create(&emulator, case, STREAM_MODE_PROVISIONED, Some(2)).await;
    tokio::time::sleep(past_activation()).await;

    let before = limits(&emulator, case).await;
    assert_eq!(
        before.open_shard_count, 2,
        "case {case}: expected the two shards the create laid down, got {before:?}"
    );

    // `SplitShard` closes the shard it names and lays down two open children, so the stream
    // stores one shard MORE than before while holding one more OPEN shard.
    let map = shard_map(&emulator, case, case).await;
    let parent = map
        .first()
        .unwrap_or_else(|| panic!("case {case}: the stream must hold a shard to split"));
    let parent_id = shard_id(case, parent);
    let (starting, ending) = hash_key_range(case, parent);
    emulator
        .ok(
            case,
            "SplitShard",
            &json!({
                "StreamName": case,
                "ShardToSplit": parent_id,
                "NewStartingHashKey": (starting + (ending - starting) / 2).to_string(),
            }),
        )
        .await;

    let stored = shard_map(&emulator, case, case).await;
    let open = stored.iter().filter(|shard| is_open(case, shard)).count();
    assert_eq!(
        (stored.len(), open),
        (4, 3),
        "case {case}: expected the split to leave 3 open shards of 4 stored, got {stored:?}"
    );
    let after = limits(&emulator, case).await;
    assert_eq!(
        after.open_shard_count,
        i64::try_from(open).unwrap_or_default(),
        "case {case}: expected the {open} open shards of the {} stored ones, got {after:?}",
        stored.len()
    );
}

/// The usage reported is the usage of the region the request was signed for: a stream of
/// another region counts against that region's quotas, not this one's.
#[tokio::test]
async fn reported_usage_is_scoped_to_the_region_the_request_is_signed_for() {
    let emulator = Emulator::new();
    let case = "limits-per-region";

    // "two streams in the same Amazon Web Services account but in two different Regions can
    // also have the same name" (`CreateStreamInput$StreamName`), so one name is used in both.
    create_in(
        &emulator,
        case,
        REGION,
        case,
        STREAM_MODE_PROVISIONED,
        Some(2),
    )
    .await;
    create_in(
        &emulator,
        case,
        OTHER_REGION,
        case,
        STREAM_MODE_PROVISIONED,
        Some(5),
    )
    .await;
    create_in(
        &emulator,
        case,
        OTHER_REGION,
        "limits-per-region-on-demand",
        STREAM_MODE_ON_DEMAND,
        None,
    )
    .await;

    let home = limits_in(&emulator, case, REGION).await;
    assert_eq!(
        (home.open_shard_count, home.on_demand_stream_count),
        (2, 0),
        "case {case}: expected only the stream {REGION} holds to be counted, got {home:?}"
    );

    // The on-demand stream's shard count is the service's to manage, and an on-demand
    // stream starts with the shards its initial capacity provides.
    let away = limits_in(&emulator, case, OTHER_REGION).await;
    assert_eq!(
        (away.open_shard_count, away.on_demand_stream_count),
        (5 + i64::from(ON_DEMAND_INITIAL_SHARDS), 1),
        "case {case}: expected only the streams {OTHER_REGION} holds to be counted, got \
         {away:?}"
    );
    assert_eq!(
        away.shard_limit, home.shard_limit,
        "case {case}: expected the same quota in both regions, got {away:?} beside {home:?}"
    );
}

/// A stream a delete armed is still held, in `DELETING`, so its shards are still the
/// account's usage — and stop being counted once the removal falls due.
#[tokio::test]
async fn a_removed_stream_stops_counting_once_its_removal_falls_due() {
    let emulator = Emulator::new();
    let case = "limits-removed";
    create(&emulator, case, STREAM_MODE_ON_DEMAND, None).await;
    tokio::time::sleep(past_activation()).await;

    let before = limits(&emulator, case).await;
    assert_eq!(
        (before.open_shard_count, before.on_demand_stream_count),
        (i64::from(ON_DEMAND_INITIAL_SHARDS), 1),
        "case {case}: expected the on-demand stream's shards to be counted, got {before:?}"
    );

    emulator
        .ok(case, "DeleteStream", &json!({ "StreamName": case }))
        .await;
    let (status, _) = summary(&emulator, case, case).await;
    assert_eq!(
        status, STREAM_DELETING,
        "case {case}: expected the stream to be {STREAM_DELETING} straight after the \
         delete, got {status}"
    );
    let during = limits(&emulator, case).await;
    assert_eq!(
        during, before,
        "case {case}: expected a stream the account still holds to still be counted, got \
         {during:?}"
    );

    tokio::time::sleep(past_deletion()).await;
    let after = limits(&emulator, case).await;
    assert_eq!(
        (after.open_shard_count, after.on_demand_stream_count),
        (0, 0),
        "case {case}: expected the removed stream to count towards nothing, got {after:?}"
    );
    assert_eq!(
        after.shard_limit, before.shard_limit,
        "case {case}: expected the quota to be unchanged by what was deleted, got {after:?}"
    );
}

/// The `ShardLimit` reported is the ceiling the service enforces: a create that would open
/// a shard past it is refused, and changes nothing.
#[tokio::test]
async fn the_reported_shard_limit_is_the_ceiling_a_create_is_refused_past() {
    let emulator = Emulator::new();
    let case = "limits-shard-ceiling";
    let quota = limits(&emulator, case).await.shard_limit;
    assert!(
        quota > 0,
        "case {case}: expected a positive ShardLimit, got {quota}"
    );

    // Fill the region to EXACTLY the quota the call above reported. There is no second,
    // per-stream ceiling, so the whole quota fits in one stream — `shard_quota.rs` asserts
    // that boundary over a table of usages.
    create(
        &emulator,
        &format!("{case}-0"),
        STREAM_MODE_PROVISIONED,
        Some(u32::try_from(quota).unwrap_or(1)),
    )
    .await;

    let full = limits(&emulator, case).await;
    assert_eq!(
        full.open_shard_count, quota,
        "case {case}: expected the region to sit at its quota of {quota} open shards, got \
         {full:?}"
    );

    let (status, response) = emulator
        .call(
            "CreateStream",
            &json!({ "StreamName": format!("{case}-over"), "ShardCount": 1 }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a create past the quota to be refused, got {status} with \
         body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}: expected LimitExceededException, got {response}"
    );

    let after = limits(&emulator, case).await;
    assert_eq!(
        after, full,
        "case {case}: expected the refused create to leave the usage as it was, got {after:?}"
    );
}

/// The `OnDemandStreamCountLimit` reported is the ceiling the service enforces: a create and
/// a mode change that would give another stream that mode past it are refused, while a
/// stream that already carries it is counted already and so is served.
#[tokio::test]
async fn the_reported_on_demand_stream_limit_is_the_ceiling_a_change_is_refused_past() {
    let emulator = Emulator::new();
    let case = "limits-on-demand-ceiling";
    let quota = limits(&emulator, case).await.on_demand_stream_count_limit;
    assert!(
        quota > 0,
        "case {case}: expected a positive OnDemandStreamCountLimit, got {quota}"
    );

    for index in 0..quota {
        create(
            &emulator,
            &format!("{case}-{index}"),
            STREAM_MODE_ON_DEMAND,
            None,
        )
        .await;
    }
    let provisioned = format!("{case}-provisioned");
    create_in(
        &emulator,
        case,
        REGION,
        &provisioned,
        STREAM_MODE_PROVISIONED,
        Some(1),
    )
    .await;
    tokio::time::sleep(past_activation()).await;

    let full = limits(&emulator, case).await;
    assert_eq!(
        full.on_demand_stream_count, quota,
        "case {case}: expected the region to sit at its quota of {quota} on-demand streams, \
         got {full:?}"
    );

    let (status, response) = emulator
        .call(
            "CreateStream",
            &json!({
                "StreamName": format!("{case}-over"),
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a create past the on-demand quota to be refused, got {status} \
         with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}: expected LimitExceededException, got {response}"
    );

    let (status, response) = emulator
        .call(
            "UpdateStreamMode",
            &json!({
                "StreamARN": stream_arn(REGION, &provisioned),
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case}: expected a mode change past the on-demand quota to be refused, got \
         {status} with body {response}"
    );
    assert_eq!(
        error_code(&response),
        "LimitExceededException",
        "case {case}: expected LimitExceededException, got {response}"
    );

    // A stream that already carries the mode is counted already, so re-stating it takes the
    // region past nothing.
    emulator
        .ok(
            case,
            "UpdateStreamMode",
            &json!({
                "StreamARN": stream_arn(REGION, &format!("{case}-0")),
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;

    let after = limits(&emulator, case).await;
    assert_eq!(
        after, full,
        "case {case}: expected the refused calls to leave the usage as it was, got {after:?}"
    );
}
