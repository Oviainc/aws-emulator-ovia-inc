// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetBandwidthRateLimitSchedule`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `GetBandwidthRateLimitSchedule` reports the bandwidth rate-limit
//! schedule PERSISTED on the gateway its `GatewayArn` resolves to — that record's own ARN
//! and every stored interval, in the stored order, with each member exactly as stored —
//! answers a gateway that has no schedule with the EMPTY array the modelled output
//! documents, and refuses an ARN the store holds no gateway under with the modelled
//! `ResourceNotFoundException`. Being a read, it changes nothing a later caller sees.
//!
//! The tables below span that class: schedules differing in interval count (including
//! both published bounds of `BandwidthRateLimitIntervals`, `@length(min: 0, max: 20)`),
//! in order, in whether the optional upload rate limit is set, and at each published
//! bound of every interval member; and ARNs differing in partition, resource type, id and
//! length, including each published bound of `GatewayArn` and one past it.
//!
//! The schedules read here are seeded straight onto the stored record, through the same
//! `bandwidth_rate_limit_intervals` field `PutBandwidthRateLimitSchedule` writes, so this
//! suite asserts the READ on its own; the sibling `put_bandwidth_rate_limit_schedule`
//! suite drives that writer over the wire and reuses the table and helpers below.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::ops::types::BandwidthRateLimitInterval;
use backup_gateway::state::service_state;

use crate::gateway_lifecycle::{build_engine, build_router, call, create, never_created_arn, Reply, REGION};

