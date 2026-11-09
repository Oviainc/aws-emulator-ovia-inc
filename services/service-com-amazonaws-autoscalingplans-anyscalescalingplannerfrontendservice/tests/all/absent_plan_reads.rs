// SPDX-License-Identifier: CC0-1.0

//! What the REPORTING operations of this service answer when nothing matches, and how
//! their documented day bounds are measured — each over a TABLE of requests rather than
//! the one request that exposed it.
//!
//! Two rules are pinned here:
//!
//! * an operation that REPORTS on a resource and binds no not-found error answers HTTP 200
//!   with its collection member empty when the (`ScalingPlanName`, `ScalingPlanVersion`)
//!   coordinate selects no stored plan — the empty set of things to report, not a client
//!   error. A refusal belongs to the operations that DO model a not-found error (the two
//!   mutating ones, pinned in [`crate::error_message_wording`]) and to what the request
//!   itself says; the VERSION half of the coordinate is pinned over a table of versions in
//!   [`crate::plan_version_selects_a_plan`];
//! * the day bounds documented on the forecast window are spans of forecast DATA, and
//!   forecast data exists at one data point per hour, so each bound is measured in whole
//!   hourly forecast periods: a window reaching less than one period past a bound is
//!   served, and one reaching a whole period past it is refused.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::{
    FORECAST_INTERVAL_SECONDS, FORECAST_SCALABLE_DIMENSION, FORECAST_SERVICE_NAMESPACE,
    MAX_FORECAST_HISTORY_DAYS, MAX_FORECAST_WINDOW_DAYS,
};

use crate::scaling_plan_lifecycle::{
    error_code, error_message, now_secs, simple_instructions, stack_source, Emulator,
    DAY_SECONDS as DAY,
};

/// The reporting operations that identify ONE plan by (`ScalingPlanName`,
/// `ScalingPlanVersion`), with the collection member each reports.
///
/// Source: `smithy_query(query="operation",operation="DescribeScalingPlanResources,
/// GetScalingPlanResourceForecastData")` — the first binds `ConcurrentUpdateException`,
/// `InternalServiceException`, `InvalidNextTokenException` and `ValidationException`, the
/// second `InternalServiceException` and `ValidationException`. NEITHER binds a not-found
/// shape, while the mutating `DeleteScalingPlan` and `UpdateScalingPlan` both bind
/// `ObjectNotFoundException`.
///
/// `pub(crate)` so a suite driving the same two reads names the same pair.
pub(crate) const READS: [(&str, &str); 2] = [
    ("DescribeScalingPlanResources", "ScalingPlanResources"),
    ("GetScalingPlanResourceForecastData", "Datapoints"),
];

/// The only scaling-plan version the model documents as valid.
const ONLY_VERSION: i64 = 1;

/// The plan the emulator holds in these tests.
const STORED_PLAN: &str = "held plan";

/// A plan these tests create and then delete, so a row can name a plan that existed.
const DELETED_PLAN: &str = "drained plan";

/// One forecast period in seconds: the granularity forecast data exists at, and the span
/// the documented day bounds are measured in.
const HOUR: i64 = FORECAST_INTERVAL_SECONDS;

/// The Auto Scaling group [`simple_instructions`] instructs.
const RESOURCE_ID: &str = "autoScalingGroup/my-asg";

/// One request for `operation`, naming `(name, version)` — plus, for the forecast
/// operation, the five further members the model marks required. Its window is one
/// forecast period starting on the hour, well inside both documented bounds, so only the
/// lookup can decide the answer.
///
/// `pub(crate)` so a suite driving the same two reads builds their requests the same way.
pub(crate) fn read_request(operation: &str, name: &str, version: i64) -> Value {
    if operation == "GetScalingPlanResourceForecastData" {
        let start = now_secs().div_euclid(HOUR) * HOUR;
        return json!({
            "ScalingPlanName": name,
            "ScalingPlanVersion": version,
            "ServiceNamespace": FORECAST_SERVICE_NAMESPACE,
            "ResourceId": RESOURCE_ID,
            "ScalableDimension": FORECAST_SCALABLE_DIMENSION,
            "ForecastDataType": "CapacityForecast",
            "StartTime": start,
            "EndTime": start + HOUR,
        });
    }
    json!({ "ScalingPlanName": name, "ScalingPlanVersion": version })
}

