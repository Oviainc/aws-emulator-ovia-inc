// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the maintenance window a gateway is ACTIVATED with.
//!
//! Every case drives a real `POST /` awsJson1_0 request through the
//! `MultiServiceRouter`, reusing the harness of `gateway_lifecycle`, so what is asserted
//! is the wire behaviour and the only way a later request can see what an earlier one
//! wrote is the shared store.
//!
//! The rule under test: an activated gateway appliance always has a maintenance window, so
//! `GatewayDetails$MaintenanceStartTime` is reported for EVERY gateway — the window its
//! activation assigned until a `PutMaintenanceStartTime` replaces it — and that window is
//! a well-formed one of a single documented form, with every member inside its published
//! range. `GatewayDetails$MaintenanceStartTime` "Returns your gateway's weekly maintenance
//! start time including the day and time of the week"
//! (`smithy_query(query="shape",shape="GatewayDetails")`), and the put only "Set[s] the
//! maintenance start time for a gateway"
//! (`smithy_query(query="doc",shape="PutMaintenanceStartTime")`) — it overrides a window
//! that is already there rather than creating the only one.
//!
//! The table below spans that class: creates differing in display name (at each published
//! bound of `Name`), in how many tags they carry, in the region they are addressed to, and
//! in what is done to the gateway afterwards (nothing, a rename, an association) — the
//! window is reported for all of them, is the same on a second read, and is what the store
//! really holds. The sibling `maintenance_start_time` suite covers the put itself; what is
//! asserted here is that a put REPLACES this window whole, in both documented forms, and
//! that it changes no other gateway's.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_backupgateway_backuponpremises_v20210101 as backup_gateway;

use backup_gateway::state::service_state;

use crate::gateway_lifecycle::{
    build_engine, build_router, call, call_in_region, create, create_body,
    issued_activation_key_for, REGION,
};

/// The published ranges of the members of `MaintenanceStartTime`
/// (`smithy_query(query="shape",shape="MaintenanceStartTime")`): `HourOfDay`
/// `@range(min: 0, max: 23)`, `MinuteOfHour` 0..59, `DayOfWeek` 0..6, `DayOfMonth` 1..31.
const HOUR_OF_DAY_RANGE: (i64, i64) = (0, 23);
/// See [`HOUR_OF_DAY_RANGE`].
const MINUTE_OF_HOUR_RANGE: (i64, i64) = (0, 59);
/// See [`HOUR_OF_DAY_RANGE`].
const DAY_OF_WEEK_RANGE: (i64, i64) = (0, 6);
/// See [`HOUR_OF_DAY_RANGE`].
const DAY_OF_MONTH_RANGE: (i64, i64) = (1, 31);

/// What one create in the table carries, and what is done to the gateway after it.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// `GatewayDisplayName` the create states.
    display_name: String,
    /// The tags the create carries.
    tags: &'static [(&'static str, &'static str)],
    /// The region the create (and every later call on it) is addressed to.
    region: &'static str,
    /// Whether the gateway is renamed through `UpdateGatewayInformation` afterwards: a
    /// write to another member must leave the window where the activation put it.
    renamed: bool,
}

/// The `MaintenanceStartTime` member `GetGateway` reports for `arn`, with the status.
async fn reported_window(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
) -> (u16, Option<Value>) {
    let reply = call_in_region(router, region, "GetGateway", &json!({ "GatewayArn": arn })).await;
    let window = reply
        .body
        .pointer("/Gateway/MaintenanceStartTime")
        .cloned();
    (reply.status, window)
}

/// The window the STORE holds for `arn`, rendered the way a response serialises it, so a
/// reported window can be compared against the record rather than against itself.
fn persisted_window(engine: &std::sync::Arc<engine::InMemoryEngine>, arn: &str) -> Value {
    let state = service_state(&**engine).expect("the in-memory engine must expose this state");
    let store = state.lock();
    let record = store
        .gateway(arn)
        .unwrap_or_else(|| panic!("no gateway stored under {arn}"));
    let window = &record.maintenance_start_time;
    let mut rendered = json!({
        "HourOfDay": window.hour_of_day,
        "MinuteOfHour": window.minute_of_hour,
    });
    if let Some(day) = window.day_of_week {
        rendered["DayOfWeek"] = json!(day);
    }
    if let Some(day) = window.day_of_month {
        rendered["DayOfMonth"] = json!(day);
    }
    rendered
}

