// SPDX-License-Identifier: CC0-1.0

//! The `TimePeriod` ordering rule over the period a write LEAVES STORED, not only over
//! the bounds the request spelled out.
//!
//! "You set the start date and end date. The start date must come before the end date"
//! and, in the same paragraph, "If you create your budget and don't specify a start date,
//! Amazon Web Services defaults to the start of your chosen time period ... If you didn't
//! specify an end date, Amazon Web Services set your end date to `06/15/87 00:00 UTC`"
//! (`Budget$TimePeriod`, from
//! `smithy_query(query="doc",shape="Budget",member="TimePeriod")`). The rule is the
//! SHAPE's, so it holds for the budget a write path stores — a bound the service itself
//! supplied is held to it exactly like one the caller sent, and no accepted write can
//! leave a stored budget whose `Start` comes after its `End`.
//!
//! Each test states that rule and asserts it over a table whose rows differ along which
//! bound the request carries (`Start` only, `End` only, both, neither, no `TimePeriod` at
//! all), the instant each bound names relative to the two defaults, the sub-second part of
//! an instant, and — for the defaulted `Start`, which the budget's granularity decides —
//! every member of the generated `TimeUnit` catalog (`TIME_UNIT_VALUES`, so the table
//! cannot drift from the model).
//!
//! The harness (`router`, `call`, `caller_account`, `is_error`, `civil_secs`) is the one
//! `tests/all/budget_lifecycle.rs` builds.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::SystemTime;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::TIME_UNIT_VALUES;
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::default_time_period_start;

use super::budget_lifecycle::{caller_account, call, civil_secs, is_error, router};
use super::client_error_envelopes::message;

/// "If you didn't specify an end date, Amazon Web Services set your end date to `06/15/87
/// 00:00 UTC`", which is also the last instant an end date may name ("The end date must
/// come before `06/15/87 00:00 UTC`") — `Budget$TimePeriod`. Written as the date it is.
fn default_end() -> i64 {
    civil_secs("2087-06-15T00:00:00Z")
}

/// A day in seconds — the span a row steps an instant by so the table does not depend on
/// one gap size.
const DAY: i64 = 86_400;

/// What a write of a budget carrying a given period must answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Expect {
    /// Stored — and read back covering a period whose `Start` comes before its `End`.
    Accepted,
    /// Refused because the period the write would leave is the wrong way round.
    Unordered,
    /// Refused because the `End` the caller named comes after the documented last
    /// instant.
    EndPastBound,
}

/// The sentence a refused write answers with, in `InvalidParameterException$Message`.
fn refusal_message(expect: Expect) -> &'static str {
    match expect {
        Expect::Accepted => "",
        Expect::Unordered => "Unable to create/update budget - end time has to be after start time.",
        Expect::EndPastBound => {
            "Unable to create/update budget: TimePeriod End must not come after \
             06/15/87 00:00 UTC."
        }
    }
}

/// One row: a case name, the budget's granularity, the `TimePeriod` member as the request
/// carries it (`None` for a request with no `TimePeriod` at all) and the verdict.
struct Row {
    case: String,
    time_unit: &'static str,
    period: Option<Value>,
    expect: Expect,
}

fn row(case: impl Into<String>, time_unit: &'static str, period: Value, expect: Expect) -> Row {
    Row {
        case: case.into(),
        time_unit,
        period: Some(period),
        expect,
    }
}

/// The rows whose verdict the DEFAULTED `End` decides: the request names a `Start` and no
/// `End`, so the period the write leaves runs from that `Start` to `06/15/87 00:00 UTC`.
/// The instants span the documented default on both sides, by a sub-second, a second and a
/// day, and include the far-future instant the issue recorded.
fn start_only_rows() -> Vec<Row> {
    let bound = default_end();
    vec![
        row(
            "Start only, a year before the default End",
            "MONTHLY",
            json!({"Start": civil_secs("2086-06-15T00:00:00Z")}),
            Expect::Accepted,
        ),
        row(
            "Start only, one second before the default End",
            "DAILY",
            json!({"Start": bound - 1}),
            Expect::Accepted,
        ),
        row(
            "Start only, exactly the default End",
            "MONTHLY",
            json!({"Start": bound}),
            Expect::Unordered,
        ),
        row(
            "Start only, a fraction of a second after the default End",
            "CUSTOM",
            json!({"Start": bound as f64 + 0.5}),
            Expect::Unordered,
        ),
        row(
            "Start only, one second after the default End",
            "QUARTERLY",
            json!({"Start": bound + 1}),
            Expect::Unordered,
        ),
        row(
            "Start only, a day after the default End",
            "ANNUALLY",
            json!({"Start": bound + DAY}),
            Expect::Unordered,
        ),
        row(
            "Start only, the recorded far-future instant",
            "MONTHLY",
            json!({"Start": civil_secs("2100-01-01T00:00:00Z")}),
            Expect::Unordered,
        ),
    ]
}

