// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAlarmHistory`: it reports the history items the alarm
//! writers (`PutMetricAlarm`, `SetAlarmState`) persisted, filtered by the members the model
//! declares, ordered by time stamp in the direction `ScanBy` asks for, and paged with
//! `MaxRecords` / `NextToken`.
//!
//! Every table row is a distinct input along what the rule under test generalises over
//! (which filter is used, the alarm type asked for, the history item type, the date window,
//! the scan direction, the page size), so no assertion is keyed to one recorded request.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The alarms the suite seeds. Two of them, so a request that names no alarm has to merge
/// the histories of several and a request that names one has to select from them.
const ALARMS: [&str; 2] = ["history-a", "history-b"];

/// The item types the seeded writers produce, in the order each alarm's history records
/// them: `PutMetricAlarm` writes a `ConfigurationUpdate`, each `SetAlarmState` a
/// `StateUpdate` (`HistoryItemType` values, from
/// `smithy_query(query="shape",shape="HistoryItemType")`: ConfigurationUpdate, StateUpdate,
/// Action, AlarmContributorStateUpdate, AlarmContributorAction).
const SEEDED_ITEM_TYPES: [&str; 3] = ["ConfigurationUpdate", "StateUpdate", "StateUpdate"];

/// A `PutMetricAlarm` request with only the members the model makes required, plus a name.
fn put_alarm(name: &str) -> Value {
    json!({
        "AlarmName": name,
        "Namespace": "MyApp",
        "MetricName": "Latency",
        "Statistic": "Average",
        "Period": 60,
        "EvaluationPeriods": 1,
        "Threshold": 1.0,
        "ComparisonOperator": "GreaterThanThreshold",
    })
}

/// Create each alarm in [`ALARMS`] and move it through two states, so every alarm has one
/// `ConfigurationUpdate` and two `StateUpdate` items and the two histories interleave in
/// time.
///
/// The pause between two writes is what makes the seeded instants far enough apart for a
/// date window to name a boundary BETWEEN two items: the writers stamp an item from the
/// service clock, so without it consecutive items land microseconds apart, closer together
/// than the margin [`BOUNDARY_MARGIN_SECS`] explains.
async fn seed(router: &MultiServiceRouter) {
    for state in ["ALARM", "OK"] {
        for name in ALARMS {
            if state == "ALARM" {
                ok(router, "PutMetricAlarm", &put_alarm(name)).await;
                tokio::time::sleep(SEED_GAP).await;
            }
            ok(
                router,
                "SetAlarmState",
                &json!({"AlarmName": name, "StateValue": state, "StateReason": "seeded"}),
            )
            .await;
            tokio::time::sleep(SEED_GAP).await;
        }
    }
}

/// How far apart [`seed`] spaces the history items it writes.
const SEED_GAP: std::time::Duration = std::time::Duration::from_millis(20);

/// How far from an item's own instant a date-window boundary is placed.
///
/// A time stamp crosses the wire as a double and comes back as one, and the conversions to
/// and from the modelled timestamp round at the nanosecond, so a boundary set to exactly the
/// instant a response reported can land a few hundred nanoseconds either side of the stored
/// item. The margin is far larger than that rounding and far smaller than [`SEED_GAP`], so
/// every row below selects a definite set of items.
const BOUNDARY_MARGIN_SECS: f64 = 0.001;

