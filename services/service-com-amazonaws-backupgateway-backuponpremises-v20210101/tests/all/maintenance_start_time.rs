// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutMaintenanceStartTime`.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: `PutMaintenanceStartTime` SETS the maintenance window of the
//! gateway its `GatewayArn` resolves to — it persists every member the request states,
//! REPLACES whatever window that gateway carried (so a day member the request does not
//! state is not on the record afterwards either), answers with that record's own ARN, and
//! leaves everything else on the record alone; `GetGateway` then reports exactly the
//! window that was written. A well-formed `GatewayArn` the store holds no gateway under is
//! refused with the modelled `ResourceNotFoundException`, and a request naming BOTH
//! `DayOfWeek` and `DayOfMonth` is refused with the modelled `ValidationException` —
//! a window is weekly or monthly, never both
//! (`smithy_query(query="shape",shape="MaintenanceStartTime")`: "Can be weekly or
//! monthly") — with nothing written either way.
//!
//! The tables below span that class: windows differing in which day member is set
//! (weekly, monthly, neither), at each published bound of every member
//! (`HourOfDay` `@range(min: 0, max: 23)`, `MinuteOfHour` 0..59, `DayOfWeek` 0..6,
//! `DayOfMonth` 1..31) and over the WHOLE day space of both forms; ARNs differing in
//! partition, resource type, id and length, including each published bound of `GatewayArn`
//! and one past it; and values one past each published bound of every time member.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::{service_state, MAINTENANCE_START_TIME_INVALID};

use crate::bandwidth_rate_limit_schedule::{arn_in, arn_of_length};
use crate::gateway_lifecycle::{
    build_engine, build_router, call, create, create_body, gateway_count, issued_activation_key,
    never_created_arn, Reply,
};

/// One maintenance window as the request states it.
///
/// The members are the modelled `PutMaintenanceStartTimeInput`'s
/// (`smithy_query(query="operation",operation="PutMaintenanceStartTime")`): `HourOfDay`
/// and `MinuteOfHour` required, `DayOfWeek` and `DayOfMonth` optional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    hour_of_day: i32,
    minute_of_hour: i32,
    day_of_week: Option<i32>,
    day_of_month: Option<i32>,
}

impl Window {
    /// A WEEKLY window: the day of the week is stated, the day of the month is not.
    fn weekly(day_of_week: i32, hour_of_day: i32, minute_of_hour: i32) -> Self {
        Self {
            hour_of_day,
            minute_of_hour,
            day_of_week: Some(day_of_week),
            day_of_month: None,
        }
    }

    /// A MONTHLY window: the day of the month is stated, the day of the week is not.
    fn monthly(day_of_month: i32, hour_of_day: i32, minute_of_hour: i32) -> Self {
        Self {
            hour_of_day,
            minute_of_hour,
            day_of_week: None,
            day_of_month: Some(day_of_month),
        }
    }

    /// A window stating only the two REQUIRED members: both day members are optional, so
    /// a request may carry neither.
    fn time_only(hour_of_day: i32, minute_of_hour: i32) -> Self {
        Self {
            hour_of_day,
            minute_of_hour,
            day_of_week: None,
            day_of_month: None,
        }
    }

    /// The `PutMaintenanceStartTime` request body for `arn`, carrying exactly the members
    /// this window states.
    fn request(&self, arn: &str) -> Value {
        let mut body = json!({
            "GatewayArn": arn,
            "HourOfDay": self.hour_of_day,
            "MinuteOfHour": self.minute_of_hour,
        });
        if let Some(day) = self.day_of_week {
            body["DayOfWeek"] = json!(day);
        }
        if let Some(day) = self.day_of_month {
            body["DayOfMonth"] = json!(day);
        }
        body
    }

    /// The `MaintenanceStartTime` object a read must report for this window: the two
    /// required members always, and a day member only where the window states one.
    fn expected(&self) -> Value {
        let mut rendered = json!({
            "HourOfDay": self.hour_of_day,
            "MinuteOfHour": self.minute_of_hour,
        });
        if let Some(day) = self.day_of_week {
            rendered["DayOfWeek"] = json!(day);
        }
        if let Some(day) = self.day_of_month {
            rendered["DayOfMonth"] = json!(day);
        }
        rendered
    }
}

