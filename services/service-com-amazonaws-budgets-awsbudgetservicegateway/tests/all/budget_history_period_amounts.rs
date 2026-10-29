// SPDX-License-Identifier: CC0-1.0

//! What each record of a budget's performance history REPORTS, over the periods a budget
//! can cover.
//!
//! The RULE these tables assert: every `BudgetedAndActualAmounts` record of a performance
//! history carries the two figures a READ of the same budget reports as of the period's
//! `Start` — the limit in force at that instant and the cost or usage attributed to the
//! budget, measured in that limit's unit — whatever instant the `Start` names, and the ONLY
//! record that reports neither figure is one for a budget whose read reports no limit at
//! all.
//!
//! The first table spans that class over the periods a budget can be created with: its
//! rows differ along where the period's `Start` falls relative to the Unix epoch (far
//! before it, before it, exactly on it, after it, in the month in progress), the budget
//! type and the unit its amounts are measured in, the amount itself, whether the period
//! names its own `End` or takes the documented default, and whether the budget's read
//! reports a limit at all.
//!
//! The second table spans the same rule over the instants a read projection is asked
//! about, including ones no host clock value can name.
//!
//! The harness (`router`, `call`, `civil_secs`, …) is the one `tests/all/budget_lifecycle.rs`
//! builds, and the history helpers are `tests/all/budget_performance_history.rs`' own.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{Budget, Spend};
use service_com_amazonaws_budgets_awsbudgetservicegateway::state::{
    budget_as_read_at_epoch, ATTRIBUTED_SPEND_AMOUNT,
};

use super::budget_lifecycle::{call, caller_account, civil_secs, router, same};
use super::budget_performance_history::{ask, bounds, create, history, records};
use super::budget_read_projection::{civil, month_start_offset};

/// `06/15/87 00:00 UTC` — the end date a budget created without one is given ("If you
/// didn't specify an end date, Amazon Web Services set your end date to `06/15/87 00:00
/// UTC`", `TimePeriod$End`), written as the date it is.
const DEFAULT_END_CIVIL: &str = "2087-06-15T00:00:00Z";

/// The time unit whose history is the single `TimePeriod` the caller chose, so a row can
/// put that period wherever it likes on the calendar: `TimeUnit$CUSTOM` names no repeating
/// period (`smithy_query(query="shape",shape="TimeUnit")` lists it, and
/// `smithy_query(query="shape",shape="TimePeriod")` bounds `Start` nowhere). Every other
/// granularity reports the periods around the clock, which
/// `tests/all/budget_performance_history.rs` already spans.
const CUSTOM: &str = "CUSTOM";

/// One row of the period table.
struct Row {
    case: &'static str,
    /// The `TimePeriod$Start` the budget is created with, in epoch seconds.
    start: i64,
    /// The `TimePeriod$End` it is created with, or `None` to take the documented default.
    end: Option<i64>,
    /// The members that make the budget what it is, beyond its name and time period.
    members: Value,
    /// The `(Amount, Unit)` the record's `BudgetedAmount` carries, or `None` for a budget
    /// whose read reports no limit at all.
    budgeted: Option<(&'static str, &'static str)>,
}

/// The period table. Every row is a budget whose whole history is the one period it names,
/// so the row's `Start` is exactly the instant the record is projected as of.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a start far before the Unix epoch",
            start: civil_secs("1900-01-01T00:00:00Z"),
            end: None,
            members: json!({
                "BudgetType": "USAGE",
                "BudgetLimit": {"Amount": "2048", "Unit": "GB"},
            }),
            budgeted: Some(("2048", "GB")),
        },
        Row {
            case: "a start before the Unix epoch",
            start: civil_secs("1960-01-01T00:00:00Z"),
            end: None,
            members: json!({
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
            }),
            budgeted: Some(("100", "USD")),
        },
        Row {
            case: "a start exactly at the Unix epoch",
            start: civil_secs("1970-01-01T00:00:00Z"),
            end: None,
            members: json!({
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "0.5", "Unit": "EUR"},
            }),
            budgeted: Some(("0.5", "EUR")),
        },
        Row {
            case: "a start after the Unix epoch, with an end of its own",
            start: civil_secs("1980-01-01T00:00:00Z"),
            end: Some(month_start_offset(2)),
            members: json!({
                "BudgetType": "COST",
                "BudgetLimit": {"Amount": "100", "Unit": "USD"},
            }),
            budgeted: Some(("100", "USD")),
        },
        Row {
            case: "a start in the month in progress",
            start: month_start_offset(0),
            end: Some(month_start_offset(3)),
            members: json!({
                "BudgetType": "RI_COVERAGE",
                "BudgetLimit": {"Amount": "100", "Unit": "PERCENTAGE"},
            }),
            budgeted: Some(("100", "PERCENTAGE")),
        },
        Row {
            // The one case that reports neither figure: an auto-adjusting budget's amount
            // is the average of cost data this emulator does not hold, so its read reports
            // no limit — and with no limit, nothing in the record names a unit to measure a
            // spend in.
            case: "a pre-epoch start on a budget whose read reports no limit",
            start: civil_secs("1960-01-01T00:00:00Z"),
            end: None,
            members: json!({
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": 1},
                },
            }),
            budgeted: None,
        },
    ]
}

