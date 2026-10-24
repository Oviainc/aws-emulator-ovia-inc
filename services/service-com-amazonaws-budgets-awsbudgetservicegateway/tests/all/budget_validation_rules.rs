// SPDX-License-Identifier: CC0-1.0

//! The documented rules a `Budget` must satisfy, asserted on BOTH write paths.
//!
//! `CreateBudget$Budget` and `UpdateBudget$NewBudget` carry the same shape, so every
//! rule the model documents for a budget holds for both calls: a budget the create path
//! refuses is a budget the update path refuses, and a refused update leaves the stored
//! budget exactly as it was.
//!
//! Each test states one RULE and asserts it over a table whose rows differ along what
//! the rule generalises over — the budget type, the time unit, the member carrying the
//! limit, the instant, the adjustment period, the form of a planned-limit key. The
//! requests the issue recorded are single rows.
//!
//! The harness (`router`, `call`, `account`, `is_error`, `same`, `carries`) is the one
//! `tests/all/budget_lifecycle.rs` builds, so this suite drives the service exactly the
//! way the others do.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::cell::Cell;

use service_com_amazonaws_budgets_awsbudgetservicegateway::ops::types::{
    BUDGET_TYPE_VALUES, TIME_UNIT_VALUES,
};

use super::budget_lifecycle::{account, call, carries, civil_secs, is_error, router, same};

/// What a write of a given budget must answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    /// Stored, and read back carrying what was sent.
    Accepted,
    /// Refused with the modelled `InvalidParameterException` (HTTP 400) — a rule the
    /// model documents in prose, which no constraint trait can express.
    Refused,
    /// Refused at request validation (HTTP 400 `ValidationException`) — a value outside
    /// a constraint the MODEL declares, so the handler is never reached.
    OutsideModelledConstraint,
}

/// The instant `YYYY-MM-DDThh:mm:ssZ` names, in epoch seconds. Every date in this suite
/// is written as the date it is rather than as an epoch literal; the conversion is the
/// harness's shared [`civil_secs`].
fn at(civil: &str) -> i64 {
    civil_secs(civil)
}

/// A `Spend` of `amount` US dollars.
fn spend(amount: &str) -> Value {
    json!({"Amount": amount, "Unit": "USD"})
}

/// The valid budget the update path starts from: a monthly cost budget with a fixed
/// limit. A refused `UpdateBudget` has to leave exactly this behind.
fn baseline(name: &str) -> Value {
    json!({
        "BudgetName": name,
        "TimeUnit": "MONTHLY",
        "BudgetType": "COST",
        "BudgetLimit": spend("100")
    })
}

/// One service, plus the account seed that keeps each row's budgets out of every other
/// row's account.
struct Harness {
    router: MultiServiceRouter,
    next: Cell<u64>,
}

impl Harness {
    fn new() -> Self {
        Self {
            router: router(),
            next: Cell::new(0),
        }
    }

    /// A fresh `(create-path, update-path)` account pair.
    fn accounts(&self) -> (String, String) {
        let seed = self.next.get();
        self.next.set(seed + 1);
        (account(600_000_000 + seed), account(700_000_000 + seed))
    }

    /// The `DescribeBudget` body for `name` under `account_id`.
    async fn describe(&self, account_id: &str, name: &str) -> Value {
        let (_, body) = call(
            &self.router,
            "DescribeBudget",
            &json!({"AccountId": account_id, "BudgetName": name}),
        )
        .await;
        body
    }