async fn put(router: &MultiServiceRouter, arn: &str, window: &Window) -> Reply {
    call(router, "PutMaintenanceStartTime", &window.request(arn)).await
}

/// The `Gateway` object `GetGateway` reports for `arn`, with the status it answered.
async fn gateway_object(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let reply = call(router, "GetGateway", &json!({ "GatewayArn": arn })).await;
    let gateway = reply
        .body
        .get("Gateway")
        .cloned()
        .unwrap_or(Value::Null);
    (reply.status, gateway)
}

/// The `MaintenanceStartTime` member `GetGateway` reports for `arn`, with the status.
async fn reported_window(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    let (status, gateway) = gateway_object(router, arn).await;
    let window = gateway
        .get("MaintenanceStartTime")
        .cloned()
        .unwrap_or(Value::Null);
    (status, window)
}

/// The window PERSISTED on the record under `arn`, rendered for comparison: what a
/// refused request must not have changed.
fn persisted_window(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> String {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let record = store
        .gateway(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"));
    format!("{:?}", record.maintenance_start_time)
}

/// The rows of the window table: both forms the model documents and neither, at each
/// published bound of every member.
fn window_rows() -> Vec<(&'static str, Window)> {
    vec![
        ("a weekly window in the middle of the week", Window::weekly(3, 9, 30)),
        // `DayOfWeek` `@range(min: 0, max: 6)`, `HourOfDay` 0..23, `MinuteOfHour` 0..59
        // (`smithy_query(query="operation",operation="PutMaintenanceStartTime")`).
        ("every weekly member at its published minimum", Window::weekly(0, 0, 0)),
        ("every weekly member at its published maximum", Window::weekly(6, 23, 59)),
        // `DayOfMonth` `@range(min: 1, max: 31)`.
        ("a monthly window on the first of the month", Window::monthly(1, 0, 1)),
        (
            // The ordinal the stored shape's own member doc calls the last day of the
            // month (`smithy_query(query="doc",shape="MaintenanceStartTime",
            // member="DayOfMonth")`).
            "a monthly window on the 28th",
            Window::monthly(28, 12, 45),
        ),
        (
            "a monthly window at the published maximum of DayOfMonth",
            Window::monthly(31, 23, 59),
        ),
        ("no day member at all, only the required time", Window::time_only(17, 5)),
        ("no day member at all, at midnight", Window::time_only(0, 0)),
    ]
}

/// Every member the request states is persisted and reported back, for windows spanning
/// both documented forms and each published bound of every member — and each gateway
/// answers from its OWN record.
#[tokio::test]
async fn put_maintenance_start_time_persists_every_window_it_is_given() {
    let engine = build_engine();
    let router = build_router(&engine);

    let rows = window_rows();
    let mut arns = Vec::new();

    for (index, (case, window)) in rows.iter().enumerate() {
        let arn = create(&router, &format!("gw-{index}")).await;

        let reply = put(&router, &arn, window).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.str_at("/GatewayArn"),
            Some(arn.as_str()),
            "case {case:?}: the reported ARN must be the record's own, got {:?}",
            reply.body
        );

        let (status, reported) = reported_window(&router, &arn).await;
        assert_eq!(
            (status, &reported),
            (200, &window.expected()),
            "case {case:?}: GetGateway must report exactly the window that was put, \
             got {status} {reported:?}"
        );
        arns.push(arn);
    }

    // Each record answers from its own window: re-reading every ARN after all of them
    // were written still reports that row's window, so no read is served from another
    // record.
    for ((case, window), arn) in rows.iter().zip(&arns) {
        let (status, reported) = reported_window(&router, arn).await;
        assert_eq!(
            (status, &reported),
            (200, &window.expected()),
            "case {case:?}: each ARN must report its own window, got {status} {reported:?}"
        );
    }
}

/// The whole day space of both documented forms round-trips: every `DayOfWeek` the model
/// accepts and every `DayOfMonth` it accepts, each at a different hour and minute, is
/// reported back exactly as it was put — and each put REPLACES the window the one before
/// it wrote.
#[tokio::test]
async fn every_day_of_the_week_and_of_the_month_round_trips() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-day-space").await;

    // `DayOfWeek` 0..6 and `DayOfMonth` 1..31 — every value inside the published ranges,
    // paired with an hour and a minute that differ per row so no two cases are alike.
    let weekly = (0..=6).map(|day| Window::weekly(day, day * 3, day * 8));
    let monthly = (1..=31).map(|day| Window::monthly(day, day % 24, (day * 2) % 60));

    for window in weekly.chain(monthly) {
        let reply = put(&router, &arn, &window).await;
        assert_eq!(
            reply.status, 200,
            "case {window:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
            reply.status, reply.body
        );

        let (status, reported) = reported_window(&router, &arn).await;
        assert_eq!(
            (status, &reported),
            (200, &window.expected()),
            "case {window:?}: the reported window must be the one just put, \
             got {status} {reported:?}"
        );
    }
}

