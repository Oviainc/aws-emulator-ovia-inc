// SPDX-License-Identifier: CC0-1.0

//! End-to-end suite for `DescribeBudgetPerformanceHistory`, driven over the wire through
//! the same `MultiServiceRouter` the production router serves from (the harness is the
//! lifecycle suite's, so every suite drives the service identically).
//!
//! The RULE these tables assert: a performance history reports the stored budget's own
//! identity and cost scoping, plus one `BudgetedAndActualAmounts` record per period of
//! that budget's `TimeUnit` that the history retains and the budget itself covers, OLDEST
//! FIRST and ending with the period in progress — each record carrying the limit in force
//! while that period ran and the cost or usage attributed to it — narrowed to the periods
//! the caller's `TimePeriod` covers and handed back one `MaxResults`-sized page at a time,
//! without changing anything a later caller sees. A budget whose time unit has no history
//! is refused.
//!
//! Each table spans that class rather than one request: its rows differ along the time
//! unit and so the period length and the retained count, the budget type and the unit its
//! amounts are measured in, whether the limit is one `BudgetLimit` or a
//! `PlannedBudgetLimits` series, which cost-scoping syntax the budget uses, where the
//! budget's own `TimePeriod` begins and ends relative to the retained periods, where the
//! caller's window falls, and the page size.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    encode_next_token, ATTRIBUTED_SPEND_AMOUNT,
};

use super::budget_lifecycle::{account, call, caller_account, civil_secs, is_error, router};
use super::budget_read_projection::month_start_offset;

/// How many periods a budget's performance history retains for each time unit, written
/// from the documentation of
/// `DescribeBudgetPerformanceHistoryResponse$BudgetPerformanceHistory`
/// (`smithy_query(query="operation",operation="DescribeBudgetPerformanceHistory")`, then
/// `smithy_query(query="doc",shape="DescribeBudgetPerformanceHistoryResponse",member="BudgetPerformanceHistory")`):
/// "For DAILY budgets, the history saves the state of the budget for the last 60 days.
/// For MONTHLY budgets, the history saves the state of the budget for the current month
/// plus the last 12 months. For QUARTERLY budgets, the history saves the state of the
/// budget for the last four quarters."
///
/// Each count includes the period IN PROGRESS, which is the period those sentences count
/// from — "the current month plus the last 12 months" is thirteen records. Spelled out
/// here from the documentation rather than read off the service, so the table cannot
/// drift into agreeing with a wrong implementation.
const RETAINED: [(&str, usize); 3] = [("DAILY", 60), ("MONTHLY", 13), ("QUARTERLY", 4)];

/// Seconds in a UTC day — the step between two consecutive `DAILY` period boundaries.
const DAY_SECS: i64 = 86_400;

/// The first month of each calendar quarter, which is where a `QUARTERLY` period begins.
const QUARTER_MONTHS: [&str; 4] = ["01", "04", "07", "10"];

/// A start date far enough back that every retained period of every time unit falls
/// inside the budget's own `TimePeriod` — written as the date it is.
const LONG_RUNNING_START: &str = "2000-01-01T00:00:00Z";

/// Now in epoch seconds, and an instant rendered as `YYYY-MM-DDThh:mm:ssZ` — the shared
/// helpers, so this suite reads the clock and the calendar the same way the sibling
/// suites in this test target do.
use super::budget_read_projection::{civil, now_secs};

/// The epoch seconds a response timestamp carries: awsJson renders a timestamp as epoch
/// seconds, so the member is a JSON number.
fn timestamp(period: &Value, member: &str) -> i64 {
    let secs = period
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("{member} must be a timestamp, got {period}"));
    secs.trunc() as i64
}

