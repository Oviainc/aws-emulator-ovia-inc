// SPDX-License-Identifier: CC0-1.0

//! The account and region SHARD QUOTA, end to end over the wire: `DescribeLimits` ->
//! `CreateStream` / `UpdateShardCount` / `SplitShard` / `MergeShards`.
//!
//! RULE: the shard quota `DescribeLimits` publishes for the account and the region a request
//! is signed for is the ONE ceiling every call that lays down a shard is measured against —
//! a call that would lay down at most `ShardLimit - OpenShardCount` more shards is served,
//! whatever stream it addresses and however many shards that one stream then holds, and a
//! call for more is refused with HTTP 400 `LimitExceededException` whose message names the
//! account, the region, the open shard count and the limit `DescribeLimits` answers with at
//! that instant, and the shards the refused call would have laid down. A refused call stores
//! nothing and leaves the reported usage exactly as it was.
//!
//! The recorded flow this was derived from reads `describe_limits()` and then asks
//! `create_stream()` for `ShardLimit - OpenShardCount + 5` shards, which is one row of the
//! tables below. The rows differ along what the rule generalises over: how much of the quota
//! the region already holds (nothing, one stream, a stream whose shard a split closed, an
//! on-demand stream, a stream whose removal has fallen due), the region the request is
//! signed for, which side of the boundary the request sits on (exactly the room left, one
//! past it, far past it), the capacity mode, and which operation lays the shards down.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    DELETION_DELAY, ON_DEMAND_INITIAL_SHARDS, STREAM_MODE_ON_DEMAND, STREAM_MODE_PROVISIONED,
};

use crate::shard_scaling::{hash_key_range, is_open, shard_id, shard_map};
use crate::stream_tags::{error_code, error_message, past_activation, Emulator, REGION};

/// A second region, so a case can show the quota is the quota of the region the request was
/// signed for.
const OTHER_REGION: &str = "eu-central-1";

/// How long a case waits before treating a deleted stream as gone: past the instant the
/// delete armed the `DELETING` -> gone transition for.
fn past_deletion() -> Duration {
    DELETION_DELAY + Duration::from_millis(400)
}