/// A later put REPLACES the window and touches nothing else: the day member the new
/// request does not state is gone, the same put twice leaves the record as one call left
/// it, and every other member of the record — and the gateway's tags — read back
/// unchanged.
#[tokio::test]
async fn a_later_put_replaces_the_window_and_touches_nothing_else() {
    let engine = build_engine();
    let router = build_router(&engine);

    let tags: BTreeMap<String, String> = [("env".to_string(), "test".to_string())]
        .into_iter()
        .collect();
    let created = call(
        &router,
        "CreateGateway",
        &create_body(
            "gw-replace",
            &issued_activation_key(&router).await,
            &tags,
        ),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateGateway must answer 200, got {} {:?}",
        created.status, created.body
    );
    let arn = created.gateway_arn().to_string();

    // Every row replaces the window the row before it wrote: weekly over weekly, monthly
    // over weekly (dropping `DayOfWeek`), the two required members alone (dropping both
    // day members), and weekly again over that.
    let rows = vec![
        ("weekly, the first window ever set", Window::weekly(2, 4, 15)),
        ("weekly replaced by another weekly", Window::weekly(5, 20, 0)),
        ("weekly replaced by a monthly window", Window::monthly(14, 1, 30)),
        ("a monthly window replaced by time alone", Window::time_only(6, 45)),
        ("time alone replaced by a weekly window", Window::weekly(0, 23, 59)),
    ];

    for (case, window) in &rows {
        let reply = put(&router, &arn, window).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
            reply.status, reply.body
        );
        let (status, reported) = reported_window(&router, &arn).await;
        assert_eq!(
            (status, &reported),
            (200, &window.expected()),
            "case {case:?}: the replacement must be the only window reported, \
             got {status} {reported:?}"
        );
    }

    // The same put again leaves the record exactly as one call left it, and reports the
    // same body.
    let (_, before) = gateway_object(&router, &arn).await;
    let last = rows.last().expect("the table has rows").1;
    let again = put(&router, &arn, &last).await;
    assert_eq!(
        (again.status, again.str_at("/GatewayArn")),
        (200, Some(arn.as_str())),
        "putting the same window again must answer 200 with the record's ARN, got {} {:?}",
        again.status,
        again.body
    );
    let (_, after) = gateway_object(&router, &arn).await;
    assert_eq!(
        after, before,
        "putting the same window again must leave the reported gateway unchanged"
    );

    // Nothing but the window moved: every other member of the record is what the create
    // left there.
    let mut without_window = after.clone();
    without_window
        .as_object_mut()
        .expect("the Gateway member must be an object")
        .remove("MaintenanceStartTime");
    assert_eq!(
        without_window,
        json!({
            "GatewayArn": arn,
            "GatewayDisplayName": "gw-replace",
            "GatewayType": backup_gateway::state::BACKUP_VM,
            "LastSeenTime": without_window.get("LastSeenTime").cloned().unwrap_or(Value::Null),
            "SoftwareVersion": backup_gateway::state::GATEWAY_SOFTWARE_VERSION,
        }),
        "a put of the maintenance window must leave every other member of the record \
         alone, got {without_window:?}"
    );

    // And the gateway's tags are untouched: a put of the window is not a tagging call.
    let listed = call(
        &router,
        "ListTagsForResource",
        &json!({ "ResourceArn": arn }),
    )
    .await;
    assert_eq!(
        (listed.status, listed.tags()),
        (200, tags),
        "a put of the maintenance window must leave the tags alone, got {} {:?}",
        listed.status,
        listed.body
    );
    assert_eq!(
        gateway_count(&engine),
        1,
        "a put must not add or drop a record"
    );
}