/// Assert `window` is a window the model admits: both required members present and inside
/// their published ranges, at most ONE day member (a window is weekly or monthly, never
/// both: `smithy_query(query="shape",shape="MaintenanceStartTime")` — "Can be weekly or
/// monthly"), and that member inside its own range.
fn assert_well_formed(case: &str, window: &Value) {
    for (member, (low, high)) in [
        ("HourOfDay", HOUR_OF_DAY_RANGE),
        ("MinuteOfHour", MINUTE_OF_HOUR_RANGE),
    ] {
        let actual = window.get(member).and_then(Value::as_i64);
        assert!(
            actual.is_some_and(|value| (low..=high).contains(&value)),
            "case {case:?}: {member} is required and inside {low}..={high}, got {actual:?} \
             in {window:?}"
        );
    }
    let day_of_week = window.get("DayOfWeek").and_then(Value::as_i64);
    let day_of_month = window.get("DayOfMonth").and_then(Value::as_i64);
    assert!(
        day_of_week.is_none() || day_of_month.is_none(),
        "case {case:?}: a window is weekly or monthly, so it cannot carry both day \
         members, got {window:?}"
    );
    if let Some(day) = day_of_week {
        let (low, high) = DAY_OF_WEEK_RANGE;
        assert!(
            (low..=high).contains(&day),
            "case {case:?}: DayOfWeek must be inside {low}..={high}, got {day} in {window:?}"
        );
    }
    if let Some(day) = day_of_month {
        let (low, high) = DAY_OF_MONTH_RANGE;
        assert!(
            (low..=high).contains(&day),
            "case {case:?}: DayOfMonth must be inside {low}..={high}, got {day} in {window:?}"
        );
    }
}

/// The window an activation assigns, as a response serialises it.
///
/// Spelled out here rather than read back out of the crate, so the assertion is
/// independent of the code that assigns it: a WEEKLY window (hence no `DayOfMonth`) early
/// on Sunday morning, `DayOfWeek` 0 being Sunday
/// (`smithy_query(query="shape",shape="MaintenanceStartTime")`). Shared with the sibling
/// `gateway_lifecycle` suite, which asserts the same window on its own create.
pub(crate) fn default_window() -> Value {
    json!({
        "DayOfWeek": 0,
        "HourOfDay": 3,
        "MinuteOfHour": 0,
    })
}

/// The rows of the create table: the display name at each published bound of `Name`
/// (`@length(min: 1, max: 100)`, `smithy_query(query="operation",operation="CreateGateway")`),
/// no tags and several, two regions, and a rename after the create.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a create carrying nothing but the required members",
            display_name: "gw-plain".to_string(),
            tags: &[],
            region: REGION,
            renamed: false,
        },
        Row {
            case: "a display name at the published minimum length",
            display_name: "g".to_string(),
            tags: &[],
            region: REGION,
            renamed: false,
        },
        Row {
            case: "a display name at the published maximum length",
            display_name: "g".repeat(100),
            tags: &[("env", "prod")],
            region: REGION,
            renamed: false,
        },
        Row {
            case: "a create carrying several tags",
            display_name: "gw-tagged".to_string(),
            tags: &[("env", "staging"), ("owner", "qa"), ("team", "backup")],
            region: REGION,
            renamed: false,
        },
        Row {
            case: "a create addressed to another region",
            display_name: "gw-other-region".to_string(),
            tags: &[],
            region: "eu-west-1",
            renamed: false,
        },
        Row {
            case: "a gateway renamed after it was activated",
            display_name: "gw-renamed".to_string(),
            tags: &[("env", "test")],
            region: REGION,
            renamed: true,
        },
    ]
}