/// The `Budget` a read of `name` reports.
async fn read(router: &MultiServiceRouter, case: &str, name: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeBudget",
        &json!({"AccountId": caller_account(), "BudgetName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: expected DescribeBudget to answer 200, got {status} {body}"
    );
    body["Budget"].clone()
}

/// A member of a record or a read, or `Value::Null` when it carries none.
fn member(value: &Value, path: &[&str]) -> Value {
    let mut found = value.clone();
    for step in path {
        found = found.get(step).cloned().unwrap_or(Value::Null);
    }
    found
}

/// RULE: each record of a budget's performance history reports the figures a read of the
/// same budget reports as of the period's `Start`, whatever instant that `Start` names —
/// and only a budget whose read reports no limit reports neither.
#[tokio::test]
async fn every_period_reports_the_amounts_the_budgets_own_read_reports() {
    for row in rows() {
        // One store per row, so rows cannot see each other's budgets: every request names
        // the one account this service serves.
        let router = router();
        let name = "history-amounts";
        let mut budget = row.members.clone();
        budget["BudgetName"] = json!(name);
        budget["TimeUnit"] = json!(CUSTOM);
        let mut period = json!({"Start": row.start});
        if let Some(end) = row.end {
            period["End"] = json!(end);
        }
        budget["TimePeriod"] = period;
        create(&router, row.case, &budget).await;

        let read = read(&router, row.case, name).await;
        let (status, body) = history(&router, &ask(name)).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200, got {status} {body}",
            row.case
        );
        let reported = records(&body);
        assert_eq!(
            reported.len(),
            1,
            "case {}: expected the one period the budget covers, got {reported:?}",
            row.case
        );
        let record = &reported[0];

        // The record is about the period the budget covers, bounds included: a start the
        // host's own clock type cannot hold is still the period it was stored with.
        let (start, end) = bounds(record);
        assert_eq!(
            (start, end),
            (
                row.start,
                row.end.unwrap_or_else(|| civil_secs(DEFAULT_END_CIVIL))
            ),
            "case {}: expected the budget's own period, got {} .. {}",
            row.case,
            civil(start),
            civil(end)
        );

        // The two figures AGREE with the single read of the same budget: the limit it
        // reports, and the spend it reports for that limit.
        let reported_limit = member(&read, &["BudgetLimit"]);
        let reported_spend = member(&read, &["CalculatedSpend", "ActualSpend"]);
        let budgeted = member(record, &["BudgetedAmount"]);
        let actual = member(record, &["ActualAmount"]);
        assert!(
            same(&budgeted, &reported_limit),
            "case {}: expected the BudgetedAmount to be the limit the read reports \
             ({reported_limit}), got {budgeted}",
            row.case
        );
        assert!(
            same(&actual, &reported_spend),
            "case {}: expected the ActualAmount to be the spend the read reports \
             ({reported_spend}), got {actual}",
            row.case
        );

        // And they are the figures the row states, so the agreement above cannot pass on
        // two reads that are both wrong.
        match row.budgeted {
            Some((amount, unit)) => {
                assert!(
                    same(&budgeted, &json!({"Amount": amount, "Unit": unit})),
                    "case {}: expected the budgeted amount {amount} {unit}, got {budgeted}",
                    row.case
                );
                assert!(
                    same(
                        &actual,
                        &json!({"Amount": ATTRIBUTED_SPEND_AMOUNT, "Unit": unit})
                    ),
                    "case {}: expected the attributed spend in {unit}, got {actual}",
                    row.case
                );
            }
            None => {
                assert!(
                    budgeted.is_null() && actual.is_null(),
                    "case {}: expected neither figure for a budget whose read reports no \
                     limit, got {budgeted} and {actual}",
                    row.case
                );
            }
        }

        // A history REPORTS; asking again answers the same record.
        let (status, again) = history(&router, &ask(name)).await;
        assert_eq!(
            status, 200,
            "case {}: expected a second read to answer 200, got {status} {again}",
            row.case
        );
        let repeated = records(&again);
        assert!(
            repeated.len() == 1 && same(&repeated[0], record),
            "case {}: expected a second read to report {record}, got {repeated:?}",
            row.case
        );
    }
}

