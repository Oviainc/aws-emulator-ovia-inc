// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutBandwidthRateLimitSchedule`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle` and the interval
//! helpers and table of the sibling `bandwidth_rate_limit_schedule` suite, so what is
//! asserted is the wire behaviour and the only way a later request can see what an
//! earlier one wrote is the shared store.
//!
//! The rule under test: `PutBandwidthRateLimitSchedule` PERSISTS the whole schedule its
//! request carries on the gateway its `GatewayArn` resolves to, REPLACING whatever that
//! record held, in the order the request listed the intervals and with every member
//! exactly as sent; it answers with that record's own ARN; being `@idempotent`, the same
//! request twice leaves the record exactly as one call left it; and it refuses an ARN the
//! store holds no gateway under with the modelled `ResourceNotFoundException`, writing
//! nothing.
//!
//! The tables below span that class: schedules differing in interval count (including
//! both published bounds of `BandwidthRateLimitIntervals`,
//! `@length(min: 0, max: 20)`), in order, in whether the optional upload rate limit is
//! set, and at each published bound of every interval member; ARNs differing in
//! partition, resource type, id and length, including each published bound of `GatewayArn`
//! and one past it; and inputs one past every published bound of every interval member.
//!
//! Each write is read back through `GetBandwidthRateLimitSchedule` — the implemented
//! reader of the same field — so what is asserted is a round trip through the store and
//! not the response the writer happened to build.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::ops::types::BandwidthRateLimitInterval;
use backup_gateway::state::service_state;

use crate::bandwidth_rate_limit_schedule::{
    arn_in, arn_of_length, expected_intervals, get_schedule, interval, persisted_intervals,
    schedule_rows,
};
use crate::gateway_lifecycle::{build_engine, build_router, call, create, never_created_arn, Reply};

/// `PutBandwidthRateLimitSchedule` for `intervals` on `arn`.
///
/// The REQUEST's interval list is rendered here rather than with the sibling suite's
/// response renderer, so a bug in either rendering cannot cancel itself out: the request
/// is built from the typed intervals on one side and the expected response from them on
/// the other.
///
/// Shared with the sibling `bandwidth_rate_limit_schedule_consistency` suite, which drives
/// the same writer over schedules whose intervals do not hold together: one place renders
/// a `PutBandwidthRateLimitSchedule` request.
pub(crate) async fn put_schedule(
    router: &MultiServiceRouter,
    arn: &str,
    intervals: &[BandwidthRateLimitInterval],
) -> Reply {
    let sent = Value::Array(
        intervals
            .iter()
            .map(|interval| {
                let mut rendered = json!({
                    "StartHourOfDay": interval.start_hour_of_day,
                    "StartMinuteOfHour": interval.start_minute_of_hour,
                    "EndHourOfDay": interval.end_hour_of_day,
                    "EndMinuteOfHour": interval.end_minute_of_hour,
                    "DaysOfWeek": interval.days_of_week,
                });
                // `AverageUploadRateLimitInBitsPerSec` is the one optional member of
                // `BandwidthRateLimitInterval`, so a row that leaves it unset sends no
                // such key.
                if let Some(rate) = interval.average_upload_rate_limit_in_bits_per_sec {
                    rendered["AverageUploadRateLimitInBitsPerSec"] = json!(rate);
                }
                rendered
            })
            .collect(),
    );
    call(
        router,
        "PutBandwidthRateLimitSchedule",
        &json!({
            "GatewayArn": arn,
            "BandwidthRateLimitIntervals": sent,
        }),
    )
    .await
}

/// The intervals `GetBandwidthRateLimitSchedule` reports for `arn`.
async fn read_back(router: &MultiServiceRouter, arn: &str) -> Value {
    let reply = get_schedule(router, arn).await;
    assert_eq!(
        reply.status, 200,
        "reading back {arn}: GetBandwidthRateLimitSchedule must answer 200, got {} {:?}",
        reply.status, reply.body
    );
    reply
        .body
        .get("BandwidthRateLimitIntervals")
        .cloned()
        .unwrap_or(Value::Null)
}