/// A forecast request for the stored plan's resource over `[start, end)`.
fn forecast_over(window: (i64, i64)) -> Value {
    let mut request = read_request("GetScalingPlanResourceForecastData", STORED_PLAN, ONLY_VERSION);
    request["StartTime"] = json!(window.0);
    request["EndTime"] = json!(window.1);
    request
}

/// An emulator holding one settled plan, and having held and deleted a second.
async fn with_one_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    for (index, name) in [(1usize, STORED_PLAN), (2, DELETED_PLAN)] {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(index),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();
    emulator
        .ok(
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": DELETED_PLAN, "ScalingPlanVersion": ONLY_VERSION }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The (name, version) pairs that match no stored plan, at the one documented version.
///
/// `ScalingPlanName` is len 1..128 with pattern `^[\p{Print}&&[^|:/]]+$`
/// (`smithy_query(query="shape",shape="ScalingPlanName")`), so the rows differ in name
/// length, character content, letter case and in whether the plan ever existed — the
/// dimensions a lookup can miss along.
fn absent_names() -> Vec<(&'static str, String)> {
    vec![
        ("a name nothing created", "sim-probe-absent".to_string()),
        ("a one-character name", "x".to_string()),
        (
            "a name at the 128-character length bound",
            "a".repeat(128),
        ),
        (
            "a name with spaces and punctuation",
            "my plan #2 (staging)".to_string(),
        ),
        ("the stored name in another case", STORED_PLAN.to_uppercase()),
        (
            "a plan that was created and then deleted",
            DELETED_PLAN.to_string(),
        ),
    ]
}

#[tokio::test]
async fn a_read_that_matches_no_stored_plan_reports_its_collection_empty() {
    let emulator = with_one_stored_plan().await;

    for (label, name) in absent_names() {
        for (operation, member) in READS {
            let request = read_request(operation, &name, ONLY_VERSION);
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 200,
                "case {operation}/{label}: expected HTTP 200, got {status} with {body}"
            );
            assert_eq!(
                body[member], json!([]),
                "case {operation}/{label}: expected an empty {member} list, got {body}"
            );
            assert!(
                body.get("NextToken").is_none_or(Value::is_null),
                "case {operation}/{label}: an empty answer continues nowhere, got {body}"
            );
        }
    }

    // The VERSION half of the same coordinate misses the same way: the member carries no
    // `@range` and no enum, so it selects the plan a read addresses rather than gating the
    // request, and a version no stored plan carries reports the same empty collection. The
    // full table of versions lives in [`crate::plan_version_selects_a_plan`].
    for (label, version) in [
        ("the version past the one a created plan carries", ONLY_VERSION + 1),
        ("version zero", 0),
        ("a negative version", -1),
        ("a version past 32 bits", 4_294_967_296),
    ] {
        for (operation, member) in READS {
            let request = read_request(operation, STORED_PLAN, version);
            let (status, body) = emulator.call(operation, &request).await;
            assert_eq!(
                status, 200,
                "case {operation}/{label} ({version}): a version nothing holds is a miss, \
                 got {status} with {body}"
            );
            assert_eq!(
                body[member],
                json!([]),
                "case {operation}/{label} ({version}): expected an empty {member} list, got \
                 {body}"
            );
        }
    }

    // The control: the plan that IS held reports what it holds on both paths, so the empty
    // answers above come from the lookup and not from the operations.
    let described = emulator
        .ok(
            "DescribeScalingPlanResources",
            read_request("DescribeScalingPlanResources", STORED_PLAN, ONLY_VERSION),
        )
        .await;
    let resources = described["ScalingPlanResources"]
        .as_array()
        .unwrap_or_else(|| panic!("the stored plan must report a list, got {described}"));
    assert_eq!(
        resources.len(),
        1,
        "the stored plan instructs one resource, so it reports one, got {described}"
    );
    assert_eq!(
        resources[0]["ResourceId"],
        json!(RESOURCE_ID),
        "the reported resource must be the instructed one, got {described}"
    );
    let forecast = emulator
        .ok(
            "GetScalingPlanResourceForecastData",
            read_request("GetScalingPlanResourceForecastData", STORED_PLAN, ONLY_VERSION),
        )
        .await;
    assert!(
        forecast["Datapoints"].is_array(),
        "the required Datapoints member must be a list, got {forecast}"
    );
}

#[tokio::test]
async fn a_read_that_matches_nothing_changes_nothing_a_later_caller_sees() {
    let emulator = with_one_stored_plan().await;
    let before = emulator.ok("DescribeScalingPlans", json!({})).await;

    // Every absent lookup, twice over, on both reads: a report must not mint the plan it
    // did not find, and the second read has to answer exactly like the first.
    for (label, name) in absent_names() {
        for (operation, member) in READS {
            let request = read_request(operation, &name, ONLY_VERSION);
            let first = emulator.ok(operation, request.clone()).await;
            let again = emulator.ok(operation, request).await;
            assert_eq!(
                first[member], again[member],
                "case {operation}/{label}: two reads of the same miss must agree, got \
                 {first} then {again}"
            );
        }
    }

    let after = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        after["ScalingPlans"], before["ScalingPlans"],
        "the reads above must leave the stored plans as they were, got {after} after \
         {before}"
    );
}