/// The message AWS answers `LimitExceededException` with when a request would take an
/// account and region past its shard quota — the string Service Quotas tooling keys on, with
/// the account, the region, the usage, the quota and the shards the request would have laid
/// down filled in. Written out here so the wire wording is pinned by the test and not only
/// by the handler that builds it.
fn expected_refusal(
    region: &str,
    open_shard_count: i64,
    shard_limit: i64,
    additional_shards: i64,
) -> String {
    format!(
        "This request would exceed the shard limit for the account {} in {region}. Current \
         shard count for the account: {open_shard_count}. Limit: {shard_limit}. Number of \
         additional shards that would have resulted from this request: {additional_shards}. \
         Refer to the AWS Service Limits page \
         (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current \
         limits and how to request higher limits.",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// The `ShardLimit` and `OpenShardCount` `DescribeLimits` reports in `region`.
async fn quota_in(emulator: &Emulator, case: &str, region: &str) -> (i64, i64) {
    let response = emulator
        .ok_in(case, region, "DescribeLimits", &json!({}))
        .await;
    let member = |name: &str| -> i64 {
        response
            .get(name)
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: DescribeLimits must carry {name}, got {response}"))
    };
    (member("ShardLimit"), member("OpenShardCount"))
}

/// A `CreateStream` body for a provisioned stream of `shards` shards.
fn provisioned(name: &str, shards: i64) -> Value {
    json!({
        "StreamName": name,
        "ShardCount": shards,
        "StreamModeDetails": { "StreamMode": STREAM_MODE_PROVISIONED },
    })
}

/// Assert that `response` is the modelled quota refusal, worded as AWS words it.
fn assert_quota_refusal(
    case: &str,
    status: u16,
    response: &Value,
    region: &str,
    open_shard_count: i64,
    shard_limit: i64,
    additional_shards: i64,
) {
    assert_eq!(
        status, 400,
        "case {case}: expected a client error, got {status} with body {response}"
    );
    assert_eq!(
        error_code(response),
        "LimitExceededException",
        "case {case}: expected the modelled quota refusal, got {response}"
    );
    assert_eq!(
        error_message(response),
        expected_refusal(region, open_shard_count, shard_limit, additional_shards),
        "case {case}: expected AWS's quota refusal wording, got {}",
        error_message(response)
    );
}

/// What the region holds when a case's boundary calls arrive.
#[derive(Debug, Clone, Copy)]
enum Usage {
    /// Nothing at all.
    Empty,
    /// One provisioned stream of this many shards.
    Provisioned(i64),
    /// One provisioned stream of two shards, one of which a split has closed — a closed
    /// shard is held by the store while it provides no capacity.
    Split,
    /// One stream of the on-demand capacity mode, whose shard count the service manages.
    OnDemand,
    /// One provisioned stream whose removal has already fallen due, so the account holds
    /// nothing again.
    Removed,
}

/// Put `region` into `usage`, under names derived from `case`.
async fn hold(emulator: &Emulator, case: &str, region: &str, usage: Usage) {
    let name = format!("{case}-held");
    match usage {
        Usage::Empty => {}
        Usage::Provisioned(shards) => {
            emulator
                .ok_in(case, region, "CreateStream", &provisioned(&name, shards))
                .await;
        }
        Usage::Split => {
            emulator
                .ok_in(case, region, "CreateStream", &provisioned(&name, 2))
                .await;
            tokio::time::sleep(past_activation()).await;
            let map = shard_map(emulator, case, &name).await;
            let parent = map
                .first()
                .unwrap_or_else(|| panic!("case {case}: the stream must hold a shard to split"));
            let (starting, ending) = hash_key_range(case, parent);
            emulator
                .ok_in(
                    case,
                    region,
                    "SplitShard",
                    &json!({
                        "StreamName": name,
                        "ShardToSplit": shard_id(case, parent),
                        "NewStartingHashKey": (starting + (ending - starting) / 2).to_string(),
                    }),
                )
                .await;
        }
        Usage::OnDemand => {
            emulator
                .ok_in(
                    case,
                    region,
                    "CreateStream",
                    &json!({
                        "StreamName": name,
                        "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
                    }),
                )
                .await;
        }
        Usage::Removed => {
            emulator
                .ok_in(case, region, "CreateStream", &provisioned(&name, 10))
                .await;
            // A delete is served only for an `ACTIVE` stream, so the create's transition is
            // waited out before the removal is armed.
            tokio::time::sleep(past_activation()).await;
            emulator
                .ok_in(
                    case,
                    region,
                    "DeleteStream",
                    &json!({ "StreamName": name }),
                )
                .await;
            tokio::time::sleep(past_deletion()).await;
        }
    }
}

/// Both sides of the published boundary, for every usage a region can be in: a create for
/// exactly the shards the quota leaves is served, and one for a single shard more is refused
/// with AWS's wording.
#[tokio::test]
async fn a_create_is_served_up_to_the_published_quota_and_refused_past_it() {
    // (the case, the region it is driven in, what that region already holds)
    let cases: Vec<(&str, &str, Usage)> = vec![
        ("quota-empty", REGION, Usage::Empty),
        ("quota-one-shard", REGION, Usage::Provisioned(1)),
        ("quota-many-shards", REGION, Usage::Provisioned(37)),
        ("quota-split-shard", REGION, Usage::Split),
        ("quota-on-demand", REGION, Usage::OnDemand),
        ("quota-removed", REGION, Usage::Removed),
        ("quota-other-region", OTHER_REGION, Usage::Provisioned(5)),
    ];

    for (case, region, usage) in cases {
        let emulator = Emulator::new();
        hold(&emulator, case, region, usage).await;

        let (shard_limit, open_shard_count) = quota_in(&emulator, case, region).await;
        let room = shard_limit - open_shard_count;
        assert!(
            room > 0,
            "case {case}: expected the quota to leave room over the usage, got \
             {shard_limit}/{open_shard_count}"
        );

        // One shard more than the quota leaves is refused, and stores nothing.
        let over = format!("{case}-over");
        let (status, response) = emulator
            .call_in(region, "CreateStream", &provisioned(&over, room + 1))
            .await;
        assert_quota_refusal(
            case,
            status,
            &response,
            region,
            open_shard_count,
            shard_limit,
            room + 1,
        );
        let (status, response) = emulator
            .call_in(region, "DescribeStreamSummary", &json!({ "StreamName": over }))
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: a refused create stores nothing, got {status} with body {response}"
        );
        assert_eq!(
            quota_in(&emulator, case, region).await,
            (shard_limit, open_shard_count),
            "case {case}: a refused create leaves the reported usage as it was"
        );

        // Exactly the room the quota leaves is served, in ONE stream however large that is,
        // and the stream that create stored reports those shards back.
        let fits = format!("{case}-fits");
        emulator
            .ok_in(case, region, "CreateStream", &provisioned(&fits, room))
            .await;
        let described = emulator
            .ok_in(
                case,
                region,
                "DescribeStreamSummary",
                &json!({ "StreamName": fits }),
            )
            .await;
        let stored = described["StreamDescriptionSummary"]
            .get("OpenShardCount")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case}: the summary must carry OpenShardCount, got {described}"));
        assert_eq!(
            stored, room,
            "case {case}: expected the stream to hold the {room} shards the quota left, got \
             {stored}"
        );
        let (filled_limit, filled_open) = quota_in(&emulator, case, region).await;
        assert_eq!(
            (filled_limit, filled_open),
            (shard_limit, shard_limit),
            "case {case}: expected the region to sit at its quota of {shard_limit} open \
             shards, got {filled_limit}/{filled_open}"
        );

        // And the quota is spent: one more shard is refused, counted against the new usage.
        let (status, response) = emulator
            .call_in(
                region,
                "CreateStream",
                &provisioned(&format!("{case}-last"), 1),
            )
            .await;
        assert_quota_refusal(
            case,
            status,
            &response,
            region,
            filled_open,
            filled_limit,
            1,
        );
    }
}