/// A request naming BOTH day members is refused: a maintenance start time is weekly or
/// monthly, never both, so the modelled `ValidationException` is answered — HTTP 400, the
/// status that shape's `@httpError` declares, with the `ErrorCode` this service names the
/// condition by — and the window the record already carried is left exactly as it was.
#[tokio::test]
async fn naming_both_day_members_is_refused_and_nothing_is_written() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-both-days").await;

    // A window that really was set, so a refusal below is about the request and not about
    // a record with nothing on it.
    let stored = Window::weekly(1, 8, 0);
    let accepted = put(&router, &arn, &stored).await;
    assert_eq!(
        accepted.status, 200,
        "the first put must answer 200, got {} {:?}",
        accepted.status, accepted.body
    );

    // Rows spanning the day space of the refusal: each published bound of both members,
    // and a pair in the middle of both ranges.
    let rows = vec![
        (
            "both day members at their published minimums",
            Window {
                hour_of_day: 0,
                minute_of_hour: 0,
                day_of_week: Some(0),
                day_of_month: Some(1),
            },
        ),
        (
            "both day members at their published maximums",
            Window {
                hour_of_day: 23,
                minute_of_hour: 59,
                day_of_week: Some(6),
                day_of_month: Some(31),
            },
        ),
        (
            "both day members in the middle of their ranges",
            Window {
                hour_of_day: 12,
                minute_of_hour: 30,
                day_of_week: Some(3),
                day_of_month: Some(15),
            },
        ),
    ];

    let persisted = persisted_window(&engine, &arn);
    for (case, window) in &rows {
        let reply = put(&router, &arn, window).await;
        assert_eq!(
            (
                reply.status,
                reply.code.as_deref(),
                reply.str_at("/ErrorCode")
            ),
            (
                400,
                Some("ValidationException"),
                Some(MAINTENANCE_START_TIME_INVALID)
            ),
            "case {case:?}: a window that is both weekly and monthly must be refused with \
             400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_window(&engine, &arn),
            persisted,
            "case {case:?}: a refused request must leave the persisted window as it was"
        );
    }

    // The window the accepted call wrote is still the one every reader reports.
    let (status, reported) = reported_window(&router, &arn).await;
    assert_eq!(
        (status, &reported),
        (200, &stored.expected()),
        "the window set by the accepted call must survive every refusal, \
         got {status} {reported:?}"
    );
}