/// Create one budget and fail the test if the service refuses it.
///
/// `pub(crate)` so the sibling suite that asserts what each record REPORTS drives the
/// same harness rather than copying it.
pub(crate) async fn create(router: &MultiServiceRouter, case: &str, budget: &Value) {
    let (status, body) = call(
        router,
        "CreateBudget",
        &json!({"AccountId": caller_account(), "Budget": budget}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected CreateBudget to answer 200, got {status} {body}"
    );
}

/// Ask for one budget's performance history.
///
/// `pub(crate)` so the sibling suites in this test target share the one helper.
pub(crate) async fn history(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    call(router, "DescribeBudgetPerformanceHistory", request).await
}

/// The request that asks about `name` with no narrowing and no paging.
///
/// `pub(crate)` so the sibling suites in this test target share the one helper.
pub(crate) fn ask(name: &str) -> Value {
    json!({"AccountId": caller_account(), "BudgetName": name})
}

/// The reported records, or an empty list when the response carries none.
///
/// `pub(crate)` so the sibling suites in this test target share the one helper.
pub(crate) fn records(body: &Value) -> Vec<Value> {
    body["BudgetPerformanceHistory"]["BudgetedAndActualAmountsList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `(Start, End)` of one record's period.
///
/// `pub(crate)` so the sibling suites in this test target share the one helper.
pub(crate) fn bounds(record: &Value) -> (i64, i64) {
    let period = &record["TimePeriod"];
    (timestamp(period, "Start"), timestamp(period, "End"))
}

/// The invariants every reported series satisfies, whatever the budget: each period's
/// bounds are ordered, no reported period begins after the call, and consecutive records
/// run oldest first and ABUT — one period ending exactly where the next begins — so the
/// series partitions the time it covers instead of double-counting any of it.
fn assert_series(case: &str, records: &[Value]) {
    let mut previous_end: Option<i64> = None;
    for record in records {
        let (start, end) = bounds(record);
        assert!(
            start < end,
            "case {case}: expected a period whose Start precedes its End, got {} .. {}",
            civil(start),
            civil(end)
        );
        assert!(
            start <= now_secs(),
            "case {case}: expected no period that has not begun, got one starting {}",
            civil(start)
        );
        if let Some(previous) = previous_end {
            assert_eq!(
                previous,
                start,
                "case {case}: expected consecutive periods to abut oldest first, got {} then {}",
                civil(previous),
                civil(start)
            );
        }
        previous_end = Some(end);
    }
}

/// The boundary grid the periods of `time_unit` sit on, read off the runtime's own
/// rendering of each boundary: every period of every unit starts at midnight UTC, a
/// monthly period on the first of a month, a quarterly one on the first of a quarter's
/// first month, and consecutive daily boundaries one day apart.
fn assert_boundaries(case: &str, time_unit: &str, records: &[Value]) {
    for record in records {
        let (start, end) = bounds(record);
        if time_unit == "DAILY" {
            assert_eq!(
                end - start,
                DAY_SECS,
                "case {case}: expected a one-day period, got {} .. {}",
                civil(start),
                civil(end)
            );
        }
        for boundary in [start, end] {
            let rendered = civil(boundary);
            let (date, time) = rendered
                .split_once('T')
                .unwrap_or_else(|| panic!("case {case}: {rendered} must be a date-time"));
            assert_eq!(
                time, "00:00:00Z",
                "case {case}: expected a midnight-UTC boundary, got {rendered}"
            );
            let mut parts = date.split('-');
            let _year = parts.next().unwrap_or_default();
            let month = parts.next().unwrap_or_default().to_string();
            let day = parts.next().unwrap_or_default().to_string();
            match time_unit {
                "DAILY" => {}
                "MONTHLY" => assert_eq!(
                    day, "01",
                    "case {case}: expected a boundary on the first of a month, got {rendered}"
                ),
                "QUARTERLY" => {
                    assert_eq!(
                        day, "01",
                        "case {case}: expected a boundary on the first of a month, got {rendered}"
                    );
                    assert!(
                        QUARTER_MONTHS.contains(&month.as_str()),
                        "case {case}: expected a boundary in a quarter's first month, \
                         got {rendered}"
                    );
                }
                other => panic!("case {case}: this suite drives no time unit {other}"),
            }
        }
    }
}

/// One row of the retained-window table.
struct WindowRow {
    case: &'static str,
    time_unit: &'static str,
    retained: usize,
    budget_type: &'static str,
    unit: &'static str,
    /// The cost-scoping members the budget carries, merged into its payload.
    scoping: Value,
}

/// The retained-window table. Rows differ along the time unit (and so the period length
/// and the retained count), the budget type, the unit the amounts are measured in, and
/// which cost-scoping syntax the budget uses.
fn window_rows() -> Vec<WindowRow> {
    let mut rows = Vec::new();
    for (time_unit, retained) in RETAINED {
        rows.push(WindowRow {
            case: "cost budget, no scoping",
            time_unit,
            retained,
            budget_type: "COST",
            unit: "USD",
            scoping: json!({}),
        });
        rows.push(WindowRow {
            case: "usage budget, cost filters",
            time_unit,
            retained,
            budget_type: "USAGE",
            unit: "GB",
            scoping: json!({"CostFilters": {"Service": ["Amazon Elastic Compute Cloud - Compute"]}}),
        });
        rows.push(WindowRow {
            case: "cost budget, filter expression and metrics",
            time_unit,
            retained,
            budget_type: "COST",
            unit: "USD",
            scoping: json!({
                "FilterExpression": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"]}},
                "Metrics": ["UnblendedCost"],
            }),
        });
    }
    rows
}

/// RULE: a history reports one record per period of the budget's own time unit that it
/// retains, oldest first and ending with the period in progress, and reports the budget's
/// own identity and cost scoping beside them.
#[tokio::test]
async fn a_history_spans_the_retained_periods_of_the_budgets_time_unit() {
    let router = router();
    for row in window_rows() {
        let case = format!("{} {}", row.time_unit, row.case);
        let name = format!("history-{case}");
        let mut budget = json!({
            "BudgetName": name,
            "BudgetType": row.budget_type,
            "TimeUnit": row.time_unit,
            "BudgetLimit": {"Amount": "250", "Unit": row.unit},
            "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
        });
        for (member, value) in row.scoping.as_object().expect("an object of members") {
            budget[member] = value.clone();
        }
        create(&router, &case, &budget).await;

        let (status, body) = history(&router, &ask(&name)).await;
        assert_eq!(
            status, 200,
            "case {case}: expected DescribeBudgetPerformanceHistory to answer 200, \
             got {status} {body}"
        );

        let header = &body["BudgetPerformanceHistory"];
        assert_eq!(
            header["BudgetName"],
            json!(name),
            "case {case}: expected the budget's own name, got {header}"
        );
        assert_eq!(
            header["BudgetType"],
            json!(row.budget_type),
            "case {case}: expected the budget's own type, got {header}"
        );
        assert_eq!(
            header["TimeUnit"],
            json!(row.time_unit),
            "case {case}: expected the budget's own time unit, got {header}"
        );
        for (member, value) in row.scoping.as_object().expect("an object of members") {
            assert_eq!(
                header.get(member).cloned().unwrap_or(Value::Null),
                *value,
                "case {case}: expected the history to carry the budget's {member}, got {header}"
            );
        }
        assert!(
            body.get("NextToken").is_none_or(Value::is_null),
            "case {case}: expected no NextToken for a history that fits one page, got {body}"
        );

        let reported = records(&body);
        assert_eq!(
            reported.len(),
            row.retained,
            "case {case}: expected {} retained periods, got {}",
            row.retained,
            reported.len()
        );
        assert_series(&case, &reported);
        assert_boundaries(&case, row.time_unit, &reported);

        let (newest_start, newest_end) = bounds(reported.last().expect("a retained period"));
        let now = now_secs();
        assert!(
            newest_start <= now && now < newest_end,
            "case {case}: expected the last record to be the period in progress, got {} .. {}",
            civil(newest_start),
            civil(newest_end)
        );

        for record in &reported {
            assert_eq!(
                record["BudgetedAmount"],
                json!({"Amount": "250", "Unit": row.unit}),
                "case {case}: expected the limit in force as the budgeted amount, got {record}"
            );
            assert_eq!(
                record["ActualAmount"],
                json!({"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": row.unit}),
                "case {case}: expected the attributed spend in the limit's unit, got {record}"
            );
        }
    }
}

/// One row of the coverage table: where the budget's own `TimePeriod` begins and ends, in
/// months relative to the month in progress, and which months the history must report.
struct CoverageRow {
    case: &'static str,
    /// The budget's `Start`, in whole months relative to the month in progress, plus a
    /// number of days into that month — so a budget can begin PART WAY through a period.
    start_months: i32,
    start_days: i64,
    end_months: Option<i32>,
    /// The months, relative to the month in progress, the history must report.
    expected_months: Vec<i32>,
}

/// RULE: a history covers only the periods of the budget's own `TimePeriod` — "the period
/// of time that's covered by a budget" — and only those that have begun.
///
/// Every row's budget is one the clock has not deleted: a budget whose `End` has passed is
/// dropped from the store the moment a call observes it (the end-date rule
/// `tests/all/budget_end_date_expiry.rs` covers), so a live budget's `End` never falls
/// before a retained period.
#[tokio::test]
async fn a_history_covers_only_the_periods_the_budget_itself_covers() {
    let rows = vec![
        CoverageRow {
            case: "started three months ago",
            start_months: -3,
            start_days: 0,
            end_months: None,
            expected_months: vec![-3, -2, -1, 0],
        },
        CoverageRow {
            case: "started this month",
            start_months: 0,
            start_days: 0,
            end_months: None,
            expected_months: vec![0],
        },
        CoverageRow {
            case: "started part way through a month",
            start_months: -2,
            start_days: 10,
            end_months: None,
            expected_months: vec![-2, -1, 0],
        },
        CoverageRow {
            case: "ends at the start of next month",
            start_months: -4,
            start_days: 0,
            end_months: Some(1),
            expected_months: vec![-4, -3, -2, -1, 0],
        },
        CoverageRow {
            case: "begins next month",
            start_months: 1,
            start_days: 0,
            end_months: None,
            expected_months: Vec::new(),
        },
    ];

    let router = router();
    for row in rows {
        let name = format!("coverage-{}", row.case);
        let mut period =
            json!({"Start": month_start_offset(row.start_months) + row.start_days * DAY_SECS});
        if let Some(end_months) = row.end_months {
            period["End"] = json!(month_start_offset(end_months));
        }
        create(
            &router,
            row.case,
            &json!({
                "BudgetName": name,
                "BudgetType": "COST",
                "TimeUnit": "MONTHLY",
                "BudgetLimit": {"Amount": "40", "Unit": "USD"},
                "TimePeriod": period,
            }),
        )
        .await;

        let (status, body) = history(&router, &ask(&name)).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} {body}",
            row.case
        );
        let reported = records(&body);
        assert_series(row.case, &reported);
        let starts: Vec<i64> = reported.iter().map(|record| bounds(record).0).collect();
        let expected: Vec<i64> = row
            .expected_months
            .iter()
            .map(|months| month_start_offset(*months))
            .collect();
        assert_eq!(
            starts,
            expected,
            "case {}: expected the months {:?}, got {:?}",
            row.case,
            expected.iter().map(|at| civil(*at)).collect::<Vec<_>>(),
            starts.iter().map(|at| civil(*at)).collect::<Vec<_>>()
        );
    }
}

/// One row of the window table: the `TimePeriod` the caller asks about, in months
/// relative to the month in progress, and the months the history must then report.
struct AskedWindowRow {
    case: &'static str,
    start_months: Option<i32>,
    end_months: Option<i32>,
    expected_months: Vec<i32>,
}

/// RULE: the `TimePeriod` a caller asks about narrows the reported records to the periods
/// that window covers; a bound it leaves out does not restrict that side.
#[tokio::test]
async fn a_history_is_narrowed_to_the_time_period_the_caller_asks_about() {
    let rows = vec![
        AskedWindowRow {
            case: "both bounds",
            start_months: Some(-3),
            end_months: Some(-1),
            expected_months: vec![-3, -2],
        },
        AskedWindowRow {
            case: "start only",
            start_months: Some(-2),
            end_months: None,
            expected_months: vec![-2, -1, 0],
        },
        AskedWindowRow {
            case: "end only",
            start_months: None,
            end_months: Some(-3),
            expected_months: vec![-5, -4],
        },
        AskedWindowRow {
            case: "one month",
            start_months: Some(-1),
            end_months: Some(0),
            expected_months: vec![-1],
        },
        AskedWindowRow {
            case: "the month in progress",
            start_months: Some(0),
            end_months: Some(1),
            expected_months: vec![0],
        },
        AskedWindowRow {
            case: "entirely ahead of the budget",
            start_months: Some(1),
            end_months: Some(3),
            expected_months: Vec::new(),
        },
        AskedWindowRow {
            case: "entirely before the budget",
            start_months: Some(-9),
            end_months: Some(-5),
            expected_months: Vec::new(),
        },
    ];

    // A budget that covers the five months before the one in progress, so a window can
    // fall inside, across, or wholly outside what the budget covers.
    let router = router();
    let name = "asked-window";
    create(
        &router,
        "asked-window",
        &json!({
            "BudgetName": name,
            "BudgetType": "COST",
            "TimeUnit": "MONTHLY",
            "BudgetLimit": {"Amount": "12.50", "Unit": "USD"},
            "TimePeriod": {"Start": month_start_offset(-5)},
        }),
    )
    .await;

    for row in rows {
        let mut request = ask(name);
        let mut window = json!({});
        if let Some(months) = row.start_months {
            window["Start"] = json!(month_start_offset(months));
        }
        if let Some(months) = row.end_months {
            window["End"] = json!(month_start_offset(months));
        }
        request["TimePeriod"] = window;
        let (status, body) = history(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} {body}",
            row.case
        );
        let reported = records(&body);
        assert_series(row.case, &reported);
        let starts: Vec<i64> = reported.iter().map(|record| bounds(record).0).collect();
        let expected: Vec<i64> = row
            .expected_months
            .iter()
            .map(|months| month_start_offset(*months))
            .collect();
        assert_eq!(
            starts,
            expected,
            "case {}: expected the months {:?}, got {:?}",
            row.case,
            expected.iter().map(|at| civil(*at)).collect::<Vec<_>>(),
            starts.iter().map(|at| civil(*at)).collect::<Vec<_>>()
        );
    }
}