/// The `AlarmHistoryItems` of a response.
fn items(body: &Value) -> Vec<Value> {
    body.get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One string member of every item, in response order.
fn member_of_each(body: &Value, member: &str) -> Vec<String> {
    items(body)
        .iter()
        .map(|item| {
            item.get(member)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every item's time stamp, in response order. The awsJson protocols carry a timestamp as
/// epoch seconds.
fn timestamps(body: &Value) -> Vec<f64> {
    items(body)
        .iter()
        .map(|item| item.get("Timestamp").and_then(Value::as_f64).unwrap_or(-1.0))
        .collect()
}

#[tokio::test]
async fn history_reports_what_the_alarm_writers_persisted() {
    let router = build_router();
    seed(&router).await;

    let described = ok(&router, "DescribeAlarmHistory", &json!({})).await;
    let reported = items(&described);
    assert_eq!(
        reported.len(),
        ALARMS.len() * SEEDED_ITEM_TYPES.len(),
        "every seeded item must be reported, got {described}"
    );

    for item in &reported {
        let name = item.get("AlarmName").and_then(Value::as_str);
        assert!(
            name.is_some_and(|name| ALARMS.contains(&name)),
            "case {item:?}: every item names the alarm it belongs to, got {name:?}"
        );
        assert_eq!(
            item.get("AlarmType").and_then(Value::as_str),
            Some("MetricAlarm"),
            "case {item:?}: a metric alarm's history item carries AlarmType MetricAlarm"
        );
        let kind = item.get("HistoryItemType").and_then(Value::as_str);
        assert!(
            kind.is_some_and(|kind| SEEDED_ITEM_TYPES.contains(&kind)),
            "case {item:?}: expected one of {SEEDED_ITEM_TYPES:?}, got {kind:?}"
        );
        let summary = item.get("HistorySummary").and_then(Value::as_str);
        assert!(
            summary.is_some_and(|summary| !summary.is_empty()),
            "case {item:?}: every item carries a summary, got {summary:?}"
        );
        let stamp = item.get("Timestamp").and_then(Value::as_f64);
        assert!(
            stamp.is_some_and(|stamp| stamp > 0.0),
            "case {item:?}: every item carries a real instant, got {stamp:?}"
        );
    }

    // Per alarm, the items come back in the order the writers recorded them (oldest first
    // when scanning ascending).
    for name in ALARMS {
        let described = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({"AlarmName": name, "ScanBy": "TimestampAscending"}),
        )
        .await;
        assert_eq!(
            member_of_each(&described, "HistoryItemType"),
            SEEDED_ITEM_TYPES,
            "case {name:?}: expected {SEEDED_ITEM_TYPES:?}, got {described}"
        );
        assert!(
            member_of_each(&described, "AlarmName")
                .iter()
                .all(|reported| reported == name),
            "case {name:?}: naming an alarm reports only its own history, got {described}"
        );
    }
}

#[tokio::test]
async fn every_filter_selects_only_the_matching_items() {
    let router = build_router();
    seed(&router).await;

    // Rows differ along which filter is used, what it names and how many items it selects.
    // `AlarmType` values come from `smithy_query(query="shape",shape="AlarmType")`:
    // CompositeAlarm, MetricAlarm, LogAlarm.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("no filter merges every alarm's history", json!({}), 6),
        ("one alarm", json!({"AlarmName": ALARMS[0]}), 3),
        (
            "an alarm with no history at all",
            json!({"AlarmName": "never-created"}),
            0,
        ),
        (
            "only configuration updates",
            json!({"HistoryItemType": "ConfigurationUpdate"}),
            2,
        ),
        (
            "only state updates",
            json!({"HistoryItemType": "StateUpdate"}),
            4,
        ),
        (
            "an item type nothing recorded",
            json!({"HistoryItemType": "Action"}),
            0,
        ),
        (
            "one alarm's state updates",
            json!({"AlarmName": ALARMS[1], "HistoryItemType": "StateUpdate"}),
            2,
        ),
        (
            "metric alarms, named explicitly",
            json!({"AlarmTypes": ["MetricAlarm"]}),
            6,
        ),
        (
            "metric alarms among several types",
            json!({"AlarmTypes": ["CompositeAlarm", "MetricAlarm", "LogAlarm"]}),
            6,
        ),
        (
            "composite alarms, of which this store holds none",
            json!({"AlarmTypes": ["CompositeAlarm"]}),
            0,
        ),
        (
            "log alarms, of which this store holds none",
            json!({"AlarmTypes": ["LogAlarm"]}),
            0,
        ),
        (
            "a contributor no item was recorded for",
            json!({"AlarmContributorId": "contributor-1"}),
            0,
        ),
    ];

    for (case, filter, expected) in cases {
        let described = ok(&router, "DescribeAlarmHistory", &filter).await;
        assert_eq!(
            items(&described).len(),
            expected,
            "case {case:?}: expected {expected} items, got {described}"
        );
    }

    // A date window: rows differ in where the window falls relative to the recorded items.
    // Every boundary is placed [`BOUNDARY_MARGIN_SECS`] off an item's own instant rather
    // than exactly on it, so no row depends on how a double rounds into a timestamp.
    let all = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"ScanBy": "TimestampAscending"}),
    )
    .await;
    let stamps = timestamps(&all);
    let oldest = stamps
        .first()
        .copied()
        .expect("the seeded history is not empty");
    let newest = stamps
        .last()
        .copied()
        .expect("the seeded history is not empty");
    let margin = BOUNDARY_MARGIN_SECS;
    let windows: Vec<(&str, Value, usize)> = vec![
        (
            "a window spanning every item",
            json!({"StartDate": oldest - margin, "EndDate": newest + margin}),
            stamps.len(),
        ),
        (
            "StartDate only, just before the oldest item",
            json!({"StartDate": oldest - margin}),
            stamps.len(),
        ),
        (
            "StartDate just past the oldest item",
            json!({"StartDate": oldest + margin}),
            stamps.len() - 1,
        ),
        (
            "EndDate just past the oldest item",
            json!({"EndDate": oldest + margin}),
            1,
        ),
        (
            "a window inside the history, dropping its ends",
            json!({"StartDate": oldest + margin, "EndDate": newest - margin}),
            stamps.len() - 2,
        ),
        (
            "a window entirely before the history",
            json!({"StartDate": oldest - 600.0, "EndDate": oldest - 300.0}),
            0,
        ),
        (
            "a window entirely after the history",
            json!({"StartDate": newest + 300.0}),
            0,
        ),
    ];
    for (case, window, expected) in windows {
        let described = ok(&router, "DescribeAlarmHistory", &window).await;
        assert_eq!(
            items(&described).len(),
            expected,
            "case {case:?}: expected {expected} items, got {described}"
        );
    }
}