/// A well-formed `GatewayArn` the store holds no gateway under is refused with the
/// modelled `ResourceNotFoundException` — HTTP 400 carrying `ErrorCode` `GatewayNotFound`
/// and the fixed sentence this service states for a missing gateway, the way every other
/// gateway operation of this service
/// refuses one — whatever the ARN's partition, resource type, id or length, and nothing is
/// written for it.
#[tokio::test]
async fn an_arn_with_no_stored_gateway_is_refused_as_not_found() {
    let engine = build_engine();
    let router = build_router(&engine);

    // A gateway that really exists, with a window of its own, so a refusal below is about
    // the ARN asked for and not about an empty store.
    let live = create(&router, "gw-live").await;
    let stored = Window::monthly(7, 3, 20);
    let accepted = put(&router, &live, &stored).await;
    assert_eq!(
        accepted.status, 200,
        "the put on the live gateway must answer 200, got {} {:?}",
        accepted.status, accepted.body
    );

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

    let window = Window::weekly(4, 11, 11);
    for (case, arn) in &rows {
        let reply = put(&router, arn, &window).await;
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
        assert_eq!(
            gateway_count(&engine),
            1,
            "case {case:?}: a refused request must not create a record"
        );
    }

    // The record that does exist still reports its own window, and a record the store no
    // longer holds is refused exactly like one it never held.
    let (status, reported) = reported_window(&router, &live).await;
    assert_eq!(
        (status, &reported),
        (200, &stored.expected()),
        "the stored gateway must still report its window, got {status} {reported:?}"
    );
    let deleted = call(&router, "DeleteGateway", &json!({ "GatewayArn": live })).await;
    assert_eq!(
        deleted.status, 200,
        "DeleteGateway must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );
    let gone = put(&router, &live, &window).await;
    assert_eq!(
        (gone.status, gone.code.as_deref()),
        (400, Some("ResourceNotFoundException")),
        "a deleted gateway must be refused like one that never existed, got {} {:?}",
        gone.status,
        gone.body
    );
}

/// A request outside the modelled constraints never reaches the store: it is refused with
/// HTTP 400 `ValidationException`, one past each published bound of every member, and the
/// window the record carries is left exactly as it was.
#[tokio::test]
async fn a_request_outside_the_modelled_constraints_is_refused() {
    let engine = build_engine();
    let router = build_router(&engine);
    let arn = create(&router, "gw-constraints").await;

    let stored = Window::weekly(2, 2, 2);
    let accepted = put(&router, &arn, &stored).await;
    assert_eq!(
        accepted.status, 200,
        "the first put must answer 200, got {} {:?}",
        accepted.status, accepted.body
    );
    let persisted = persisted_window(&engine, &arn);

    // One past each published bound: `HourOfDay` 0..23, `MinuteOfHour` 0..59, `DayOfWeek`
    // 0..6, `DayOfMonth` 1..31, `GatewayArn` `@length(min: 50, max: 180)` with its
    // pattern — plus each required member missing.
    let rows: Vec<(&str, Value)> = vec![
        (
            "one under the minimum of HourOfDay",
            json!({ "GatewayArn": arn, "HourOfDay": -1, "MinuteOfHour": 0 }),
        ),
        (
            "one over the maximum of HourOfDay",
            json!({ "GatewayArn": arn, "HourOfDay": 24, "MinuteOfHour": 0 }),
        ),
        (
            "one under the minimum of MinuteOfHour",
            json!({ "GatewayArn": arn, "HourOfDay": 0, "MinuteOfHour": -1 }),
        ),
        (
            "one over the maximum of MinuteOfHour",
            json!({ "GatewayArn": arn, "HourOfDay": 0, "MinuteOfHour": 60 }),
        ),
        (
            "one under the minimum of DayOfWeek",
            json!({ "GatewayArn": arn, "HourOfDay": 1, "MinuteOfHour": 1, "DayOfWeek": -1 }),
        ),
        (
            "one over the maximum of DayOfWeek",
            json!({ "GatewayArn": arn, "HourOfDay": 1, "MinuteOfHour": 1, "DayOfWeek": 7 }),
        ),
        (
            "one under the minimum of DayOfMonth",
            json!({ "GatewayArn": arn, "HourOfDay": 1, "MinuteOfHour": 1, "DayOfMonth": 0 }),
        ),
        (
            "one over the maximum of DayOfMonth",
            json!({ "GatewayArn": arn, "HourOfDay": 1, "MinuteOfHour": 1, "DayOfMonth": 32 }),
        ),
        (
            "no HourOfDay at all",
            json!({ "GatewayArn": arn, "MinuteOfHour": 0 }),
        ),
        (
            "no MinuteOfHour at all",
            json!({ "GatewayArn": arn, "HourOfDay": 0 }),
        ),
        (
            "no GatewayArn at all",
            json!({ "HourOfDay": 0, "MinuteOfHour": 0 }),
        ),
        (
            "one character under the 50-character minimum of GatewayArn",
            json!({ "GatewayArn": arn_of_length(49), "HourOfDay": 0, "MinuteOfHour": 0 }),
        ),
        (
            "one character over the 180-character maximum of GatewayArn",
            json!({ "GatewayArn": arn_of_length(181), "HourOfDay": 0, "MinuteOfHour": 0 }),
        ),
        (
            "a partition the GatewayArn pattern does not accept",
            json!({
                "GatewayArn": arn_in("aws-iso", "gateway", "bgw-0000000000000003"),
                "HourOfDay": 0,
                "MinuteOfHour": 0,
            }),
        ),
    ];

    for (case, body) in &rows {
        let reply = call(&router, "PutMaintenanceStartTime", body).await;
        assert_eq!(
            (reply.status, reply.code.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a request outside the modelled constraints must be refused \
             with 400 ValidationException, got {} {:?}",
            reply.status,
            reply.body
        );
        assert_eq!(
            persisted_window(&engine, &arn),
            persisted,
            "case {case:?}: a refused request must leave the persisted window as it was"
        );
    }

    let (status, reported) = reported_window(&router, &arn).await;
    assert_eq!(
        (status, &reported),
        (200, &stored.expected()),
        "the window set by the accepted call must survive every refusal, \
         got {status} {reported:?}"
    );
}