/// Every schedule of the shared table is persisted as sent and reads back through
/// `GetBandwidthRateLimitSchedule`, and the response names the stored record's own ARN.
#[tokio::test]
async fn put_bandwidth_rate_limit_schedule_persists_the_schedule_it_was_sent() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = schedule_rows();
    let mut arns = Vec::new();

    for (index, (case, intervals)) in rows.iter().enumerate() {
        let arn = create(&router, &format!("gw-{index}")).await;

        let reply = put_schedule(&router, &arn, intervals).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: a stored gateway must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.str_at("/GatewayArn"),
            Some(arn.as_str()),
            "case {case:?}: the reported ARN must be the record's own, got {:?}",
            reply.body
        );
        assert_eq!(
            persisted_intervals(&engine, &arn),
            format!("{intervals:?}"),
            "case {case:?}: the stored intervals must be the ones the request carried"
        );
        assert_eq!(
            read_back(&router, &arn).await,
            expected_intervals(intervals),
            "case {case:?}: the written schedule must read back in the order it was sent"
        );
        arns.push(arn);
    }

    // Each write landed on its OWN record: re-reading every ARN after all of them were
    // written still reports that row's intervals, so no put overwrote another gateway.
    for ((case, intervals), arn) in rows.iter().zip(&arns) {
        assert_eq!(
            read_back(&router, arn).await,
            expected_intervals(intervals),
            "case {case:?}: each ARN must still report the schedule written to it"
        );
    }
}

/// A put states the WHOLE schedule: each write replaces the one before it, the empty list
/// clears the schedule back to "no bandwidth rate limiting in effect", and the same
/// request twice leaves the record exactly as one call left it (`@idempotent`).
#[tokio::test]
async fn putting_a_schedule_replaces_the_schedule_before_it() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-replace").await;

    // Rows walk a SEQUENCE of writes over one record: a schedule, a longer one, a shorter
    // one, the same one again, the empty list, and a non-empty one after the clear.
    let sequence: Vec<(&str, Vec<BandwidthRateLimitInterval>)> = vec![
        (
            "a first schedule of one interval",
            vec![interval(1, 0, 2, 30, &[1, 2, 3, 4, 5], None)],
        ),
        (
            "three intervals replace the one before",
            vec![
                interval(0, 0, 5, 59, &[0, 6], Some(102_400)),
                interval(22, 15, 23, 45, &[1, 2, 3, 4, 5], None),
                interval(6, 30, 7, 0, &[0], Some(1_048_576)),
            ],
        ),
        (
            "a shorter schedule drops the intervals that are gone",
            vec![interval(6, 30, 7, 0, &[0], Some(1_048_576))],
        ),
        (
            "the same schedule again",
            vec![interval(6, 30, 7, 0, &[0], Some(1_048_576))],
        ),
        ("the empty list clears the schedule", vec![]),
        (
            "a schedule set again after a clear",
            vec![interval(12, 0, 12, 30, &[4], Some(8_000_000_000_000))],
        ),
    ];

    for (case, intervals) in &sequence {
        let reply = put_schedule(&router, &arn, intervals).await;
        assert_eq!(
            (reply.status, reply.str_at("/GatewayArn")),
            (200, Some(arn.as_str())),
            "case {case:?}: the put must answer 200 with the record's ARN, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            read_back(&router, &arn).await,
            expected_intervals(intervals),
            "case {case:?}: the record must hold exactly the schedule last written"
        );
    }

    // Idempotence, stated over the store: repeating the last request changes neither the
    // response nor what is persisted.
    let (_, last) = sequence.last().expect("the sequence has rows");
    let before = persisted_intervals(&engine, &arn);
    let first = put_schedule(&router, &arn, last).await;
    let again = put_schedule(&router, &arn, last).await;
    assert_eq!(
        (first.status, &first.body),
        (again.status, &again.body),
        "the same request twice must answer identically, got {:?} then {:?}",
        first.body,
        again.body
    );
    assert_eq!(
        persisted_intervals(&engine, &arn),
        before,
        "the same request twice must leave the record exactly as one call left it"
    );
}