#[tokio::test]
async fn scan_by_orders_the_history_by_time_stamp() {
    let router = build_router();
    seed(&router).await;

    // Rows differ in the direction asked for, including omitting `ScanBy` entirely.
    for (case, request) in [
        ("ascending", json!({"ScanBy": "TimestampAscending"})),
        ("descending", json!({"ScanBy": "TimestampDescending"})),
        ("no ScanBy", json!({})),
    ] {
        let described = ok(&router, "DescribeAlarmHistory", &request).await;
        let stamps = timestamps(&described);
        let ordered = if case == "ascending" {
            stamps.windows(2).all(|pair| pair[0] <= pair[1])
        } else {
            stamps.windows(2).all(|pair| pair[0] >= pair[1])
        };
        assert!(
            ordered,
            "case {case:?}: expected the time stamps ordered, got {stamps:?}"
        );
    }

    // The invariant: one direction is the exact reverse of the other.
    let ascending = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"ScanBy": "TimestampAscending"}),
    )
    .await;
    let descending = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"ScanBy": "TimestampDescending"}),
    )
    .await;
    let mut reversed = timestamps(&descending);
    reversed.reverse();
    assert_eq!(
        timestamps(&ascending),
        reversed,
        "descending must be the reverse of ascending, got {descending}"
    );
}

#[tokio::test]
async fn paging_visits_every_item_exactly_once() {
    let router = build_router();
    seed(&router).await;
    let total = ALARMS.len() * SEEDED_ITEM_TYPES.len();

    // Rows differ in the page size and the scan direction, so the number of pages and the
    // split differ. `MaxRecords` is modelled `range 1..100`, so 1 and 100 are the published
    // bounds.
    for (case, page_size, scan_by) in [
        ("one item per page, ascending", 1, "TimestampAscending"),
        ("two items per page, ascending", 2, "TimestampAscending"),
        ("four items per page, descending", 4, "TimestampDescending"),
        ("every item in one page", 6, "TimestampDescending"),
        ("a page larger than the history", 100, "TimestampAscending"),
    ] {
        let mut seen: Vec<f64> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"MaxRecords": page_size, "ScanBy": scan_by});
            if let Some(token) = token.as_deref() {
                request["NextToken"] = json!(token);
            }
            let described = ok(&router, "DescribeAlarmHistory", &request).await;
            let page = items(&described);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must not exceed MaxRecords, got {described}"
            );
            seen.extend(timestamps(&described));
            pages += 1;
            token = described
                .get("NextToken")
                .and_then(Value::as_str)
                .map(ToString::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case {case:?}: paging must terminate, saw {seen:?}"
            );
        }
        let expected_pages = total.div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, walked {pages} over {seen:?}"
        );
        assert_eq!(
            seen.len(),
            total,
            "case {case:?}: paging must visit every item exactly once, saw {seen:?}"
        );
        let ordered = if scan_by == "TimestampAscending" {
            seen.windows(2).all(|pair| pair[0] <= pair[1])
        } else {
            seen.windows(2).all(|pair| pair[0] >= pair[1])
        };
        assert!(
            ordered,
            "case {case:?}: paging must keep the scan order, saw {seen:?}"
        );
    }
}