    /// Drive `budget` through BOTH write paths and assert each answers `expect`.
    ///
    /// On the create path an accepted budget reads back and a refused one stores
    /// nothing. On the update path the budget already exists as [`baseline`], so an
    /// accepted budget replaces it and reads back, while a refused one leaves the stored
    /// budget byte-for-byte as it was.
    async fn check(&self, case: &str, budget: &Value, expect: Verdict) {
        let name = budget["BudgetName"]
            .as_str()
            .expect("every case names its budget")
            .to_string();
        let exception = match expect {
            Verdict::Accepted => "",
            Verdict::Refused => "InvalidParameterException",
            Verdict::OutsideModelledConstraint => "ValidationException",
        };
        let (creating, updating) = self.accounts();

        // The create path.
        let (status, body) = call(
            &self.router,
            "CreateBudget",
            &json!({"AccountId": &creating, "Budget": budget}),
        )
        .await;
        let read = self.describe(&creating, &name).await;
        if expect == Verdict::Accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBudget to answer 200, got {status} {body}"
            );
            assert!(
                carries(budget, &read["Budget"]),
                "case {case:?}: expected the created budget to carry {budget}, got {read}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected CreateBudget to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, exception),
                "case {case:?}: expected {exception} from CreateBudget, got {body}"
            );
            assert!(
                is_error(&read, "NotFoundException"),
                "case {case:?}: expected a refused CreateBudget to store nothing, got {read}"
            );
        }

        // The update path, over a budget that already exists.
        let (status, body) = call(
            &self.router,
            "CreateBudget",
            &json!({"AccountId": &updating, "Budget": baseline(&name)}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the baseline budget must be created, got {status} {body}"
        );
        let before = self.describe(&updating, &name).await["Budget"].clone();
        let (status, body) = call(
            &self.router,
            "UpdateBudget",
            &json!({"AccountId": &updating, "NewBudget": budget}),
        )
        .await;
        let after = self.describe(&updating, &name).await["Budget"].clone();
        if expect == Verdict::Accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateBudget to answer 200, got {status} {body}"
            );
            assert!(
                carries(budget, &after),
                "case {case:?}: expected the updated budget to carry {budget}, got {after}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected UpdateBudget to answer 400, got {status} {body}"
            );
            assert!(
                is_error(&body, exception),
                "case {case:?}: expected {exception} from UpdateBudget, got {body}"
            );
            assert!(
                same(&before, &after),
                "case {case:?}: expected a refused UpdateBudget to leave {before} stored, \
                 got {after}"
            );
        }
    }
}

/// RULE: a budget whose type tracks cost or usage must carry a limit — "`BudgetLimit` is
/// required for cost or usage budgets, but optional for RI or Savings Plans utilization
/// or coverage budgets" (`Budget$BudgetLimit`) — and the limit may come from any member
/// that determines it: the fixed `BudgetLimit`, a non-empty `PlannedBudgetLimits` (which
/// a read derives the current `BudgetLimit` from), or the `AutoAdjustData` that
/// "determine[s] the budget amount for an auto-adjusting budget".
#[tokio::test]
async fn a_cost_or_usage_budget_must_carry_a_limit() {
    let harness = Harness::new();
    // The catalog is the generated `BUDGET_TYPE_VALUES`, so every modelled type is
    // covered, not only the recorded COST one.
    for budget_type in BUDGET_TYPE_VALUES {
        let requires_limit = matches!(*budget_type, "COST" | "USAGE");
        let expect = if requires_limit {
            Verdict::Refused
        } else {
            Verdict::Accepted
        };
        // The recorded case: a monthly budget with neither BudgetLimit nor
        // PlannedBudgetLimits.
        harness
            .check(
                &format!("{budget_type} with no limit at all"),
                &json!({
                    "BudgetName": "no-limit",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": budget_type
                }),
                expect,
            )
            .await;

        // Each member that can carry the limit satisfies the rule. `PlannedBudgetLimits`
        // is only available for cost or usage budgets, so it is offered only there (its
        // own rule has its own test below).
        let mut sources: Vec<(&str, Value)> = vec![
            ("a fixed BudgetLimit", json!({"BudgetLimit": spend("10")})),
            (
                "AutoAdjustData",
                json!({"AutoAdjustData": {"AutoAdjustType": "FORECAST"}}),
            ),
        ];
        if requires_limit {
            sources.push((
                "PlannedBudgetLimits",
                json!({
                    "PlannedBudgetLimits": {
                        at("2024-01-01T00:00:00Z").to_string(): spend("10")
                    }
                }),
            ));
        }
        for (source, member) in sources {
            let mut budget = json!({
                "BudgetName": "with-limit",
                "TimeUnit": "MONTHLY",
                "BudgetType": budget_type
            });
            for (name, value) in member.as_object().expect("a member map") {
                budget[name] = value.clone();
            }
            harness
                .check(
                    &format!("{budget_type} with {source}"),
                    &budget,
                    Verdict::Accepted,
                )
                .await;
        }
    }

    // An EMPTY planned map carries no limit, so a cost budget is not satisfied by it.
    harness
        .check(
            "COST with an empty PlannedBudgetLimits",
            &json!({
                "BudgetName": "empty-plan",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "PlannedBudgetLimits": {}
            }),
            Verdict::Refused,
        )
        .await;
}