/// RULE: a history is paged by `MaxResults` — each page carries at most that many records
/// and a `NextToken` for the rest, the token walks the series in period order, and the
/// pages concatenate to exactly the unpaged history.
#[tokio::test]
async fn a_history_is_paged_by_max_results_in_period_order() {
    let router = router();
    // The DAILY retained window is the longest one, so it is the series that pages.
    let name = "paged-history";
    create(
        &router,
        "paged-history",
        &json!({
            "BudgetName": name,
            "BudgetType": "COST",
            "TimeUnit": "DAILY",
            "BudgetLimit": {"Amount": "5", "Unit": "USD"},
            "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
        }),
    )
    .await;

    let (_, whole) = history(&router, &ask(name)).await;
    let expected = records(&whole);
    assert_eq!(
        expected.len(),
        RETAINED[0].1,
        "case unpaged: expected the retained DAILY window, got {}",
        expected.len()
    );

    // Page sizes spanning `MaxResults`' published `@range(min: 1, max: 100)`: the minimum,
    // a size that divides the series, one that does not, the series length exactly, and
    // the maximum.
    for page_size in [1, 10, 7, 60, 100] {
        let case = format!("page size {page_size}");
        let mut collected: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = ask(name);
            request["MaxResults"] = json!(page_size);
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let (status, body) = history(&router, &request).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200, got {status} {body}"
            );
            let page = records(&body);
            assert!(
                page.len() <= page_size,
                "case {case}: expected at most {page_size} records in a page, got {}",
                page.len()
            );
            collected.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case {case}: expected the walk to terminate, got {pages} pages"
            );
            token = body
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let remaining = expected.len().saturating_sub(collected.len());
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "case {case}: expected a NextToken only while records remain, \
                 got {token:?} with {remaining} left"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            collected, expected,
            "case {case}: expected the pages to concatenate to the whole history"
        );
    }
}

