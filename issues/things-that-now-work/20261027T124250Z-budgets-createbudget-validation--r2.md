Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f1c94accd1d7961aa84a7b9995986ca278396b6

# A BudgetLimit amount other than 100 is accepted for RI / Savings Plans utilization or coverage budgets, although 100 is the only valid value

_Filed: 2026-10-27T12:42:50Z_
_Operation: CreateBudget | Class: validation_

> _Note: this issue was identified against the code base at commit `1f1c94accd1d7961aa84a7b9995986ca278396b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Budget$BudgetLimit` documents two halves of one rule for the four utilization/coverage budget types (`RI_UTILIZATION`, `RI_COVERAGE`, `SAVINGS_PLANS_UTILIZATION`, `SAVINGS_PLANS_COVERAGE`): the limit DEFAULTS to `100`, and `100` is "the only valid value" for them.

The crate implements only the default half. `crate::state::apply_percentage_limit_default` returns early as soon as the request carries any `BudgetLimit`, so a caller-supplied amount is stored verbatim, and `crate::state::validate_budget` — which enforces every other documented `Budget` cross-member rule, including the other two sentences of this very paragraph (`BudgetLimit` required for cost/usage budgets, `BudgetLimit` not usable with `PlannedBudgetLimits`) — never looks at the amount against the budget type. `CreateBudget` therefore answers 200 and persists an RI/Savings-Plans budget whose limit is a value the model calls invalid, and `DescribeBudget`/`DescribeBudgets` echo it afterwards.

The gap is in the shared write-path validator, so `UpdateBudget` (which calls the same `crate::state::validate_budget`) leaves a stored budget in the same shape; the same stored amount also feeds `crate::state::threshold_amount`, so a `PERCENTAGE` notification threshold is then measured as a percentage of a limit the service could not have stored.

This is NOT the gap covered by the already-fixed CreateBudget validation issue (20261024T024810Z-budgets-createbudget-validation): that one enumerated five other rules (missing BudgetLimit for cost/usage, the 06/15/87 end bound, AutoAdjustData pairings, PlannedBudgetLimits TimeUnit/keys, subscriber addresses) and never mentions the "only valid value" sentence. Nor does it reverse the fix that introduced the default: an absent limit still defaults to 100, and a supplied limit of 100 is still stored with the caller's own unit (the unit is explicitly left unconstrained by the model).

## Evidence

Model (`smithy_query(query="doc",shape="Budget",member="BudgetLimit")`): "BudgetLimit is required for cost or usage budgets, but optional for RI or Savings Plans utilization or coverage budgets. RI and Savings Plans utilization or coverage budgets default to 100. **This is the only valid value for RI or Savings Plans utilization or coverage budgets.** You can't use BudgetLimit with PlannedBudgetLimits for CreateBudget and UpdateBudget actions."

The crate quotes the same sentence but implements only the default — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:642-646:
    /// "RI and Savings Plans utilization or coverage budgets default to `100`"
    /// (`Budget$BudgetLimit`) — the `Spend$Amount` such a budget is stored with when the
    /// request carries no `BudgetLimit`.
    pub const PERCENTAGE_LIMIT_AMOUNT: &str = "100";

Handler side — src/state.rs:831-841 (`apply_percentage_limit_default`):
    fn apply_percentage_limit_default(budget: &mut Budget) {
        if budget.budget_limit.is_some()
            || !PERCENTAGE_LIMIT_BUDGET_TYPES.contains(&budget.budget_type.as_str())
        {
            return;
        }
(and src/state.rs:821-822: "A limit the caller DID send is kept as sent")

The validator that owns the paragraph's other sentences never checks the amount — src/state.rs:1784-1814 (`validate_budget`): the body is the BudgetLimit-vs-PlannedBudgetLimits refusal, the FilterExpression/Metrics-vs-CostFilters/CostTypes refusal, `time_period_ordered`, then `validate_time_period_end(budget)?; validate_budget_limit_present(budget)?; validate_planned_budget_limits(budget)?; validate_auto_adjust_data(budget)?;` — no branch reads `PERCENTAGE_LIMIT_BUDGET_TYPES` together with `budget_limit.amount`. Reached from src/ops/create_budget.rs:211 and src/ops/update_budget.rs:197.

No test pins today's acceptance, and the test suite says so explicitly — tests/all/service_owned_budget_members.rs:730-738:
    // A utilization or coverage budget's limit has only one valid AMOUNT ("RI and Savings
    // Plans utilization or coverage budgets default to `100`. This is the only valid
    // value", `Budget$BudgetLimit`), so those rows vary the unit — which that sentence
    // does not constrain and `UnitValue`'s `.*` pattern accepts — rather than pinning
    // acceptance of an amount the documentation calls invalid.
    let percentage_limits = [
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": PERCENTAGE_LIMIT_UNIT}),
        json!({"Amount": PERCENTAGE_LIMIT_AMOUNT, "Unit": "PERCENT"}),
    ];
Every other RI/SP row in the suite sends either no limit or amount "100" (tests/all/budget_rule_conjunction.rs:71-77 `fixed_limit`, tests/all/budget_lifecycle.rs:295, tests/all/notification_alarm_state.rs:213, tests/all/calculated_spend.rs:292-293), so enforcing the rule breaks no existing test.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"<caller account>","Budget":{"BudgetName":"ri-util","TimeUnit":"MONTHLY","BudgetType":"RI_UTILIZATION","BudgetLimit":{"Amount":"50","Unit":"PERCENTAGE"}}}
2. Observe HTTP 200. Expected: the modelled `InvalidParameterException` ("An error on the client occurred. Typically, the cause is an invalid input value.") because 100 "is the only valid value for RI or Savings Plans utilization or coverage budgets", and nothing persisted.
3. POST `DescribeBudget` for "ri-util" — the response reports `BudgetLimit.Amount = "50"`, and `CalculatedSpend.ActualSpend` is reported in that limit's unit, i.e. a budget shape the model says cannot exist.
4. Repeat with `UpdateBudget` and {"AccountId":"<caller account>","NewBudget":{"BudgetName":"ri-util","TimeUnit":"MONTHLY","BudgetType":"SAVINGS_PLANS_COVERAGE","BudgetLimit":{"Amount":"0","Unit":"PERCENTAGE"}}} — also 200, through the same `crate::state::validate_budget`.
5. Attach a notification with `ThresholdType` PERCENTAGE and `Threshold` 80 and read it back: its `NotificationState` is computed by `crate::state::threshold_amount` as 80% of the invalid stored limit.