/// RULE: a budget's end date must not come after `06/15/87 00:00 UTC` ("The end date
/// must come before `06/15/87 00:00 UTC`", `Budget$TimePeriod`) — which is also the end
/// date a budget written without one is given ("If you didn't specify an end date, Amazon
/// Web Services set your end date to `06/15/87 00:00 UTC`", `TimePeriod$End`), so that
/// instant itself is accepted.
#[tokio::test]
async fn a_time_period_end_after_the_documented_last_instant_is_refused() {
    let harness = Harness::new();
    let bound = at("2087-06-15T00:00:00Z");
    let start = at("2024-01-01T00:00:00Z");
    // Rows span the published bound and each side of it, in both the seconds-grid form
    // and as dates, plus the far-future end the issue recorded.
    let rows: Vec<(&str, i64, Verdict)> = vec![
        ("a year before the bound", at("2086-06-15T00:00:00Z"), Verdict::Accepted),
        ("one second before the bound", bound - 1, Verdict::Accepted),
        ("the published bound itself", bound, Verdict::Accepted),
        ("one second after the bound", bound + 1, Verdict::Refused),
        ("a day after the bound", bound + 86_400, Verdict::Refused),
        (
            "the recorded far-future end",
            at("2100-01-01T00:00:00Z"),
            Verdict::Refused,
        ),
    ];
    for (case, end, expect) in rows {
        harness
            .check(
                case,
                &json!({
                    "BudgetName": "period-end",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "BudgetLimit": spend("1"),
                    "TimePeriod": {"Start": start, "End": end}
                }),
                expect,
            )
            .await;
        // The same rule with no Start sent: the end is the caller's whatever the service
        // defaults the start to.
        harness
            .check(
                &format!("{case}, no Start sent"),
                &json!({
                    "BudgetName": "period-end-only",
                    "TimeUnit": "DAILY",
                    "BudgetType": "USAGE",
                    "BudgetLimit": spend("1"),
                    "TimePeriod": {"End": end}
                }),
                expect,
            )
            .await;
    }

    // A budget that sends no end at all takes the documented default, so it is accepted.
    harness
        .check(
            "no End sent",
            &json!({
                "BudgetName": "no-end",
                "TimeUnit": "MONTHLY",
                "BudgetType": "COST",
                "BudgetLimit": spend("1"),
                "TimePeriod": {"Start": start}
            }),
            Verdict::Accepted,
        )
        .await;
}

/// The maximum `BudgetAdjustmentPeriod` each `TimeUnit` granularity allows, written out
/// from `HistoricalOptions$BudgetAdjustmentPeriod`: "The maximum value depends on the
/// `TimeUnit` granularity of the budget: For the `DAILY` granularity, the maximum value
/// is `60`. For the `MONTHLY` granularity, the maximum value is `12`. For the
/// `QUARTERLY` granularity, the maximum value is `4`. For the `ANNUALLY` granularity,
/// the maximum value is `1`."
const DOCUMENTED_MAXIMUM_ADJUSTMENT_PERIOD: &[(&str, i32)] = &[
    ("DAILY", 60),
    ("MONTHLY", 12),
    ("QUARTERLY", 4),
    ("ANNUALLY", 1),
];

/// The modelled `@range` on `AdjustmentPeriod`
/// (`smithy_query(query="shape",shape="HistoricalOptions")`): a value outside it is
/// refused at request validation, before any of the rules below are reached.
const MODELLED_ADJUSTMENT_PERIOD_RANGE: (i32, i32) = (1, 60);