/// A gateway-shaped ARN in `partition` naming `id`, so the rows span every partition the
/// modelled `GatewayArn` pattern accepts (`aws`, `aws-cn`, `aws-us-gov`) and both
/// resource types this service's ARNs name.
///
/// Shared with the sibling `maintenance_start_time` suite, which refuses the same ARN
/// rows: one place builds a `GatewayArn` the modelled pattern accepts.
pub(crate) fn arn_in(partition: &str, resource_type: &str, id: &str) -> String {
    format!(
        "arn:{partition}:backup-gateway:{REGION}:{}:{resource_type}/{id}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// A `GatewayArn`-shaped ARN of exactly `len` characters, for the published length bounds
/// of the modelled shape: `@length(min: 50, max: 180)`
/// (`smithy_query(query="shape",shape="GatewayArn")`). The id segment absorbs the length,
/// so every character stays inside the pattern's `[a-zA-Z-0-9]+`, and the resource type is
/// kept short enough that the 50-character minimum fits.
///
/// Shared with the sibling `maintenance_start_time` suite, which asserts the same bounds.
pub(crate) fn arn_of_length(len: usize) -> String {
    let prefix = format!(
        "arn:aws:backup-gateway:{REGION}:{}:g/",
        engine::EMULATOR_ACCOUNT_ID
    );
    assert!(
        len > prefix.len(),
        "a {len}-character ARN cannot hold the {}-character prefix",
        prefix.len()
    );
    format!("{prefix}{}", "x".repeat(len - prefix.len()))
}

/// One modelled `BandwidthRateLimitInterval`
/// (`smithy_query(query="shape",shape="BandwidthRateLimitInterval")`: every member
/// required but `AverageUploadRateLimitInBitsPerSec`).
pub(crate) fn interval(
    start_hour: i32,
    start_minute: i32,
    end_hour: i32,
    end_minute: i32,
    days: &[i32],
    average_upload: Option<i64>,
) -> BandwidthRateLimitInterval {
    BandwidthRateLimitInterval {
        average_upload_rate_limit_in_bits_per_sec: average_upload,
        start_hour_of_day: start_hour,
        end_hour_of_day: end_hour,
        start_minute_of_hour: start_minute,
        end_minute_of_hour: end_minute,
        days_of_week: days.to_vec(),
    }
}

/// The `BandwidthRateLimitIntervals` list as it must appear in the response body, in the
/// stored order. `AverageUploadRateLimitInBitsPerSec` is absent where it was not set:
/// "This field does not appear in the response if the upload rate limit is not set"
/// (`smithy_query(query="shape",shape="BandwidthRateLimitInterval")`).
pub(crate) fn expected_intervals(intervals: &[BandwidthRateLimitInterval]) -> Value {
    Value::Array(
        intervals
            .iter()
            .map(|interval| {
                let mut rendered = json!({
                    "StartHourOfDay": interval.start_hour_of_day,
                    "EndHourOfDay": interval.end_hour_of_day,
                    "StartMinuteOfHour": interval.start_minute_of_hour,
                    "EndMinuteOfHour": interval.end_minute_of_hour,
                    "DaysOfWeek": interval.days_of_week,
                });
                if let Some(rate) = interval.average_upload_rate_limit_in_bits_per_sec {
                    rendered["AverageUploadRateLimitInBitsPerSec"] = json!(rate);
                }
                rendered
            })
            .collect(),
    )
}

pub(crate) async fn get_schedule(router: &MultiServiceRouter, arn: &str) -> Reply {
    call(
        router,
        "GetBandwidthRateLimitSchedule",
        &json!({ "GatewayArn": arn }),
    )
    .await
}

/// Set `intervals` on the stored gateway — the one field
/// `PutBandwidthRateLimitSchedule` will write (see the module docs).
fn set_schedule(
    engine: &Arc<engine::InMemoryEngine>,
    arn: &str,
    intervals: Vec<BandwidthRateLimitInterval>,
) {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    state
        .lock()
        .gateway_mut(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"))
        .bandwidth_rate_limit_intervals = intervals;
}

/// The intervals PERSISTED on the record under `arn`, rendered for comparison: what a read
/// must not have changed.
pub(crate) fn persisted_intervals(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let record = store
        .gateway(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"));
    format!("{:?}", record.bandwidth_rate_limit_intervals)
}

/// The rows of the schedule table: schedules spanning interval count, order, the optional
/// upload rate limit and each published bound of every interval member.
pub(crate) fn schedule_rows() -> Vec<(&'static str, Vec<BandwidthRateLimitInterval>)> {
    // The full 20-interval maximum of `BandwidthRateLimitIntervals`
    // (`@length(min: 0, max: 20)`), one per hour so no two rows are alike.
    let twenty: Vec<BandwidthRateLimitInterval> = (0..20)
        .map(|hour| {
            interval(
                hour,
                0,
                hour,
                59,
                &[(hour % 7)],
                Some(51_200 * i64::from(hour + 1)),
            )
        })
        .collect();

    vec![
        (
            "no schedule has ever been set: the empty array the model documents",
            vec![],
        ),
        (
            "one interval, no upload rate limit set",
            vec![interval(1, 0, 2, 30, &[1, 2, 3, 4, 5], None)],
        ),
        (
            "three intervals, stored order preserved, one day repeated",
            vec![
                interval(0, 0, 5, 59, &[0, 6], Some(102_400)),
                interval(22, 15, 23, 45, &[1, 2, 3, 4, 5], None),
                interval(6, 30, 7, 0, &[0], Some(1_048_576)),
            ],
        ),
        (
            // `StartHourOfDay`/`EndHourOfDay` `@range(min: 0, max: 23)`,
            // `StartMinuteOfHour`/`EndMinuteOfHour` 0..59, `DaysOfWeek`
            // `@length(min: 1, max: 7)` over ordinals 0..6,
            // `AverageUploadRateLimitInBitsPerSec` 51200..8000000000000.
            "every member at its published minimum",
            vec![interval(0, 0, 0, 0, &[0], Some(51_200))],
        ),
        (
            "every member at its published maximum",
            vec![interval(
                23,
                59,
                23,
                59,
                &[0, 1, 2, 3, 4, 5, 6],
                Some(8_000_000_000_000),
            )],
        ),
        (
            "the 20-interval maximum of BandwidthRateLimitIntervals",
            twenty,
        ),
    ]
}

/// Every member the response carries comes off the stored record, for schedules spanning
/// interval count, order, the optional upload rate limit and each published bound of every
/// interval member — and each gateway answers from its OWN record.
#[tokio::test]
async fn get_bandwidth_rate_limit_schedule_reports_the_stored_schedule() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = schedule_rows();
    let mut arns = Vec::new();

    for (index, (case, intervals)) in rows.iter().enumerate() {
        let arn = create(&router, &format!("gw-{index}")).await;
        set_schedule(&engine, &arn, intervals.clone());

        let reply = get_schedule(&router, &arn).await;
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
            reply
                .body
                .get("BandwidthRateLimitIntervals")
                .cloned()
                .unwrap_or(Value::Null),
            expected_intervals(intervals),
            "case {case:?}: the reported intervals must be the stored list in order, got {:?}",
            reply.body
        );
        arns.push(arn);
    }

    // Each record answers from its own schedule: re-reading every ARN after all of them
    // were stored still reports that row's intervals, so no read is served from another
    // record.
    for ((case, intervals), arn) in rows.iter().zip(&arns) {
        let reply = get_schedule(&router, arn).await;
        assert_eq!(
            (
                reply.status,
                reply
                    .body
                    .get("BandwidthRateLimitIntervals")
                    .cloned()
                    .unwrap_or(Value::Null)
            ),
            (200, expected_intervals(intervals)),
            "case {case:?}: each ARN must report its own schedule, got {} {:?}",
            reply.status,
            reply.body
        );
    }
}

/// A well-formed `GatewayArn` the store holds no gateway under is refused with the
/// modelled `ResourceNotFoundException` — HTTP 400 carrying `ErrorCode` `GatewayNotFound`
/// and the fixed sentence this service states for a missing gateway, the way every other
/// gateway operation of this service refuses one — whatever the ARN's partition, resource
/// type, id or length.
#[tokio::test]
async fn an_arn_with_no_stored_gateway_is_refused_as_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway that really exists, so a refusal below is about the ARN asked for and not
    // about an empty store.
    let live = create(&router, "gw-live").await;
    set_schedule(
        &engine,
        &live,
        vec![interval(3, 0, 4, 0, &[2], Some(204_800))],
    );

    let rows: Vec<(&str, String)> = vec![
        (
            "a gateway ARN that was never created",
            never_created_arn(),
        ),
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
        let reply = get_schedule(&router, arn).await;
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
    }

    // The record that does exist still reports its schedule, and a record the store no
    // longer holds is refused exactly like one it never held.
    let found = get_schedule(&router, &live).await;
    assert_eq!(
        (found.status, found.str_at("/GatewayArn")),
        (200, Some(live.as_str())),
        "the stored gateway must still report its schedule, got {} {:?}",
        found.status,
        found.body
    );
    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = get_schedule(&router, &live).await;
    assert_eq!(
        (gone.status, gone.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "a deleted gateway must be refused like one that never existed, got {} {:?}",
        gone.status,
        gone.body
    );
}

/// A `GatewayArn` outside the modelled constraints never reaches the lookup: it is refused
/// with HTTP 400 `ValidationException`, one past each published bound of `GatewayArn`
/// (`@length(min: 50, max: 180)`, pattern
/// `^arn:(aws|aws-cn|aws-us-gov):backup-gateway(:[a-zA-Z-0-9]+){3}\/[a-zA-Z-0-9]+$`).
#[tokio::test]
async fn get_bandwidth_rate_limit_schedule_refuses_an_arn_outside_the_modelled_constraints() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows: Vec<(&str, Value)> = vec![
        (
            "one character under the 50-character minimum",
            json!({ "GatewayArn": arn_of_length(49) }),
        ),
        (
            "one character over the 180-character maximum",
            json!({ "GatewayArn": arn_of_length(181) }),
        ),
        (
            "a partition the pattern does not accept",
            json!({ "GatewayArn": arn_in("aws-iso", "gateway", "bgw-0000000000000003") }),
        ),
        (
            "another service's ARN",
            json!({ "GatewayArn": format!(
                "arn:aws:backup:{REGION}:{}:gateway/bgw-0000000000000004",
                engine::EMULATOR_ACCOUNT_ID
            ) }),
        ),
        ("no GatewayArn at all", json!({})),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "GetBandwidthRateLimitSchedule", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an ARN outside the modelled constraints must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
    }

    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .gateway_count(),
        0,
        "a refused request must not have created anything"
    );
}