#[tokio::test]
async fn the_documented_window_bounds_are_measured_in_whole_forecast_periods() {
    let week = MAX_FORECAST_WINDOW_DAYS * DAY;
    let history = MAX_FORECAST_HISTORY_DAYS * DAY;

    // Rows differ along the bound they probe (the maximum WIDTH of the window and how far
    // back its START may sit), along how far past that bound they reach — nothing, a
    // second, a minute, the last second inside one forecast period, a whole period, two
    // periods — and along which side of now they lie. `accepted` is what the rule answers;
    // `names_width` says which bound a refusal is about.
    //
    // `(label, window, accepted, names_width)`; a window is `(start, end)` in epoch
    // seconds, with the start placed on a forecast period boundary where the WIDTH is what
    // the row probes.
    let hour_start = now_secs().div_euclid(HOUR) * HOUR;
    let cases: [(&str, (i64, i64), bool, bool); 10] = [
        (
            "a window exactly at the documented maximum width",
            (hour_start, hour_start + week),
            true,
            true,
        ),
        (
            "a window one second past the maximum width",
            (hour_start, hour_start + week + 1),
            true,
            true,
        ),
        (
            "a window a minute past the maximum width",
            (hour_start, hour_start + week + 60),
            true,
            true,
        ),
        (
            "a window the last second inside the period past the maximum width",
            (hour_start, hour_start + week + HOUR - 1),
            true,
            true,
        ),
        (
            "a window a whole forecast period past the maximum width",
            (hour_start, hour_start + week + HOUR),
            false,
            true,
        ),
        (
            "a window two forecast periods past the maximum width",
            (hour_start, hour_start + week + 2 * HOUR),
            false,
            true,
        ),
        (
            "a start exactly at the documented history bound",
            (now_secs() - history, now_secs() - history + HOUR),
            true,
            false,
        ),
        (
            "a start a minute past the history bound",
            (now_secs() - history - 60, now_secs() - history - 60 + HOUR),
            true,
            false,
        ),
        (
            "a start a whole forecast period past the history bound",
            (
                now_secs() - history - HOUR,
                now_secs() - history - HOUR + HOUR,
            ),
            false,
            false,
        ),
        (
            "a start two forecast periods past the history bound",
            (
                now_secs() - history - 2 * HOUR,
                now_secs() - history - 2 * HOUR + HOUR,
            ),
            false,
            false,
        ),
    ];

    let emulator = with_one_stored_plan().await;
    for (label, window, accepted, names_width) in cases {
        let (status, body) = emulator
            .call("GetScalingPlanResourceForecastData", &forecast_over(window))
            .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {label:?} ({window:?}): expected HTTP 200, got {status} with {body}"
            );
            assert!(
                body["Datapoints"].is_array(),
                "case {label:?} ({window:?}): expected the required Datapoints list, got \
                 {body}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label:?} ({window:?}): expected a 400 ValidationException, got {status} \
             with {body}"
        );
        // The sentence is read only to tell WHICH bound refused the row, not because a
        // message text is the rule.
        let message = error_message(&body);
        let expected_days = if names_width {
            MAX_FORECAST_WINDOW_DAYS
        } else {
            MAX_FORECAST_HISTORY_DAYS
        };
        assert!(
            message.contains(&format!("{expected_days} days")),
            "case {label:?} ({window:?}): the refusal must be about the {expected_days}-day \
             bound, got {message:?}"
        );
    }
}