/// RULE: a `NextToken` this history never minted a page at is refused with the modelled
/// `InvalidNextTokenException` (HTTP 400), not served as a last page.
#[tokio::test]
async fn a_next_token_this_history_never_minted_is_refused() {
    let router = router();
    let name = "token-history";
    create(
        &router,
        "token-history",
        &json!({
            "BudgetName": name,
            "BudgetType": "COST",
            "TimeUnit": "QUARTERLY",
            "BudgetLimit": {"Amount": "5", "Unit": "USD"},
            "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
        }),
    )
    .await;

    // Rows: a token that is not one this service mints, one minted for an offset past the
    // end of the four-quarter series, and one minted at its last offset.
    for (case, token) in [
        ("not a token", "nonsense".to_string()),
        ("past the end", encode_next_token(RETAINED[2].1 + 1)),
        ("at the end", encode_next_token(RETAINED[2].1)),
    ] {
        let mut request = ask(name);
        request["NextToken"] = json!(token);
        let (status, body) = history(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected the token to be refused with 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidNextTokenException"),
            "case {case}: expected InvalidNextTokenException, got {body}"
        );
    }
}

/// RULE: the budgeted amount of each record is the limit in force while that period ran,
/// which for a `PlannedBudgetLimits` budget is the planned entry that period started
/// under, and the actual amount is measured in that same unit.
#[tokio::test]
async fn each_record_carries_the_limit_in_force_for_its_own_period() {
    let router = router();
    let name = "planned-history";
    // The planned entries, keyed by "the start of the month, UTC in epoch seconds"
    // (`Budget$PlannedBudgetLimits`): a limit for the month three back, one for the month
    // one back, and one for the month in progress — so a record falls before the first
    // entry, between two entries, and on an entry.
    let planned = [(-3_i32, "30"), (-1, "60"), (0, "90")];
    let mut limits = serde_json::Map::new();
    for (months, amount) in planned {
        limits.insert(
            month_start_offset(months).to_string(),
            json!({"Amount": amount, "Unit": "USD"}),
        );
    }
    create(
        &router,
        "planned-history",
        &json!({
            "BudgetName": name,
            "BudgetType": "COST",
            "TimeUnit": "MONTHLY",
            "PlannedBudgetLimits": Value::Object(limits),
            "TimePeriod": {"Start": month_start_offset(-5)},
        }),
    )
    .await;

    let (status, body) = history(&router, &ask(name)).await;
    assert_eq!(status, 200, "case planned: expected 200, got {status} {body}");
    let reported = records(&body);
    assert_series("planned", &reported);
    assert_eq!(
        reported.len(),
        6,
        "case planned: expected the six months the budget covers, got {}",
        reported.len()
    );

    for (months, record) in (-5..=0).zip(&reported) {
        let case = format!("planned month {months}");
        // The entry in force is the latest planned entry that had started when the period
        // began; a period before every entry takes the first one, since the series "begins
        // at a date in the future" relative to it.
        let expected = planned
            .iter()
            .rev()
            .find(|(at, _)| *at <= months)
            .map_or(planned[0].1, |(_, amount)| *amount);
        assert_eq!(
            record["BudgetedAmount"],
            json!({"Amount": expected, "Unit": "USD"}),
            "case {case}: expected the planned limit {expected} in force, got {record}"
        );
        assert_eq!(
            record["ActualAmount"],
            json!({"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": "USD"}),
            "case {case}: expected the attributed spend in the limit's unit, got {record}"
        );
    }
}