/// The refusal's variable parts come from the request and the store: whatever count a
/// request past the quota carries is the count the message reports, and the quota and usage
/// it names are the ones `DescribeLimits` answers with.
#[tokio::test]
async fn the_quota_refusal_names_the_shards_the_request_asked_for() {
    let emulator = Emulator::new();
    let case = "quota-requested-count";
    // A refusal stores nothing, so every row is driven against the same held usage.
    hold(&emulator, case, REGION, Usage::Provisioned(12)).await;
    let (shard_limit, open_shard_count) = quota_in(&emulator, case, REGION).await;
    let room = shard_limit - open_shard_count;

    // Each row: how many shards past the room left the request asks for.
    for past in [1_i64, 5, 37, room, room + open_shard_count, 100_000] {
        let requested = room + past;
        let label = format!("{case}/{requested}");
        let (status, response) = emulator
            .call(
                "CreateStream",
                &provisioned(&format!("{case}-{requested}"), requested),
            )
            .await;
        assert_quota_refusal(
            &label,
            status,
            &response,
            REGION,
            open_shard_count,
            shard_limit,
            requested,
        );
        assert_eq!(
            quota_in(&emulator, &label, REGION).await,
            (shard_limit, open_shard_count),
            "case {label}: a refused create leaves the reported usage as it was"
        );
    }
}