#[tokio::test]
async fn history_outlives_the_alarm_it_belongs_to() {
    let router = build_router();
    seed(&router).await;

    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": [ALARMS[0]]}),
    )
    .await;

    // "CloudWatch retains the history of an alarm even if you delete the alarm."
    let described = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"AlarmName": ALARMS[0]}),
    )
    .await;
    assert_eq!(
        items(&described).len(),
        SEEDED_ITEM_TYPES.len(),
        "a deleted alarm keeps its history, got {described}"
    );
    let still_an_alarm = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": [ALARMS[0]]}),
    )
    .await;
    assert_eq!(
        still_an_alarm
            .get("MetricAlarms")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        0,
        "the alarm itself is gone, got {still_an_alarm}"
    );
}

#[tokio::test]
async fn a_token_names_the_same_item_while_more_history_is_written() {
    let router = build_router();
    seed(&router).await;

    // The invariant: a cursor identifies an ITEM, not a position, so writing more history
    // between two pages can never make the next page repeat what the first already
    // reported. Rows differ in the scan direction, which decides whether the new items fall
    // before or after the cursor.
    for (case, scan_by) in [
        ("ascending", "TimestampAscending"),
        ("descending", "TimestampDescending"),
    ] {
        let first = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({"MaxRecords": 2, "ScanBy": scan_by}),
        )
        .await;
        let already_seen = timestamps(&first);
        let token = first
            .get("NextToken")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .expect("a history longer than two items pages");

        ok(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": ALARMS[0],
                "StateValue": "ALARM",
                "StateReason": "recorded between two pages",
            }),
        )
        .await;

        let second = ok(
            &router,
            "DescribeAlarmHistory",
            &json!({"MaxRecords": 2, "ScanBy": scan_by, "NextToken": token}),
        )
        .await;
        for stamp in timestamps(&second) {
            assert!(
                !already_seen.contains(&stamp),
                "case {case:?}: the next page must not repeat the first ({already_seen:?}), \
                 got {second}"
            );
        }
    }
}

#[tokio::test]
async fn bad_requests_carry_the_modelled_status_and_code() {
    let router = build_router();
    seed(&router).await;

    // Rows differ in how the request is wrong and which refusal that is.
    for (case, request, status, code) in [
        (
            "a token this service never minted",
            json!({"NextToken": "not-a-token"}),
            400,
            "InvalidNextToken",
        ),
        (
            "a token that is not even hex",
            json!({"NextToken": "zzzz"}),
            400,
            "InvalidNextToken",
        ),
        (
            "an inverted date range",
            json!({"StartDate": 1_700_000_600.0, "EndDate": 1_700_000_000.0}),
            400,
            "ValidationException",
        ),
        (
            "MaxRecords below the modelled minimum",
            json!({"MaxRecords": 0}),
            400,
            "ValidationException",
        ),
        (
            "MaxRecords above the modelled maximum",
            json!({"MaxRecords": 101}),
            400,
            "ValidationException",
        ),
        (
            "an AlarmName longer than the modelled 255",
            json!({"AlarmName": "n".repeat(256)}),
            400,
            "ValidationException",
        ),
        (
            "a HistoryItemType the enum does not declare",
            json!({"HistoryItemType": "Nonsense"}),
            400,
            "ValidationException",
        ),
    ] {
        let (actual_status, body) = call(&router, "DescribeAlarmHistory", &request).await;
        assert_eq!(
            actual_status, status,
            "case {case:?}: expected HTTP {status}, got {actual_status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected error code {code}, got {body}"
        );
    }

    // A token the service DID mint keeps working, so the refusals above are about the token
    // being forged rather than about paging being refused.
    let first = ok(&router, "DescribeAlarmHistory", &json!({"MaxRecords": 1})).await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .expect("a history of six items pages at MaxRecords 1");
    let second = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({"MaxRecords": 1, "NextToken": token}),
    )
    .await;
    assert_eq!(
        items(&second).len(),
        1,
        "a token this service minted must answer the next page, got {second}"
    );
    assert_ne!(
        timestamps(&second),
        timestamps(&first),
        "the second page must not repeat the first, got {second}"
    );
}
