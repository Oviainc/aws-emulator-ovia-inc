Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 27d60cb41c89860474fb4eac178b022e94c750b0

# The ShowFilterExpression projection rule is implemented twice (DescribeBudget and DescribeBudgets)

_Filed: 2026-10-24T22:28:37Z_
_Operation: DescribeBudget | Class: maintainability_

> _Note: this issue was identified against the code base at commit `27d60cb41c89860474fb4eac178b022e94c750b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "a budget's `FilterExpression` is reported only when the caller asked for it" is written out independently in the two read handlers, over the same projected `Budget` shape. Either copy can be changed (for example to strip or keep another member the flag governs) without the other, so the two reads can drift apart while both still answer 200. The crate already centralises every other read-side projection of a stored budget in `crate::state::project_budget` (src/state.rs:966-974), which both handlers call immediately before applying their own copy of this branch; folding the flag into that one function (or a neighbouring helper in `state.rs`) leaves every response byte and every existing test unchanged.

## Evidence

Copy 1 — src/ops/describe_budget.rs:184-189:
```
let mut budget = crate::state::project_budget(stored, ::std::time::SystemTime::now());
// "Specifies whether the response includes the filter expression associated with
// the budget" — so it is left out until the caller asks for it.
if !input.show_filter_expression.unwrap_or(false) {
    budget.filter_expression = None;
}
```
Copy 2 — src/ops/describe_budgets.rs:186-195:
```
let show_filter_expression = input.show_filter_expression.unwrap_or(false);
let page = page
    .into_iter()
    .map(|mut budget| {
        if !show_filter_expression {
            budget.filter_expression = None;
        }
        budget
    })
```
Both sit directly on the output of the shared projection helper, src/state.rs:966 `pub fn project_budget(stored: &StoredBudget, at: SystemTime) -> Budget` ("The `Budget` a read reports for `stored`, as of `at`", src/state.rs:958).

Existing coverage that must stay green: tests/all/budget_lifecycle.rs:1599-1680 `the_filter_expression_is_returned_only_when_asked_for` — "/// RULE: a budget's `FilterExpression` is returned only when the caller asks for it\n/// with `ShowFilterExpression`" — drives both operations (`for operation in ["DescribeBudget", "DescribeBudgets"]`, line 1640) over show = None/false/true.

## Steps to Reproduce

1. Read src/ops/describe_budget.rs:184-189 and src/ops/describe_budgets.rs:186-195: the same `filter_expression = None` rule appears in both, each applied to the output of `crate::state::project_budget`.
2. Note that src/state.rs:966 already owns every other read-side projection of a stored budget, so the rule has an existing single home.
3. Confirm tests/all/budget_lifecycle.rs:1599-1680 exercises the rule through both operations, so moving it into one helper is verifiable without changing any response or test.