/// The rows whose verdict the DEFAULTED `Start` decides: the request names an `End` and no
/// `Start`, so the period the write leaves begins at the start of the period the call
/// falls in, which the budget's `TimeUnit` names. Those boundaries are asserted instant by
/// instant by `tests/all/budget_read_projection.rs`; here each one is taken from the
/// service's own `default_time_period_start` so a row names the exact edge, and the two
/// refused rows stand whatever boundary the write itself computes (a later boundary is
/// still after the `End` they name).
fn end_only_rows(now: SystemTime) -> Vec<Row> {
    let mut rows = vec![
        row(
            "End only, the Unix epoch",
            "DAILY",
            json!({"End": 0}),
            Expect::Unordered,
        ),
        row(
            "End only, a decade before this call",
            "MONTHLY",
            json!({"End": civil_secs("2016-01-01T00:00:00Z")}),
            Expect::Unordered,
        ),
        row(
            "End only, well after this call",
            "QUARTERLY",
            json!({"End": civil_secs("2086-01-01T00:00:00Z")}),
            Expect::Accepted,
        ),
    ];
    // The whole generated catalog, since the defaulted `Start` is the boundary the
    // budget's own granularity names. (A `TimeUnit` outside the catalog never reaches a
    // handler: it is refused at request validation against the modelled enum.)
    for time_unit in TIME_UNIT_VALUES.iter().copied() {
        let boundary = default_time_period_start(time_unit, now).secs();
        rows.push(row(
            format!("{time_unit}: End only, exactly the start of the period this call falls in"),
            time_unit,
            json!({"End": boundary}),
            Expect::Unordered,
        ));
        rows.push(row(
            format!("{time_unit}: End only, one second before that period's start"),
            time_unit,
            json!({"End": boundary - 1}),
            Expect::Unordered,
        ));
        // A day past this call is after every boundary at or before it, whichever
        // granularity named it.
        rows.push(row(
            format!("{time_unit}: End only, a day after this call"),
            time_unit,
            json!({"End": epoch_secs(now) + DAY}),
            Expect::Accepted,
        ));
    }
    rows
}

/// The rows the request spells both bounds out for, and the ones it leaves the whole
/// period to the service: the verdict a period gets does not depend on WHICH side the
/// caller supplied.
fn both_or_neither_rows() -> Vec<Row> {
    let bound = default_end();
    let mut rows = vec![
        row(
            "both bounds, Start before End",
            "MONTHLY",
            json!({"Start": civil_secs("2024-01-01T00:00:00Z"), "End": civil_secs("2086-01-01T00:00:00Z")}),
            Expect::Accepted,
        ),
        row(
            "both bounds, End a year before Start",
            "MONTHLY",
            json!({"Start": civil_secs("2025-01-01T00:00:00Z"), "End": civil_secs("2024-01-01T00:00:00Z")}),
            Expect::Unordered,
        ),
        row(
            "both bounds, End equal to Start",
            "DAILY",
            json!({"Start": civil_secs("2024-01-01T00:00:00Z"), "End": civil_secs("2024-01-01T00:00:00Z")}),
            Expect::Unordered,
        ),
        row(
            "both bounds, End one second after the documented bound",
            "MONTHLY",
            json!({"Start": civil_secs("2024-01-01T00:00:00Z"), "End": bound + 1}),
            Expect::EndPastBound,
        ),
        row(
            "both bounds, Start and End both after the documented bound",
            "MONTHLY",
            json!({"Start": civil_secs("2100-01-01T00:00:00Z"), "End": civil_secs("2086-06-15T00:00:00Z")}),
            Expect::Unordered,
        ),
        row(
            "an empty TimePeriod, both bounds defaulted",
            "ANNUALLY",
            json!({}),
            Expect::Accepted,
        ),
    ];
    rows.push(Row {
        case: "no TimePeriod at all".to_string(),
        time_unit: "MONTHLY",
        period: None,
        expect: Expect::Accepted,
    });
    rows
}

/// `at` as whole epoch seconds.
fn epoch_secs(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(SystemTime::UNIX_EPOCH)
            .expect("this call is after the Unix epoch")
            .as_secs(),
    )
    .expect("an epoch this host can represent")
}

/// The budget a row writes: a cost budget with a fixed limit, so the only rule in play is
/// the one over its period.
fn budget(name: &str, row: &Row) -> Value {
    let mut budget = json!({
        "BudgetName": name,
        "TimeUnit": row.time_unit,
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "100", "Unit": "USD"}
    });
    if let Some(period) = row.period.as_ref() {
        budget["TimePeriod"] = period.clone();
    }
    budget
}

/// A well-formed budget the update path starts from, so a refused `UpdateBudget` has
/// something to have left alone.
fn baseline(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": {"Amount": "1", "Unit": "USD"},
        "TimePeriod": {
            "Start": civil_secs("2024-01-01T00:00:00Z"),
            "End": civil_secs("2086-01-01T00:00:00Z")
        }
    })
}

/// The `DescribeBudget` body for `name`.
async fn describe(store: &MultiServiceRouter, account_id: &str, name: &str) -> Value {
    let (_, body) = call(
        store,
        "DescribeBudget",
        &json!({"AccountId": account_id, "BudgetName": name}),
    )
    .await;
    body
}