/// Reading the schedule changes nothing a later caller sees: repeated reads agree, the
/// persisted record is untouched — including a gateway that has no schedule, which does
/// not acquire one by being read — and the read neither adds nor drops a record.
#[tokio::test]
async fn reading_the_schedule_never_changes_what_a_later_caller_sees() {
    let engine = build_engine();
    let router = build_router(&engine);

    let unscheduled = create(&router, "gw-unscheduled").await;
    let scheduled = create(&router, "gw-scheduled").await;
    let stored = vec![
        interval(8, 0, 11, 59, &[1, 3, 5], Some(51_200)),
        interval(20, 0, 23, 59, &[0, 6], None),
    ];
    set_schedule(&engine, &scheduled, stored.clone());

    for arn in [&unscheduled, &scheduled] {
        let before = persisted_intervals(&engine, arn);
        let mut bodies = Vec::new();
        for read in 0..3 {
            let reply = get_schedule(&router, arn).await;
            assert_eq!(
                reply.status, 200,
                "read {read} of {arn}: the stored schedule must read back, got {} {:?}",
                reply.status, reply.body
            );
            bodies.push(reply.body);
        }
        assert!(
            bodies.windows(2).all(|pair| pair[0] == pair[1]),
            "repeated reads of {arn} must agree, got {bodies:?}"
        );
        assert_eq!(
            persisted_intervals(&engine, arn),
            before,
            "a read of {arn} must leave the persisted schedule exactly as it was"
        );
    }

    // The unscheduled gateway still has nothing scheduled after being read: a read never
    // mints a schedule.
    assert_eq!(
        persisted_intervals(&engine, &unscheduled),
        "[]",
        "a read must not write a schedule onto a gateway that has none"
    );
    assert_eq!(
        service_state(&*engine)
            .expect("the in-memory engine must expose this state")
            .lock()
            .gateway_count(),
        2,
        "a read must not add or drop a record"
    );

    // And the gateway itself still reads back unchanged through its own reader.
    let gateway = call(&router, "GetGateway", &json!({ "GatewayArn": scheduled })).await;
    assert_eq!(
        (gateway.status, gateway.str_at("/Gateway/GatewayArn")),
        (200, Some(scheduled.as_str())),
        "the gateway itself must still read back, got {} {:?}",
        gateway.status,
        gateway.body
    );

    // A schedule a later write replaces is what every later read reports: the response
    // follows the store, not the first answer.
    let replacement = vec![interval(12, 0, 12, 30, &[4], Some(8_000_000_000_000))];
    set_schedule(&engine, &scheduled, replacement.clone());
    let reply = get_schedule(&router, &scheduled).await;
    assert_eq!(
        (
            reply.status,
            reply
                .body
                .get("BandwidthRateLimitIntervals")
                .cloned()
                .unwrap_or(Value::Null)
        ),
        (200, expected_intervals(&replacement)),
        "a read must report the schedule the store now holds, got {} {:?}",
        reply.status,
        reply.body
    );

    // Clearing it reports the empty array again, not the schedule that was there before.
    set_schedule(&engine, &scheduled, vec![]);
    let cleared = get_schedule(&router, &scheduled).await;
    assert_eq!(
        (
            cleared.status,
            cleared
                .body
                .get("BandwidthRateLimitIntervals")
                .cloned()
                .unwrap_or(Value::Null)
        ),
        (200, json!([])),
        "a cleared schedule must read back as the empty array, got {} {:?}",
        cleared.status,
        cleared.body
    );
}