/// The planned entries the projection table is asserted over: an amount and a unit per
/// entry, keyed by the start of a month — "The `key` is the start of the month, `UTC` in
/// epoch seconds" (`Budget$PlannedBudgetLimits`). A unit per entry, so an assertion cannot
/// pass on one limit the projection reports for every instant.
const PLANNED: [(i32, &str, &str); 3] = [(-3, "30", "USD"), (-1, "60", "EUR"), (0, "90", "GBP")];

/// RULE: the read projection answers for the instant it is ASKED about, whatever instant
/// that is — reporting the planned limit in force then, and the attributed spend in that
/// limit's unit.
///
/// Asserted against the projection helper directly, the way
/// `tests/all/budget_read_projection.rs` asserts the derivation for a planned key no write
/// path would store: a `PlannedBudgetLimits` map "is available for cost or usage budget and
/// supports both monthly and quarterly `TimeUnit`", and the history of such a budget
/// reports the periods around the clock — so the far ends of the rule, including instants
/// no host clock value can name, are reachable only here.
#[test]
fn the_read_projection_answers_for_every_instant_it_is_asked_about() {
    let planned: std::collections::HashMap<String, Spend> = PLANNED
        .iter()
        .map(|(months, amount, unit)| {
            (
                month_start_offset(*months).to_string(),
                Spend {
                    amount: (*amount).to_string(),
                    unit: (*unit).to_string(),
                },
            )
        })
        .collect();
    let budget = Budget {
        budget_name: "planned-projection".to_string(),
        time_unit: "MONTHLY".to_string(),
        budget_type: "COST".to_string(),
        planned_budget_limits: Some(planned),
        ..Budget::default()
    };

    // Rows differ along where the instant falls: below every instant there is, before the
    // Unix epoch, on it, before the first entry, on an entry, between two entries, on the
    // last, past it, and above every instant a clock value can name. The entry in force is
    // the latest one that has started, and a budget whose entries have all still to start
    // reports its first.
    let rows: Vec<(String, i64, (&str, &str))> = vec![
        (
            "the earliest instant there is".to_string(),
            i64::MIN,
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "far before the Unix epoch".to_string(),
            civil_secs("1900-01-01T00:00:00Z"),
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "the Unix epoch itself".to_string(),
            civil_secs("1970-01-01T00:00:00Z"),
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "the month before the first entry".to_string(),
            month_start_offset(-4),
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "the first entry's own month".to_string(),
            month_start_offset(-3),
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "a month between two entries".to_string(),
            month_start_offset(-2),
            (PLANNED[0].1, PLANNED[0].2),
        ),
        (
            "the second entry's own month".to_string(),
            month_start_offset(-1),
            (PLANNED[1].1, PLANNED[1].2),
        ),
        (
            "the month in progress".to_string(),
            month_start_offset(0),
            (PLANNED[2].1, PLANNED[2].2),
        ),
        (
            "a month past the last entry".to_string(),
            month_start_offset(3),
            (PLANNED[2].1, PLANNED[2].2),
        ),
        (
            "the latest instant there is".to_string(),
            i64::MAX,
            (PLANNED[2].1, PLANNED[2].2),
        ),
    ];

    for (case, at, (amount, unit)) in rows {
        let projected = budget_as_read_at_epoch(&budget, at);
        let limit = projected
            .budget_limit
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: expected a limit in force at {at}"));
        assert_eq!(
            (limit.amount.as_str(), limit.unit.as_str()),
            (amount, unit),
            "case {case:?}: expected the limit {amount} {unit} in force at {at}, got \
             {limit:?}"
        );
        let spend = projected
            .calculated_spend
            .as_ref()
            .unwrap_or_else(|| panic!("case {case:?}: expected a spend reported at {at}"));
        assert_eq!(
            (
                spend.actual_spend.amount.as_str(),
                spend.actual_spend.unit.as_str()
            ),
            (ATTRIBUTED_SPEND_AMOUNT, unit),
            "case {case:?}: expected the attributed spend in {unit} at {at}, got {:?}",
            spend.actual_spend
        );
    }
}