/// Assert that the budget `read` reports covers a period whose `Start` comes before its
/// `End` — the shape the model says a stored budget is always in.
fn assert_stored_period_ordered(case: &str, read: &Value) {
    let period = &read["Budget"]["TimePeriod"];
    let start = period["Start"].as_f64();
    let end = period["End"].as_f64();
    let (Some(start), Some(end)) = (start, end) else {
        panic!("case {case:?}: expected the stored budget to cover a period with both bounds, got {read}");
    };
    assert!(
        start < end,
        "case {case:?}: expected the stored Start to come before the stored End, got \
         Start {start} and End {end} in {read}"
    );
}

/// Assert the verdict `row` expects from `CreateBudget`.
async fn check_create(row: &Row) {
    let store = router();
    let account_id = caller_account();
    let name = "stored-period-ordering";
    let case = &row.case;
    let (status, body) = call(
        &store,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": budget(name, row)}),
    )
    .await;
    let read = describe(&store, &account_id, name).await;
    if row.expect == Expect::Accepted {
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
        );
        assert_stored_period_ordered(case, &read);
    } else {
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateBudget to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        let expected = refusal_message(row.expect);
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
        assert!(
            is_error(&read, "NotFoundException"),
            "case {case:?}: expected a refused CreateBudget to store nothing, got {read}"
        );
    }
}

/// Assert the same verdict from `UpdateBudget`, over a budget that already exists: the
/// rule is the shape's, so both write paths answer one and the same period the same way,
/// and a refused update leaves the stored budget as it was.
async fn check_update(row: &Row) {
    let store = router();
    let account_id = caller_account();
    let name = "stored-period-ordering";
    let case = &row.case;
    let (status, body) = call(
        &store,
        "CreateBudget",
        &json!({"AccountId": &account_id, "Budget": baseline(name)}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the baseline budget must be created, got {status} {body}"
    );
    let before = describe(&store, &account_id, name).await;
    let (status, body) = call(
        &store,
        "UpdateBudget",
        &json!({"AccountId": &account_id, "NewBudget": budget(name, row)}),
    )
    .await;
    let after = describe(&store, &account_id, name).await;
    if row.expect == Expect::Accepted {
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
        );
        assert_stored_period_ordered(case, &after);
    } else {
        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateBudget to answer 400, got {status} {body}"
        );
        assert!(
            is_error(&body, "InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {body}"
        );
        let expected = refusal_message(row.expect);
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
        assert_eq!(
            before["Budget"]["TimePeriod"], after["Budget"]["TimePeriod"],
            "case {case:?}: expected a refused UpdateBudget to leave the stored period \
             alone, got {after}"
        );
        assert_stored_period_ordered(case, &after);
    }
}

/// RULE: a write is judged by the period it LEAVES stored — every bound the request
/// carried, plus the documented default for each it did not — so a budget whose `Start`
/// would come at or after its `End` is refused however few of the two bounds the request
/// spelled out, and every accepted write reads back covering a period whose `Start` comes
/// before its `End`.
#[tokio::test]
async fn the_ordering_rule_holds_over_the_period_a_write_leaves_stored() {
    let now = SystemTime::now();
    let rows = start_only_rows()
        .into_iter()
        .chain(end_only_rows(now))
        .chain(both_or_neither_rows());
    for row in rows {
        check_create(&row).await;
        check_update(&row).await;
    }
}

/// RULE, the consequence the issue names: a budget covering a well-formed period has a
/// performance history over the periods it retains, which a stored `Start`-after-`End`
/// period could never report (no window whose start is after its end overlaps anything).
/// The period a write leaves is always well formed, so the history of a budget written
/// with a `Start` the caller named and no `End` is not empty.
#[tokio::test]
async fn a_budget_written_with_a_start_and_no_end_reports_its_performance_history() {
    let store = router();
    let account_id = caller_account();
    let name = "history-over-defaulted-end";
    // A start in the past, so the retained periods of a MONTHLY budget are behind us and
    // the defaulted End (06/15/87 00:00 UTC) is still ahead.
    let (status, body) = call(
        &store,
        "CreateBudget",
        &json!({
            "AccountId": &account_id,
            "Budget": {
                "BudgetName": name,
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
                "TimePeriod": {"Start": civil_secs("2024-01-01T00:00:00Z")}
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case start-and-no-end: expected CreateBudget to answer 200, got {status} {body}"
    );
    let read = describe(&store, &account_id, name).await;
    assert_stored_period_ordered("start-and-no-end", &read);
    let (status, body) = call(
        &store,
        "DescribeBudgetPerformanceHistory",
        &json!({"AccountId": &account_id, "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case start-and-no-end: expected the history read to answer 200, got {status} {body}"
    );
    let amounts = body["BudgetPerformanceHistory"]["BudgetedAndActualAmountsList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        !amounts.is_empty(),
        "case start-and-no-end: expected the retained periods to be reported, got {body}"
    );
}