/// RULE: a budget whose time unit names no repeating period covers exactly the one
/// `TimePeriod` its caller chose, so its history is that single period — and nothing at
/// all while that period is still ahead of the clock.
#[tokio::test]
async fn a_custom_period_budget_reports_the_one_period_it_covers() {
    let router = router();
    for (case, start_months, end_months, expected) in [
        ("a custom period under way", -2, 2, true),
        ("a custom period ahead", 1, 3, false),
    ] {
        let name = format!("custom-{case}");
        create(
            &router,
            case,
            &json!({
                "BudgetName": name,
                "BudgetType": "COST",
                "TimeUnit": "CUSTOM",
                "BudgetLimit": {"Amount": "17", "Unit": "USD"},
                "TimePeriod": {
                    "Start": month_start_offset(start_months),
                    "End": month_start_offset(end_months),
                },
            }),
        )
        .await;

        let (status, body) = history(&router, &ask(&name)).await;
        assert_eq!(status, 200, "case {case}: expected 200, got {status} {body}");
        let reported = records(&body);
        if !expected {
            assert!(
                reported.is_empty(),
                "case {case}: expected no record for a period that has not begun, got {reported:?}"
            );
            continue;
        }
        assert_eq!(
            reported.len(),
            1,
            "case {case}: expected the one period the budget covers, got {reported:?}"
        );
        let (start, end) = bounds(&reported[0]);
        assert_eq!(
            (start, end),
            (
                month_start_offset(start_months),
                month_start_offset(end_months)
            ),
            "case {case}: expected the budget's own period, got {} .. {}",
            civil(start),
            civil(end)
        );
    }
}