/// A well-formed `GatewayArn` the store holds no gateway under is refused with the
/// modelled `ResourceNotFoundException` — HTTP 400 carrying `ErrorCode` `GatewayNotFound`
/// and the fixed sentence this service states for a missing gateway, the way every other
/// gateway operation of this service
/// refuses one — whatever the ARN's partition, resource type, id or length, and nothing is
/// written.
#[tokio::test]
async fn a_put_for_an_arn_with_no_stored_gateway_is_refused_and_writes_nothing() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway that really exists, with a schedule of its own, so a refusal below is
    // about the ARN asked for and not about an empty store.
    let live = create(&router, "gw-live").await;
    let live_schedule = vec![interval(3, 0, 4, 0, &[2], Some(204_800))];
    assert_eq!(
        put_schedule(&router, &live, &live_schedule).await.status,
        200,
        "the live gateway must take its schedule"
    );
    let live_stored = persisted_intervals(&engine, &live);

    let rows: Vec<(&str, String)> = vec![
        ("a gateway ARN that was never created", never_created_arn()),
        (
            "a gateway ARN in the aws-cn partition",
            arn_in("aws-cn", "gateway", "bgw-ffffffffffffff02"),
        ),
        (
            "a gateway ARN in the aws-us-gov partition",
            arn_in("aws-us-gov", "gateway", "bgw-ffffffffffffff03"),
        ),
        (
            "an ARN naming another resource type of this service",
            arn_in("aws", "hypervisor", "hv-ffffffffffffff04"),
        ),
        (
            "the 50-character minimum of the modelled GatewayArn",
            arn_of_length(50),
        ),
        (
            "the 180-character maximum of the modelled GatewayArn",
            arn_of_length(180),
        ),
    ];

    for (case, arn) in &rows {
        let reply = put_schedule(&router, arn, &[interval(8, 0, 9, 0, &[1], None)]).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode"),
                reply.str_at("/Message"),
            ),
            (
                400,
                Some("ResourceNotFoundException"),
                Some("GatewayNotFound"),
                Some("The specified gateway was not found."),
            ),
            "case {case:?}: an ARN with no stored gateway must be refused 400 \
             ResourceNotFoundException / GatewayNotFound, got {} {:?}",
            reply.status,
            reply.body
        );
        let gone = get_schedule(&router, arn).await;
        assert_eq!(
            (gone.status, gone.code.as_deref()),
            (400, Some("ResourceNotFoundException")),
            "case {case:?}: a refused put must not have created a record, got {} {:?}",
            gone.status,
            gone.body
        );
    }

    // The record that does exist is untouched by every refusal above, and a record the
    // store no longer holds is refused exactly like one it never held.
    assert_eq!(
        persisted_intervals(&engine, &live),
        live_stored,
        "a refused put must leave every other record exactly as it was"
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .gateway_count(),
        1,
        "a refused put must not add or drop a record"
    );

    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let after_delete = put_schedule(&router, &live, &live_schedule).await;
    assert_eq!(
        (after_delete.status, after_delete.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "a deleted gateway must be refused like one that never existed, got {} {:?}",
        after_delete.status,
        after_delete.body
    );
}

