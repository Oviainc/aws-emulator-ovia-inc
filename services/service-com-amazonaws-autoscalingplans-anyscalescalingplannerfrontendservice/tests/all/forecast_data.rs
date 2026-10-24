// SPDX-License-Identifier: CC0-1.0

//! `GetScalingPlanResourceForecastData` over the real wire path.
//!
//! Four rules are pinned here, each over a TABLE of requests rather than the one that
//! exposed them:
//!
//! * the forecast WINDOW is checked against its documented limits before any plan is
//!   looked up, so the same window earns the same verdict whether or not the named plan
//!   exists — `EndTime` has to be later than `StartTime`, the window may span at most
//!   `MAX_FORECAST_WINDOW_DAYS` days measured on the two instants the request carries
//!   (and nothing coarser), and `StartTime` may be at most `MAX_FORECAST_HISTORY_DAYS`
//!   days before the current date and time. Nothing bounds how far AHEAD a window reaches;
//! * `ScalingPlanVersion` has exactly one documented valid value, so every other version
//!   is refused with the modelled `ValidationException` (HTTP 400) rather than served;
//! * forecast data is issued for ONE kind of scalable resource — an Auto Scaling group's
//!   desired capacity — so any other `(ServiceNamespace, ScalableDimension)` pair, and a
//!   resource id that does not identify a resource of that kind, is refused with the
//!   modelled `ValidationException` (HTTP 400);
//! * an accepted request answers HTTP 200 with the required `Datapoints` member.
//!
//! Every plan these tests store configures DYNAMIC scaling only — its instruction carries
//! no load metric specification, which is what "configur[es] predictive scaling" — so it
//! has no forecast and its accepted answers carry no data point, whatever the window or
//! the forecast data type. What a plan that DOES configure predictive scaling forecasts is
//! pinned in [`crate::forecast_datapoints`].
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE, MAX_FORECAST_HISTORY_DAYS,
    MAX_FORECAST_WINDOW_DAYS,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, instruction, simple_instructions, stack_source, Emulator,
};

/// The operation under test.
const OPERATION: &str = "GetScalingPlanResourceForecastData";

/// Seconds in a day, for building windows out of the two documented day counts.
const DAY: i64 = 86_400;

/// Seconds in an hour, used only to place a window's endpoints at a KNOWN offset inside
/// one clock hour: the documented widths are measured on the instants themselves, so an
/// hour is a convenient probe size here and not a granularity any rule rounds to.
const HOUR: i64 = 3_600;

/// The Auto Scaling group the stored plan of these tests scales. Its id carries the
/// documented prefix: `ResourceId` "consists of a prefix (autoScalingGroup) followed by
/// the name of a specified Auto Scaling group (my-asg)".
const COVERED_RESOURCE: &str = "autoScalingGroup/my-asg";

/// An Auto Scaling group no stored plan of these tests covers.
const UNCOVERED_RESOURCE: &str = "autoScalingGroup/other-asg";

/// The plan the emulator holds in these tests.
const STORED_PLAN: &str = "forecast plan";

/// A plan name this service does not hold.
const ABSENT_PLAN: &str = "no such plan";

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// How far from a documented bound a row is placed when the verdict depends on the
/// SERVING clock: the service reads its own `now` a few milliseconds after this test
/// builds the request, so a row sits a whole minute inside or past the bound and the
/// elapsed time cannot flip it.
const CLOCK_MARGIN_SECONDS: i64 = 60;

/// Now, as the epoch seconds an `awsJson1_1` timestamp is carried as.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("a host clock at or after the epoch")
        .as_secs() as i64
}

/// The coordinates of one scalable resource: `(ServiceNamespace, ResourceId,
/// ScalableDimension)`.
type Resource<'a> = (&'a str, &'a str, &'a str);

/// The Auto Scaling group resource the stored plan of these tests covers.
const ASG_RESOURCE: Resource<'static> = (
    FORECAST_SERVICE_NAMESPACE,
    COVERED_RESOURCE,
    FORECAST_SCALABLE_DIMENSION,
);

/// One forecast request, with every member the model marks required: a plan name and
/// version, a scalable resource, a forecast data type and the window `[start, end)`.
fn forecast_request(
    plan: &str,
    version: i64,
    resource: Resource<'_>,
    data_type: &str,
    window: (i64, i64),
) -> Value {
    let (namespace, resource_id, dimension) = resource;
    let (start, end) = window;
    json!({
        "ScalingPlanName": plan,
        "ScalingPlanVersion": version,
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "ForecastDataType": data_type,
        "StartTime": start,
        "EndTime": end,
    })
}