/// RULE: a history the service cannot report is refused with the modelled error for the
/// reason it cannot — never with an empty history.
#[tokio::test]
async fn a_history_the_service_cannot_report_is_refused() {
    let router = router();
    create(
        &router,
        "refusals",
        &json!({
            "BudgetName": "annual-budget",
            "BudgetType": "COST",
            "TimeUnit": "ANNUALLY",
            "BudgetLimit": {"Amount": "1000", "Unit": "USD"},
            "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
        }),
    )
    .await;
    create(
        &router,
        "refusals",
        &json!({
            "BudgetName": "monthly-budget",
            "BudgetType": "COST",
            "TimeUnit": "MONTHLY",
            "BudgetLimit": {"Amount": "1000", "Unit": "USD"},
            "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
        }),
    )
    .await;

    let rows = vec![
        // "Budget history isn't available for ANNUAL budgets"
        // (`DescribeBudgetPerformanceHistory`).
        (
            "an annual budget",
            ask("annual-budget"),
            400,
            "InvalidParameterException",
        ),
        // The service answers every request refusal at HTTP 400 whatever 4xx its error
        // shape declares, keeping the modelled error code — the rule
        // `tests/all/client_error_envelopes.rs` pins, which holds for the authorization
        // refusal too.
        (
            "a budget that does not exist",
            ask("no-such-budget"),
            400,
            "NotFoundException",
        ),
        (
            "another account's budget",
            json!({"AccountId": account(210_987_654_321), "BudgetName": "monthly-budget"}),
            400,
            "AccessDeniedException",
        ),
        (
            "a window whose start follows its end",
            json!({
                "AccountId": caller_account(),
                "BudgetName": "monthly-budget",
                "TimePeriod": {"Start": month_start_offset(0), "End": month_start_offset(-2)},
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a window that is a single instant",
            json!({
                "AccountId": caller_account(),
                "BudgetName": "monthly-budget",
                "TimePeriod": {"Start": month_start_offset(-1), "End": month_start_offset(-1)},
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a budget name outside its pattern",
            json!({"AccountId": caller_account(), "BudgetName": "bad:name"}),
            400,
            "ValidationException",
        ),
    ];

    for (case, request, status, exception) in rows {
        let (answered, body) = history(&router, &request).await;
        assert_eq!(
            answered, status,
            "case {case}: expected HTTP {status}, got {answered} {body}"
        );
        assert!(
            is_error(&body, exception),
            "case {case}: expected {exception}, got {body}"
        );
    }
}

/// RULE: reading a performance history reports, it does not write — a second read, and a
/// read of the budget itself, see exactly what the first read saw.
#[tokio::test]
async fn reading_a_history_changes_nothing_a_later_caller_sees() {
    let router = router();
    for (case, time_unit) in [
        ("daily", "DAILY"),
        ("monthly", "MONTHLY"),
        ("quarterly", "QUARTERLY"),
    ] {
        let name = format!("purity-{case}");
        create(
            &router,
            case,
            &json!({
                "BudgetName": name,
                "BudgetType": "COST",
                "TimeUnit": time_unit,
                "BudgetLimit": {"Amount": "9", "Unit": "USD"},
                "TimePeriod": {"Start": civil_secs(LONG_RUNNING_START)},
            }),
        )
        .await;

        let read_budget = json!({"AccountId": caller_account(), "BudgetName": name});
        let (_, before) = call(&router, "DescribeBudget", &read_budget).await;
        let (_, first) = history(&router, &ask(&name)).await;
        let (_, second) = history(&router, &ask(&name)).await;
        let (_, after) = call(&router, "DescribeBudget", &read_budget).await;

        assert_eq!(
            first, second,
            "case {case}: expected a repeated read to report the same history, \
             got {first} then {second}"
        );
        assert_eq!(
            before, after,
            "case {case}: expected the budget to be unchanged by the read, \
             got {before} then {after}"
        );
    }
}