/// RULE: `HistoricalOptions` are "the parameters that define or describe the historical
/// data that your auto-adjusting budget is based on" (`AutoAdjustData$HistoricalOptions`),
/// so they are required by the `HISTORICAL` auto-adjust type and accepted for no other;
/// and the `BudgetAdjustmentPeriod` they carry must not exceed the maximum documented for
/// the budget's `TimeUnit` granularity.
#[tokio::test]
async fn auto_adjust_data_must_match_its_type_and_granularity() {
    let harness = Harness::new();

    // The pairing rule, over both members of `AutoAdjustType`.
    let pairings: Vec<(&str, Value, Verdict)> = vec![
        (
            "HISTORICAL with its historical options",
            json!({
                "AutoAdjustType": "HISTORICAL",
                "HistoricalOptions": {"BudgetAdjustmentPeriod": 12}
            }),
            Verdict::Accepted,
        ),
        (
            "HISTORICAL with no historical options",
            json!({"AutoAdjustType": "HISTORICAL"}),
            Verdict::Refused,
        ),
        (
            // The combination the issue recorded.
            "FORECAST with historical options",
            json!({
                "AutoAdjustType": "FORECAST",
                "HistoricalOptions": {"BudgetAdjustmentPeriod": 1}
            }),
            Verdict::Refused,
        ),
        (
            "FORECAST on its own",
            json!({"AutoAdjustType": "FORECAST"}),
            Verdict::Accepted,
        ),
        (
            "an AutoAdjustType outside the enum",
            json!({"AutoAdjustType": "GUESS"}),
            Verdict::OutsideModelledConstraint,
        ),
    ];
    for (case, auto_adjust, expect) in pairings {
        harness
            .check(
                case,
                &json!({
                    "BudgetName": "auto-adjust",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": "COST",
                    "AutoAdjustData": auto_adjust
                }),
                expect,
            )
            .await;
    }

    // The per-granularity maximum: each published bound, and one past it.
    let (modelled_min, modelled_max) = MODELLED_ADJUSTMENT_PERIOD_RANGE;
    for (time_unit, documented_max) in DOCUMENTED_MAXIMUM_ADJUSTMENT_PERIOD {
        let rows: Vec<(String, i32, Verdict)> = vec![
            (
                format!("{time_unit} at the modelled minimum {modelled_min}"),
                modelled_min,
                Verdict::Accepted,
            ),
            (
                format!("{time_unit} at its documented maximum {documented_max}"),
                *documented_max,
                Verdict::Accepted,
            ),
            (
                format!("{time_unit} one past its documented maximum"),
                documented_max + 1,
                // One past the per-granularity maximum is still inside the modelled
                // range for every granularity but DAILY, whose maximum IS that range's
                // top — there, one past it is a modelled-constraint violation.
                if documented_max + 1 > modelled_max {
                    Verdict::OutsideModelledConstraint
                } else {
                    Verdict::Refused
                },
            ),
            (
                format!("{time_unit} one past the modelled maximum"),
                modelled_max + 1,
                Verdict::OutsideModelledConstraint,
            ),
            (
                format!("{time_unit} below the modelled minimum"),
                modelled_min - 1,
                Verdict::OutsideModelledConstraint,
            ),
        ];
        for (case, period, expect) in rows {
            harness
                .check(
                    &case,
                    &json!({
                        "BudgetName": "adjust-period",
                        "TimeUnit": time_unit,
                        "BudgetType": "COST",
                        "AutoAdjustData": {
                            "AutoAdjustType": "HISTORICAL",
                            "HistoricalOptions": {"BudgetAdjustmentPeriod": period}
                        }
                    }),
                    expect,
                )
                .await;
        }
    }

    // A granularity the documentation names no maximum for — `CUSTOM` names no period
    // length — is bounded only by the modelled range, so the top of that range is
    // accepted there rather than over-rejected.
    harness
        .check(
            "CUSTOM at the top of the modelled range",
            &json!({
                "BudgetName": "custom-adjust",
                "TimeUnit": "CUSTOM",
                "BudgetType": "COST",
                "AutoAdjustData": {
                    "AutoAdjustType": "HISTORICAL",
                    "HistoricalOptions": {"BudgetAdjustmentPeriod": modelled_max}
                }
            }),
            Verdict::Accepted,
        )
        .await;
}