/// A forecast request for the Auto Scaling group resource, over `[start, end)`.
fn asg_request(plan: &str, version: i64, data_type: &str, window: (i64, i64)) -> Value {
    forecast_request(plan, version, ASG_RESOURCE, data_type, window)
}

/// An emulator holding one `Active` plan that covers [`COVERED_RESOURCE`].
async fn with_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": STORED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The `Datapoints` member of a successful response, which the model marks required.
///
/// Asserted in two parts, because they are two different claims: the member is PRESENT
/// and carries the modelled list shape (an array), and separately — at each call site —
/// how many data points that list holds.
fn datapoints(body: &Value) -> &Vec<Value> {
    assert!(
        !body["Datapoints"].is_null(),
        "the required Datapoints member must be present, got {body}"
    );
    body.get("Datapoints")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected Datapoints to be a list, got {body}"))
}

/// What the window rules answer for one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// HTTP 200 carrying the required `Datapoints` list, which is empty because the
    /// stored plan configures no predictive scaling.
    Accepted,
    /// HTTP 400 `ValidationException`, because the window spans more than the documented
    /// maximum duration.
    RefusedAsTooWide,
    /// HTTP 400 `ValidationException`, because the start is further than the documented
    /// span before the current date and time.
    RefusedAsTooOld,
    /// HTTP 400 `ValidationException`, because the end is not after the start.
    RefusedAsOutOfOrder,
}

/// The start of the clock hour containing now.
///
/// Every width row below places its two endpoints at a KNOWN offset inside an hour, so a
/// row that used to be accepted only because both endpoints fell in hour-aligned periods
/// seven days apart is exercised whatever time the test happens to run at.
fn current_hour_start() -> i64 {
    now_secs().div_euclid(HOUR) * HOUR
}

/// Assert the verdict of one window on one plan name.
async fn assert_window_verdict(
    emulator: &Emulator,
    label: &str,
    held: &str,
    plan: &str,
    window: (i64, i64),
    verdict: Verdict,
) {
    let body = asg_request(plan, ONLY_VERSION, "CapacityForecast", window);
    let (status, parsed) = emulator.call(OPERATION, &body).await;
    if verdict == Verdict::Accepted {
        assert_eq!(
            status, 200,
            "case {label:?} on {held} ({window:?}): expected HTTP 200, got {status} \
             with {parsed}"
        );
        assert!(
            datapoints(&parsed).is_empty(),
            "case {label:?} on {held} ({window:?}): expected no data points, got {parsed}"
        );
        return;
    }

    assert_eq!(
        (status, error_code(&parsed)),
        (400, "ValidationException"),
        "case {label:?} on {held} ({window:?}): expected HTTP 400 ValidationException, \
         got {status} with {parsed}"
    );
    // The sentence is read only to tell WHICH relation refused the row — the width, the
    // history span or the endpoint order — not because a message text is the rule.
    let message = error_message(&parsed);
    let named = (
        message.contains(&format!("{MAX_FORECAST_WINDOW_DAYS} days")),
        message.contains(&format!("{MAX_FORECAST_HISTORY_DAYS} days")),
    );
    let expected = match verdict {
        Verdict::RefusedAsTooWide => (true, false),
        Verdict::RefusedAsTooOld => (false, true),
        Verdict::Accepted | Verdict::RefusedAsOutOfOrder => (false, false),
    };
    assert_eq!(
        named, expected,
        "case {label:?} on {held} ({window:?}): expected the refusal for {verdict:?}, \
         got {message:?}"
    );
}