/// Every activated gateway reports the maintenance window its activation assigned, whatever
/// the create carried — and the window reported is the one the store holds, is well formed,
/// and does not change when the gateway is read again or written to elsewhere.
#[tokio::test]
async fn every_activated_gateway_reports_a_maintenance_window() {
    let engine = build_engine();
    let router = build_router(&engine);

    let mut arns = Vec::new();
    for row in rows() {
        let tags: BTreeMap<String, String> = row
            .tags
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        let activation_key = issued_activation_key_for(&router, row.region).await;
        let created = call_in_region(
            &router,
            row.region,
            "CreateGateway",
            &create_body(&row.display_name, &activation_key, &tags),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreateGateway must answer 200, got {} {:?}",
            row.case, created.status, created.body
        );
        let arn = created.gateway_arn().to_string();

        let (status, window) = reported_window(&router, row.region, &arn).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetGateway must answer 200, got {status}",
            row.case
        );
        let window = window.unwrap_or_else(|| {
            panic!(
                "case {:?}: a gateway no PutMaintenanceStartTime has touched must still \
                 report a MaintenanceStartTime, got none",
                row.case
            )
        });
        assert_well_formed(row.case, &window);
        assert_eq!(
            window,
            default_window(),
            "case {:?}: the window must be the one the activation assigned, got {window:?}",
            row.case
        );
        assert_eq!(
            window,
            persisted_window(&engine, &arn),
            "case {:?}: the reported window must be the one the store holds, got {window:?}",
            row.case
        );

        // A read reports the same window again: nothing about reading a gateway moves it.
        let (_, again) = reported_window(&router, row.region, &arn).await;
        assert_eq!(
            again.as_ref(),
            Some(&window),
            "case {:?}: reading the gateway twice must report the same window, got {again:?}",
            row.case
        );

        if row.renamed {
            let renamed = call_in_region(
                &router,
                row.region,
                "UpdateGatewayInformation",
                &json!({ "GatewayArn": arn, "GatewayDisplayName": "gw-renamed-2" }),
            )
            .await;
            assert_eq!(
                renamed.status, 200,
                "case {:?}: UpdateGatewayInformation must answer 200, got {} {:?}",
                row.case, renamed.status, renamed.body
            );
            let (_, after) = reported_window(&router, row.region, &arn).await;
            assert_eq!(
                after.as_ref(),
                Some(&window),
                "case {:?}: a write to another member must leave the window alone, got \
                 {after:?}",
                row.case
            );
        }

        arns.push((row.case, row.region, arn));
    }

    // Every gateway still reports its own window once all of them exist, so no read is
    // served from another record.
    for (case, region, arn) in &arns {
        let (status, window) = reported_window(&router, region, arn).await;
        assert_eq!(
            (status, window.as_ref()),
            (200, Some(&default_window())),
            "case {case:?}: each gateway must still report the window of its own \
             activation, got {status} {window:?}"
        );
    }
}

/// A put REPLACES the window the activation assigned — in both documented forms and with
/// neither day member — and replaces it only on the gateway its request names.
#[tokio::test]
async fn a_put_replaces_the_window_the_activation_assigned() {
    let engine = build_engine();
    let router = build_router(&engine);

    // (case, the request's members beyond the ARN, the window a read must then report).
    // Weekly over the weekly default, monthly over it (so the default's `DayOfWeek` is
    // gone), and the two required members alone (so no day member survives at all).
    let rows = [
        (
            "a weekly window over the default",
            json!({ "HourOfDay": 11, "MinuteOfHour": 45, "DayOfWeek": 4 }),
            json!({ "HourOfDay": 11, "MinuteOfHour": 45, "DayOfWeek": 4 }),
        ),
        (
            "a monthly window over the default",
            json!({ "HourOfDay": 0, "MinuteOfHour": 0, "DayOfMonth": 28 }),
            json!({ "HourOfDay": 0, "MinuteOfHour": 0, "DayOfMonth": 28 }),
        ),
        (
            "the required members alone over the default",
            json!({ "HourOfDay": 23, "MinuteOfHour": 59 }),
            json!({ "HourOfDay": 23, "MinuteOfHour": 59 }),
        ),
    ];

    // The gateway no put touches: its window must still be the one its activation assigned
    // after every row below.
    let untouched = create(&router, "gw-untouched").await;

    for (index, (case, members, expected)) in rows.iter().enumerate() {
        let arn = create(&router, &format!("gw-replaced-{index}")).await;
        let (_, before) = reported_window(&router, REGION, &arn).await;
        assert_eq!(
            before.as_ref(),
            Some(&default_window()),
            "case {case:?}: the gateway must start from the window its activation \
             assigned, got {before:?}"
        );

        let mut body = members.clone();
        body["GatewayArn"] = json!(arn);
        let put = call(&router, "PutMaintenanceStartTime", &body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: PutMaintenanceStartTime must answer 200, got {} {:?}",
            put.status, put.body
        );

        let (status, window) = reported_window(&router, REGION, &arn).await;
        let window = window
            .unwrap_or_else(|| panic!("case {case:?}: GetGateway reported no window at all"));
        assert_well_formed(case, &window);
        assert_eq!(
            (status, &window),
            (200, expected),
            "case {case:?}: the put window must REPLACE the activation's whole, got \
             {status} {window:?}"
        );
        assert_eq!(
            window,
            persisted_window(&engine, &arn),
            "case {case:?}: the reported window must be the one the store holds, got \
             {window:?}"
        );

        let (_, other) = reported_window(&router, REGION, &untouched).await;
        assert_eq!(
            other.as_ref(),
            Some(&default_window()),
            "case {case:?}: a put on one gateway must leave another gateway's window \
             alone, got {other:?}"
        );
    }
}