/// RULE: `PlannedBudgetLimits` "is available for cost or usage budget and supports both
/// monthly and quarterly `TimeUnit`", and its "`key` is the start of the month" or "the
/// start of the quarter, `UTC` in epoch seconds" (`Budget$PlannedBudgetLimits`) — so the
/// map is refused on any other budget type or granularity, and every key must be the
/// midnight-UTC start of a period of the budget's own time unit.
#[tokio::test]
async fn planned_budget_limits_need_their_time_unit_and_period_starts() {
    let harness = Harness::new();
    let month_start = at("2024-02-01T00:00:00Z");
    let quarter_start = at("2024-04-01T00:00:00Z");

    // The granularity rule, over the generated `TIME_UNIT_VALUES` catalog: every entry
    // gets the start of a period of its OWN unit where that exists, and the first of a
    // month otherwise, so a refusal is about the granularity and not the key.
    for time_unit in TIME_UNIT_VALUES {
        let key = match *time_unit {
            "QUARTERLY" => quarter_start,
            _ => month_start,
        };
        let expect = if matches!(*time_unit, "MONTHLY" | "QUARTERLY") {
            Verdict::Accepted
        } else {
            Verdict::Refused
        };
        harness
            .check(
                &format!("PlannedBudgetLimits on a {time_unit} budget"),
                &json!({
                    "BudgetName": "planned-unit",
                    "TimeUnit": time_unit,
                    "BudgetType": "COST",
                    "PlannedBudgetLimits": {key.to_string(): spend("1")}
                }),
                expect,
            )
            .await;
    }

    // The budget-type rule, over the generated `BUDGET_TYPE_VALUES` catalog.
    for budget_type in BUDGET_TYPE_VALUES {
        let expect = if matches!(*budget_type, "COST" | "USAGE") {
            Verdict::Accepted
        } else {
            Verdict::Refused
        };
        harness
            .check(
                &format!("PlannedBudgetLimits on a {budget_type} budget"),
                &json!({
                    "BudgetName": "planned-type",
                    "TimeUnit": "MONTHLY",
                    "BudgetType": budget_type,
                    "PlannedBudgetLimits": {month_start.to_string(): spend("1")}
                }),
                expect,
            )
            .await;
    }

    // The key rule. Rows differ along the form of the key: aligned, misaligned by a day
    // or by hours, not a number at all, and one bad key among good ones — and, for
    // QUARTERLY, a month start that is not the start of a calendar quarter.
    let rows: Vec<(&str, &str, Value, Verdict)> = vec![
        (
            "every quarter start of a year, QUARTERLY",
            "QUARTERLY",
            json!({
                at("2024-01-01T00:00:00Z").to_string(): spend("1"),
                at("2024-04-01T00:00:00Z").to_string(): spend("2"),
                at("2024-07-01T00:00:00Z").to_string(): spend("3"),
                at("2024-10-01T00:00:00Z").to_string(): spend("4")
            }),
            Verdict::Accepted,
        ),
        (
            "twelve month starts, MONTHLY",
            "MONTHLY",
            Value::Object(
                (1..=12)
                    .map(|month| {
                        (
                            at(&format!("2024-{month:02}-01T00:00:00Z")).to_string(),
                            spend("1"),
                        )
                    })
                    .collect(),
            ),
            Verdict::Accepted,
        ),
        (
            "a month start that is not a quarter start, QUARTERLY",
            "QUARTERLY",
            json!({at("2024-02-01T00:00:00Z").to_string(): spend("1")}),
            Verdict::Refused,
        ),
        (
            "mid-month, MONTHLY",
            "MONTHLY",
            json!({at("2024-01-15T00:00:00Z").to_string(): spend("1")}),
            Verdict::Refused,
        ),
        (
            "the first of a month, but not at midnight UTC, MONTHLY",
            "MONTHLY",
            json!({at("2024-01-01T12:00:00Z").to_string(): spend("1")}),
            Verdict::Refused,
        ),
        (
            "one second before a month start, MONTHLY",
            "MONTHLY",
            json!({(at("2024-02-01T00:00:00Z") - 1).to_string(): spend("1")}),
            Verdict::Refused,
        ),
        (
            "a key that is not a number, MONTHLY",
            "MONTHLY",
            json!({"not-an-epoch": spend("1")}),
            Verdict::Refused,
        ),
        (
            // The epoch arithmetic behind the rule has to refuse these rather than
            // overflow on them: a key is a caller-supplied decimal of any size.
            "a key at the top of the signed 64-bit range, MONTHLY",
            "MONTHLY",
            json!({i64::MAX.to_string(): spend("1")}),
            Verdict::Refused,
        ),
        (
            "a key past the signed 64-bit range, MONTHLY",
            "MONTHLY",
            json!({"9223372036854775808": spend("1")}),
            Verdict::Refused,
        ),
        (
            "a key before the epoch, MONTHLY",
            "MONTHLY",
            json!({"-1704067200": spend("1")}),
            Verdict::Refused,
        ),
        (
            "one misaligned key among aligned ones, MONTHLY",
            "MONTHLY",
            json!({
                at("2024-01-01T00:00:00Z").to_string(): spend("1"),
                at("2024-02-01T00:00:00Z").to_string(): spend("2"),
                at("2024-03-02T00:00:00Z").to_string(): spend("3")
            }),
            Verdict::Refused,
        ),
    ];
    for (case, time_unit, planned, expect) in rows {
        harness
            .check(
                case,
                &json!({
                    "BudgetName": "planned-keys",
                    "TimeUnit": time_unit,
                    "BudgetType": "COST",
                    "PlannedBudgetLimits": planned
                }),
                expect,
            )
            .await;
    }
}