#[tokio::test]
async fn the_forecast_window_width_is_measured_on_the_requested_instants() {
    let week = MAX_FORECAST_WINDOW_DAYS * DAY;
    // Rows differ along what the rule generalises over: the WIDTH of the window, the
    // ORDER of its two endpoints, where inside its clock hour each endpoint sits, and
    // whether the window lies ahead of or behind now. The documented maximum duration is
    // probed inside, exactly at, one second past, and far past the bound — including the
    // overshoots that stay inside one hour, which the duration between the instants
    // refuses just the same.
    //
    // `(label, offset of the start inside the current hour, width in seconds, verdict)`.
    let cases: [(&str, i64, i64, Verdict); 17] = [
        ("a one-second window", 600, 1, Verdict::Accepted),
        (
            "a ten-minute window inside a single hour",
            600,
            600,
            Verdict::Accepted,
        ),
        (
            "an hour-long window starting on the hour",
            0,
            HOUR,
            Verdict::Accepted,
        ),
        (
            "a window one second inside the documented maximum width",
            0,
            week - 1,
            Verdict::Accepted,
        ),
        (
            "a window exactly at the documented maximum width",
            0,
            week,
            Verdict::Accepted,
        ),
        (
            "a maximum-width window starting mid-hour",
            1_777,
            week,
            Verdict::Accepted,
        ),
        (
            "a window one second past the documented maximum width",
            0,
            week + 1,
            Verdict::RefusedAsTooWide,
        ),
        (
            "a minute past the maximum width, both endpoints inside one hour",
            800,
            week + 60,
            Verdict::RefusedAsTooWide,
        ),
        (
            "half an hour past the maximum width, both endpoints inside one hour",
            1_000,
            week + 1_800,
            Verdict::RefusedAsTooWide,
        ),
        (
            "the last second of the hour past the maximum width",
            0,
            week + HOUR - 1,
            Verdict::RefusedAsTooWide,
        ),
        (
            "an hour past the maximum width",
            0,
            week + HOUR,
            Verdict::RefusedAsTooWide,
        ),
        (
            "a day past the maximum width",
            0,
            week + DAY,
            Verdict::RefusedAsTooWide,
        ),
        (
            "a window ten days wide",
            800,
            10 * DAY,
            Verdict::RefusedAsTooWide,
        ),
        (
            "an end time equal to the start time",
            600,
            0,
            Verdict::RefusedAsOutOfOrder,
        ),
        (
            "an end time a second before the start time",
            600,
            -1,
            Verdict::RefusedAsOutOfOrder,
        ),
        (
            "an end time half an hour before the start time",
            600,
            -1_800,
            Verdict::RefusedAsOutOfOrder,
        ),
        (
            "an end time a week before the start time",
            600,
            -week,
            Verdict::RefusedAsOutOfOrder,
        ),
    ];

    let emulator = with_stored_plan().await;
    // Every row is checked twice: against the plan the emulator holds and against a name
    // it does not hold. The window verdict is the same either way, which is what "the
    // window is checked before the plan lookup" means.
    for (plan, held) in [(STORED_PLAN, "a stored plan"), (ABSENT_PLAN, "an absent plan")] {
        for (label, start_offset, width, verdict) in cases {
            let start = current_hour_start() + start_offset;
            assert_window_verdict(&emulator, label, held, plan, (start, start + width), verdict)
                .await;
        }
    }
}

#[tokio::test]
async fn a_start_further_back_than_the_documented_span_is_refused() {
    // One width — a day, well inside the documented maximum — at every position the
    // request can put it: ahead of now, around now, just inside the documented span
    // before now, just past it, and far past it on both sides. `StartTime` "can be at
    // most MAX_FORECAST_HISTORY_DAYS days before the current date and time", while
    // `EndTime` "can accept a date and time that is more than two days in the future",
    // so only the backward direction is bounded.
    //
    // `(label, offset of the start from now in seconds, verdict)`.
    let history = MAX_FORECAST_HISTORY_DAYS * DAY;
    let cases: [(&str, i64, Verdict); 14] = [
        ("a window starting now", 0, Verdict::Accepted),
        (
            "a window two days ahead, the documented forecast horizon",
            2 * DAY,
            Verdict::Accepted,
        ),
        ("a window a week ahead", 7 * DAY, Verdict::Accepted),
        ("a window a year ahead", 365 * DAY, Verdict::Accepted),
        ("a window an hour in the past", -HOUR, Verdict::Accepted),
        ("a window a week in the past", -7 * DAY, Verdict::Accepted),
        ("a window thirty days in the past", -30 * DAY, Verdict::Accepted),
        (
            "a window an hour inside the documented span",
            -history + HOUR,
            Verdict::Accepted,
        ),
        (
            "a window a minute inside the documented span",
            -history + CLOCK_MARGIN_SECONDS,
            Verdict::Accepted,
        ),
        (
            "a window a minute past the documented span",
            -history - CLOCK_MARGIN_SECONDS,
            Verdict::RefusedAsTooOld,
        ),
        (
            "a window an hour past the documented span",
            -history - HOUR,
            Verdict::RefusedAsTooOld,
        ),
        (
            "a window a hundred days in the past",
            -100 * DAY,
            Verdict::RefusedAsTooOld,
        ),
        (
            "a window two hundred days in the past",
            -200 * DAY,
            Verdict::RefusedAsTooOld,
        ),
        (
            "a window a year in the past",
            -365 * DAY,
            Verdict::RefusedAsTooOld,
        ),
    ];

    let emulator = with_stored_plan().await;
    for (plan, held) in [(STORED_PLAN, "a stored plan"), (ABSENT_PLAN, "an absent plan")] {
        for (label, start_offset, verdict) in cases {
            let start = now_secs() + start_offset;
            assert_window_verdict(&emulator, label, held, plan, (start, start + DAY), verdict)
                .await;
        }
    }
}