/// Every operation that lays a shard down is measured against that ONE published quota, and
/// refuses past it with that ONE wording — while a call that fits in the room left is served.
#[tokio::test]
async fn every_call_that_lays_down_a_shard_is_refused_past_the_published_quota() {
    let emulator = Emulator::new();
    let case = "quota-every-call";
    let subject = format!("{case}-subject");
    let (shard_limit, _) = quota_in(&emulator, case, REGION).await;

    // A stream to re-tile, and a filler stream that leaves room for exactly ONE more shard.
    emulator
        .ok(case, "CreateStream", &provisioned(&subject, 4))
        .await;
    emulator
        .ok(
            case,
            "CreateStream",
            &provisioned(&format!("{case}-filler"), shard_limit - 5),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let (shard_limit, open_shard_count) = quota_in(&emulator, case, REGION).await;
    assert_eq!(
        shard_limit - open_shard_count,
        1,
        "case {case}: expected exactly one shard of room, got {shard_limit}/{open_shard_count}"
    );

    let map = shard_map(&emulator, case, &subject).await;
    let first = map
        .first()
        .unwrap_or_else(|| panic!("case {case}: the subject must hold shards"));
    let (starting, ending) = hash_key_range(case, first);

    // (the call, its body, how many shards it would lay down)
    let calls: Vec<(&str, Value, i64)> = vec![
        (
            "CreateStream",
            provisioned(&format!("{case}-another"), 2),
            2,
        ),
        (
            "UpdateShardCount",
            json!({
                "StreamName": subject,
                "TargetShardCount": 8,
                "ScalingType": "UNIFORM_SCALING",
            }),
            8,
        ),
        (
            "SplitShard",
            json!({
                "StreamName": subject,
                "ShardToSplit": shard_id(case, first),
                "NewStartingHashKey": (starting + (ending - starting) / 2).to_string(),
            }),
            2,
        ),
    ];

    for (operation, body, additional) in calls {
        let label = format!("{case}/{operation}");
        let (status, response) = emulator.call(operation, &body).await;
        assert_quota_refusal(
            &label,
            status,
            &response,
            REGION,
            open_shard_count,
            shard_limit,
            additional,
        );
        assert_eq!(
            quota_in(&emulator, &label, REGION).await,
            (shard_limit, open_shard_count),
            "case {label}: a refused call leaves the reported usage as it was"
        );
    }

    // A merge lays down ONE shard, which the room left admits, and it gives capacity back:
    // the two shards it closes stop counting as open.
    let second = map
        .get(1)
        .unwrap_or_else(|| panic!("case {case}: the subject must hold two shards to merge"));
    emulator
        .ok(
            case,
            "MergeShards",
            &json!({
                "StreamName": subject,
                "ShardToMerge": shard_id(case, first),
                "AdjacentShardToMerge": shard_id(case, second),
            }),
        )
        .await;
    let (_, merged_open) = quota_in(&emulator, case, REGION).await;
    assert_eq!(
        merged_open,
        open_shard_count - 1,
        "case {case}: expected the merge to leave one open shard fewer, got {merged_open}"
    );
    let stored = shard_map(&emulator, case, &subject).await;
    let open = stored.iter().filter(|shard| is_open(case, shard)).count();
    assert_eq!(
        (stored.len(), open),
        (5, 3),
        "case {case}: expected the merge to leave 3 open shards of 5 stored, got {stored:?}"
    );
}

/// The on-demand capacity mode takes its initial shards from the same quota: the shards the
/// service manages for such a stream are counted and refused past exactly as a provisioned
/// stream's are.
#[tokio::test]
async fn an_on_demand_create_is_counted_against_the_same_quota() {
    let emulator = Emulator::new();
    let case = "quota-on-demand-create";
    let (shard_limit, _) = quota_in(&emulator, case, REGION).await;
    let initial = i64::from(ON_DEMAND_INITIAL_SHARDS);

    // Leave less room than an on-demand stream's initial shard count needs.
    emulator
        .ok(
            case,
            "CreateStream",
            &provisioned(&format!("{case}-filler"), shard_limit - initial + 1),
        )
        .await;
    let (shard_limit, open_shard_count) = quota_in(&emulator, case, REGION).await;

    let (status, response) = emulator
        .call(
            "CreateStream",
            &json!({
                "StreamName": format!("{case}-over"),
                "StreamModeDetails": { "StreamMode": STREAM_MODE_ON_DEMAND },
            }),
        )
        .await;
    assert_quota_refusal(
        case,
        status,
        &response,
        REGION,
        open_shard_count,
        shard_limit,
        initial,
    );
    assert_eq!(
        quota_in(&emulator, case, REGION).await,
        (shard_limit, open_shard_count),
        "case {case}: a refused create leaves the reported usage as it was"
    );
}
