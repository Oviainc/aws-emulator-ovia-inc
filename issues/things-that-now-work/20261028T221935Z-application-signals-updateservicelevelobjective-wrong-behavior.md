Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 68144f2b1a9e2dba602611483c97527f7042e8b8

# An update that supplies Goal resets the Goal members it omits to the create-time defaults instead of retaining their previous values

_Filed: 2026-10-28T22:19:35Z_
_Operation: UpdateServiceLevelObjective | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `68144f2b1a9e2dba602611483c97527f7042e8b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateServiceLevelObjective` is documented to retain the previous value of every parameter a request omits. The handler honours that for the top-level members (Description, SliConfig, BurnRateConfigurations, AutoInvestigationEnabled), but for `Goal` it rebuilds the whole structure through `crate::state::goal_with_defaults`, which fills every member the request left out with the CREATE-time default rather than with the value the stored record carries.

So an SLO created with `Goal { Interval: CalendarInterval(MONTH), AttainmentGoal: 99.9, WarningThreshold: 20.0 }` and then updated with `Goal { AttainmentGoal: 95.0 }` comes back carrying `WarningThreshold: 50.0` and a rolling 7-day interval: two settings the update never mentioned are silently overwritten, and the customer's stored values are gone. Every stored record is affected, because the create path already fills all three members (`goal_with_defaults` is the create's writer too), so there is always a concrete previous value to lose rather than an absent one.

The loss is observable beyond the record itself: `BatchGetServiceLevelObjectiveBudgetReport` computes `BudgetStatus` from `goal.warning_threshold` (`budget_status_of(total, total, goal.warning_threshold)`), so after such an update the budget report warns at a threshold the caller never asked for and the evaluation interval the budget is measured over changes from a calendar month to a rolling week.

## Evidence

MODEL — the operation's own documentation sentence (`smithy_query(query="doc", shape="UpdateServiceLevelObjective")`):
"Updates an existing service level objective (SLO). If you omit parameters, the previous values of those parameters are retained."

MODEL — the Goal members the code overwrites call themselves parameters (`smithy_query(query="shape", shape="Goal")`):
* `WarningThreshold`: "The percentage of remaining budget over total budget that you want to get warnings for. If you omit this parameter, the default of 50.0 is used."
* `Interval`: "The time period used to evaluate the SLO. It can be either a calendar interval or rolling interval. If you omit this parameter, a rolling interval of 7 days is used."

HANDLER — services/service-com-amazonaws-applicationsignals-applicationsignals/src/ops/update_service_level_objective.rs:244-246:
```
        if let Some(goal) = input.goal.as_ref() {
            slo.goal = crate::state::goal_with_defaults(Some(goal));
        }
```

WHAT THAT WRITES — src/state.rs:855-873 (`goal_with_defaults`), which substitutes the defaults for whatever the request omitted, with no reference to the stored goal:
```
pub fn goal_with_defaults(requested: Option<&Goal>) -> Goal {
    Goal {
        interval: Some(
            requested
                .and_then(|goal| goal.interval.clone())
                .unwrap_or_else(default_interval),
        ),
        attainment_goal: Some(
            requested
                .and_then(|goal| goal.attainment_goal)
                .unwrap_or(DEFAULT_ATTAINMENT_GOAL),
        ),
        warning_threshold: Some(
            requested
                .and_then(|goal| goal.warning_threshold)
                .unwrap_or(DEFAULT_WARNING_THRESHOLD),
        ),
    }
}
```
with src/state.rs:144 `const DEFAULT_WARNING_THRESHOLD: f64 = 50.0;` and src/state.rs:846-851 `default_interval()` -> rolling 7 `DAY`.

THE HANDLER'S OWN DOC ADMITS THE OVERWRITE — src/ops/update_service_level_objective.rs:161-165:
```
    /// A supplied `Goal` REPLACES the stored goal wholesale: each of its own members the
    /// request omits takes the default the model documents for it, not the value the stored
    /// goal happened to carry. The same holds for the SLI configs and the burn-rate list —
```

DOWNSTREAM READER OF THE CLOBBERED MEMBER — src/ops/batch_get_service_level_objective_budget_report.rs:452:
```
            budget_status_of(total, total, goal.warning_threshold),
```

TEST COVERAGE — tests/all/slo_lifecycle.rs:515-519 is the only update-Goal row, and it asserts nothing about the goal's own members:
```
        Case {
            label: "Goal",
            patch: json!({ "Goal": { "AttainmentGoal": 95.0 } }),
            untouched: &["Description", "Sli", "BurnRateConfigurations", "AutoInvestigationEnabled"],
        },
```
so the reset of `WarningThreshold`/`Interval` is pinned by no assertion.

## Steps to Reproduce

1. POST /slo with {"Name":"goal-slo","SliConfig":{...any valid period-based config...},"Goal":{"Interval":{"CalendarInterval":{"StartTime":<epoch>,"DurationUnit":"MONTH","Duration":1}},"AttainmentGoal":99.9,"WarningThreshold":20.0}} — the response's Slo.Goal carries all three members as sent.
2. PATCH /slo/goal-slo with {"Goal":{"AttainmentGoal":95.0}} — the request names neither WarningThreshold nor Interval.
3. GET /slo/goal-slo: Slo.Goal.WarningThreshold is 50.0 (not the stored 20.0) and Slo.Goal.Interval is a rolling 7-day interval (not the stored calendar month), although the operation documents that "if you omit parameters, the previous values of those parameters are retained".
4. POST /budget-report with {"Timestamp":<now>,"SloIds":["goal-slo"]}: the report's BudgetStatus is now computed against the 50.0 threshold and the rolling-week interval the caller never set (src/ops/batch_get_service_level_objective_budget_report.rs:452).