#[tokio::test]
async fn a_too_wide_window_is_refused_on_its_width_wherever_it_sits() {
    // The two window rules are independent: a window that breaks the width rule is
    // refused on the width whether it lies ahead of now, around now, or further back than
    // the documented span — so neither rule hides the other.
    let week = MAX_FORECAST_WINDOW_DAYS * DAY;
    let history = MAX_FORECAST_HISTORY_DAYS * DAY;
    let cases: [(&str, i64); 4] = [
        ("a too-wide window starting a year ahead", 365 * DAY),
        ("a too-wide window starting now", 0),
        ("a too-wide window inside the documented span", -history / 2),
        ("a too-wide window past the documented span", -300 * DAY),
    ];

    let emulator = with_stored_plan().await;
    for (label, start_offset) in cases {
        let start = now_secs() + start_offset;
        assert_window_verdict(
            &emulator,
            label,
            "a stored plan",
            STORED_PLAN,
            (start, start + week + 1),
            Verdict::RefusedAsTooWide,
        )
        .await;
    }
}

#[tokio::test]
async fn only_the_documented_scaling_plan_version_is_accepted() {
    // `ScalingPlanVersion` is a long (`smithy_query(query="shape",
    // shape="ScalingPlanVersion")`) documented as "Currently, the only valid value is 1",
    // so the rows differ in version MAGNITUDE and SIGN across that type's whole range:
    // zero, the next version up, a negative version, a version past 32 bits and both
    // extremes of the type.
    let cases: [(&str, i64, bool); 7] = [
        ("the only documented version", ONLY_VERSION, true),
        ("version zero", 0, false),
        ("the next version up", ONLY_VERSION + 1, false),
        ("a negative version", -7, false),
        ("a version past 32 bits", 4_294_967_296, false),
        ("the largest long", i64::MAX, false),
        ("the smallest long", i64::MIN, false),
    ];

    let emulator = with_stored_plan().await;
    // Checked against a plan the emulator holds and a name it does not: the version is a
    // request member, so its verdict does not depend on what is stored.
    for (plan, held) in [(STORED_PLAN, "a stored plan"), (ABSENT_PLAN, "an absent plan")] {
        for (label, version, accepted) in cases {
            let base = now_secs();
            let body = asg_request(plan, version, "CapacityForecast", (base, base + HOUR));
            let (status, parsed) = emulator.call(OPERATION, &body).await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {label:?} ({version}) on {held}: expected HTTP 200, got {status} \
                     with {parsed}"
                );
                assert!(
                    datapoints(&parsed).is_empty(),
                    "case {label:?} ({version}) on {held}: expected no data points, got {parsed}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&parsed)),
                (400, "ValidationException"),
                "case {label:?} ({version}) on {held}: expected HTTP 400 \
                 ValidationException, got {status} with {parsed}"
            );
            let message = error_message(&parsed);
            assert!(
                message.contains("ScalingPlanVersion"),
                "case {label:?} ({version}) on {held}: the refusal must name the member it \
                 is about, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn only_an_auto_scaling_group_desired_capacity_is_forecast() {
    // Every other `ScalableDimension` the model lists, each with a resource id of the
    // kind that dimension scales (source: `smithy_query(query="shape",
    // shape="ScalableDimension")`, mirrored as `SCALABLE_DIMENSION_VALUES`, and the
    // worked resource-id examples in `ScalingInstruction$ResourceId`'s documentation) —
    // none of which this operation forecasts for. The last three rows probe the other
    // halves of the rule: the right dimension under another namespace, an off-enum
    // dimension, and an Auto Scaling group id without the documented prefix.
    let cases: [(&str, &str, &str, &str); 10] = [
        (
            "an ECS service's desired count",
            "ecs",
            "service/default/sample-webapp",
            "ecs:service:DesiredCount",
        ),
        (
            "a Spot Fleet request's target capacity",
            "ec2",
            "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4ea7606d",
            "ec2:spot-fleet-request:TargetCapacity",
        ),
        (
            "an Aurora replica count",
            "rds",
            "cluster:my-db-cluster",
            "rds:cluster:ReadReplicaCount",
        ),
        (
            "a DynamoDB table's read capacity",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:ReadCapacityUnits",
        ),
        (
            "a DynamoDB table's write capacity",
            "dynamodb",
            "table/my-table",
            "dynamodb:table:WriteCapacityUnits",
        ),
        (
            "a DynamoDB index's read capacity",
            "dynamodb",
            "table/my-table/index/my-table-index",
            "dynamodb:index:ReadCapacityUnits",
        ),
        (
            "a DynamoDB index's write capacity",
            "dynamodb",
            "table/my-table/index/my-table-index",
            "dynamodb:index:WriteCapacityUnits",
        ),
        (
            "the Auto Scaling dimension under another service namespace",
            "dynamodb",
            COVERED_RESOURCE,
            FORECAST_SCALABLE_DIMENSION,
        ),
        (
            "a scalable dimension outside the modelled enum",
            FORECAST_SERVICE_NAMESPACE,
            COVERED_RESOURCE,
            "autoscaling:autoScalingGroup:Nonsense",
        ),
        (
            "an Auto Scaling group id without the documented prefix",
            FORECAST_SERVICE_NAMESPACE,
            "my-asg",
            FORECAST_SCALABLE_DIMENSION,
        ),
    ];

    let emulator = with_stored_plan().await;
    for (label, namespace, resource_id, dimension) in cases {
        let base = now_secs();
        let body = forecast_request(
            STORED_PLAN,
            ONLY_VERSION,
            (namespace, resource_id, dimension),
            "CapacityForecast",
            (base, base + HOUR),
        );
        let (status, parsed) = emulator.call(OPERATION, &body).await;
        assert_eq!(
            (status, error_code(&parsed)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {parsed}"
        );
    }
}

#[tokio::test]
async fn a_forecast_data_type_outside_the_modelled_enum_is_refused() {
    let emulator = with_stored_plan().await;
    // Rows differ in how far they miss the enum: an empty value, a near-miss in case, a
    // value from another member's enum, and a plausible but unmodelled name.
    for label in [
        "",
        "capacityforecast",
        "ASGAverageCPUUtilization",
        "CapacityForecasts",
    ] {
        let base = now_secs();
        let body = asg_request(STORED_PLAN, ONLY_VERSION, label, (base, base + HOUR));
        let (status, parsed) = emulator.call(OPERATION, &body).await;
        assert_eq!(
            (status, error_code(&parsed)),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {parsed}"
        );
    }
}

#[tokio::test]
async fn a_plan_configuring_no_predictive_scaling_reports_an_empty_forecast() {
    let emulator = with_stored_plan().await;
    // A second plan, covering a DIFFERENT Auto Scaling group, so a request for the
    // covered resource reaches a plan that does not cover it.
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "other plan",
                "ApplicationSource": stack_source(2),
                "ScalingInstructions": [instruction(
                    FORECAST_SERVICE_NAMESPACE,
                    UNCOVERED_RESOURCE,
                    FORECAST_SCALABLE_DIMENSION,
                    1,
                    10,
                    "ASGAverageCPUUtilization",
                    40.0,
                )],
            }),
        )
        .await;
    emulator.make_all_due();

    // Rows differ along what the lookup generalises over: whether the plan is held and
    // whether the plan that is held covers the named resource. Neither plan's instruction
    // carries a load metric specification, so neither configures predictive scaling and
    // neither has a forecast — the covered resource included.
    let lookups: [(&str, &str); 3] = [
        ("a plan that covers the resource", STORED_PLAN),
        ("a plan this service does not hold", ABSENT_PLAN),
        ("a plan that covers another resource", "other plan"),
    ];

    // Source: `smithy_query(query="shape",shape="ForecastDataType")`, mirrored in
    // `src/ops/types/shared_types.rs` as `FORECAST_DATA_TYPE_VALUES` — every value,
    // including the two scheduled-action ones the issue did not cite.
    for data_type in FORECAST_DATA_TYPE_VALUES {
        for (label, plan) in lookups {
            let base = now_secs();
            let parsed = emulator
                .ok(
                    OPERATION,
                    asg_request(plan, ONLY_VERSION, data_type, (base, base + DAY)),
                )
                .await;
            assert!(
                datapoints(&parsed).is_empty(),
                "case {label:?} / {data_type}: expected no data points, got {parsed}"
            );
        }
    }
}

#[tokio::test]
async fn a_forecast_read_leaves_the_stored_plan_as_it_was() {
    let emulator = with_stored_plan().await;
    let before = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [STORED_PLAN] }),
        )
        .await;

    let base = now_secs();
    for data_type in FORECAST_DATA_TYPE_VALUES {
        emulator
            .ok(
                OPERATION,
                asg_request(STORED_PLAN, ONLY_VERSION, data_type, (base, base + DAY)),
            )
            .await;
    }

    let after = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [STORED_PLAN] }),
        )
        .await;
    assert_eq!(
        after, before,
        "reading the forecast must not change what a later caller sees: {before} became \
         {after}"
    );
}