/// A request outside the modelled constraints never reaches the store: it is refused with
/// HTTP 400 `ValidationException`, one past each published bound of `GatewayArn`
/// (`@length(min: 50, max: 180)` and its pattern), of `BandwidthRateLimitIntervals`
/// (`@length(min: 0, max: 20)`) and of every member of `BandwidthRateLimitInterval`
/// (`HourOfDay` 0..23, `MinuteOfHour` 0..59, `DayOfWeek` 0..6, `DaysOfWeek`
/// `@length(min: 1, max: 7)`, `AverageUploadRateLimit` 51200..8000000000000 —
/// `smithy_query(query="shape",shape="BandwidthRateLimitInterval")`).
#[tokio::test]
async fn a_put_outside_the_modelled_constraints_is_refused_with_400() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-validation").await;
    let stored = vec![interval(5, 0, 6, 0, &[3], Some(51_200))];
    assert_eq!(
        put_schedule(&router, &arn, &stored).await.status,
        200,
        "the gateway must take a schedule before the refusals below"
    );
    let before = persisted_intervals(&engine, &arn);

    /// One interval as a JSON object, so a row can carry a member the typed shape cannot
    /// hold (an hour of 24, a day of 7, a missing required member).
    fn interval_json(
        start_hour: i64,
        start_minute: i64,
        end_hour: i64,
        end_minute: i64,
        days: Vec<i64>,
        average_upload: Option<i64>,
    ) -> Value {
        let mut rendered = json!({
            "StartHourOfDay": start_hour,
            "EndHourOfDay": end_hour,
            "StartMinuteOfHour": start_minute,
            "EndMinuteOfHour": end_minute,
            "DaysOfWeek": days,
        });
        if let Some(rate) = average_upload {
            rendered["AverageUploadRateLimitInBitsPerSec"] = json!(rate);
        }
        rendered
    }

    let ok = interval_json(1, 0, 2, 0, vec![1], None);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one interval over the 20-interval maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals":
                    Value::Array((0..21).map(|_| ok.clone()).collect()),
            }),
        ),
        (
            "StartHourOfDay one over the maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(24, 0, 2, 0, vec![1], None)],
            }),
        ),
        (
            "EndHourOfDay one under the minimum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, -1, 0, vec![1], None)],
            }),
        ),
        (
            "StartMinuteOfHour one over the maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 60, 2, 0, vec![1], None)],
            }),
        ),
        (
            "EndMinuteOfHour one under the minimum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, 2, -1, vec![1], None)],
            }),
        ),
        (
            "a DaysOfWeek ordinal one over the maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, 2, 0, vec![7], None)],
            }),
        ),
        (
            "a DaysOfWeek ordinal one under the minimum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, 2, 0, vec![-1], None)],
            }),
        ),
        (
            "DaysOfWeek shorter than its one-entry minimum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, 2, 0, vec![], None)],
            }),
        ),
        (
            "DaysOfWeek one entry over its seven-entry maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals":
                    [interval_json(1, 0, 2, 0, vec![0, 1, 2, 3, 4, 5, 6, 0], None)],
            }),
        ),
        (
            "AverageUploadRateLimitInBitsPerSec one under the minimum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [interval_json(1, 0, 2, 0, vec![1], Some(51_199))],
            }),
        ),
        (
            "AverageUploadRateLimitInBitsPerSec one over the maximum",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals":
                    [interval_json(1, 0, 2, 0, vec![1], Some(8_000_000_000_001))],
            }),
        ),
        (
            "a required interval member absent",
            json!({
                "GatewayArn": arn,
                "BandwidthRateLimitIntervals": [json!({
                    "StartHourOfDay": 1,
                    "StartMinuteOfHour": 0,
                    "EndMinuteOfHour": 0,
                    "DaysOfWeek": [1],
                })],
            }),
        ),
        (
            "required BandwidthRateLimitIntervals absent",
            json!({ "GatewayArn": arn }),
        ),
        (
            "required GatewayArn absent",
            json!({ "BandwidthRateLimitIntervals": [ok.clone()] }),
        ),
        (
            "a GatewayArn one character under the 50-character minimum",
            json!({
                "GatewayArn": arn_of_length(49),
                "BandwidthRateLimitIntervals": [ok.clone()],
            }),
        ),
        (
            "a GatewayArn one character over the 180-character maximum",
            json!({
                "GatewayArn": arn_of_length(181),
                "BandwidthRateLimitIntervals": [ok.clone()],
            }),
        ),
        (
            "a partition the GatewayArn pattern does not accept",
            json!({
                "GatewayArn": arn_in("aws-iso", "gateway", "bgw-0000000000000003"),
                "BandwidthRateLimitIntervals": [ok.clone()],
            }),
        ),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "PutBandwidthRateLimitSchedule", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a request outside the modelled constraints must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_intervals(&engine, &arn),
            before,
            "case {case:?}: a refused request must not have written anything"
        );
    }
}

/// A put writes the SCHEDULE and nothing else: the gateway's other members are untouched,
/// and another gateway's schedule is untouched too.
#[tokio::test]
async fn a_put_touches_only_the_schedule_of_the_gateway_it_names() {
    let engine = build_engine();
    let router = build_router(&engine);
    let target = create(&router, "gw-target").await;
    let bystander = create(&router, "gw-bystander").await;

    let before = call(&router, "GetGateway", &json!({ "GatewayArn": target })).await;
    assert_eq!(
        before.status, 200,
        "GetGateway must answer 200, got {} {:?}",
        before.status, before.body
    );

    let schedule = vec![
        interval(8, 0, 11, 59, &[1, 3, 5], Some(51_200)),
        interval(20, 0, 23, 59, &[0, 6], None),
    ];
    assert_eq!(
        put_schedule(&router, &target, &schedule).await.status,
        200,
        "the put must be accepted"
    );

    let after = call(&router, "GetGateway", &json!({ "GatewayArn": target })).await;
    assert_eq!(
        (after.status, &after.body),
        (before.status, &before.body),
        "a put of a schedule must leave every other member of the gateway as it was, got \
         {:?} then {:?}",
        before.body,
        after.body
    );
    assert_eq!(
        read_back(&router, &bystander).await,
        json!([]),
        "a put must not write a schedule onto another gateway"
    );
}
