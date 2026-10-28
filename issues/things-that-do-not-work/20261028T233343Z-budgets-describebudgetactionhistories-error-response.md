Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 92bcdac01f88c8b4612f5537a2ed608090c91758

# DescribeBudgetActionHistories checks its TimePeriod only after resolving the action, so a malformed window is answered NotFoundException instead of InvalidParameterException

_Filed: 2026-10-28T23:33:43Z_
_Operation: DescribeBudgetActionHistories | Class: error-response_

> _Note: this issue was identified against the code base at commit `92bcdac01f88c8b4612f5537a2ed608090c91758`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other handler in this crate that enforces a rule the request carries *in itself* (a window whose bounds are the wrong way round, an address of the wrong form, a definition that does not match its action type) applies that check BEFORE it resolves the resource, and each of them states why in a comment: "so a request that is malformed in itself is refused the same way whichever budget it names". Its own sibling history read, `DescribeBudgetPerformanceHistory`, applies exactly this check — the `TimePeriod` ordering rule — before it even locks the store.

`DescribeBudgetActionHistories` does the opposite: it resolves the budget and the action first (answering `NotFoundException` when either is absent) and only then refuses a `TimePeriod` whose `Start` does not come before its `End`. So one and the same malformed window produces two different modelled errors depending on something unrelated to it — `InvalidParameterException` when the action happens to exist, `NotFoundException` when it does not — and the crate's own stated rule ("refused the same way whichever budget it names") is broken on the one read that shares the rule with `DescribeBudgetPerformanceHistory`.

The fix is an ordering change only: hoist the `crate::state::time_period_ordered` refusal above the store lookup, exactly as `describe_budget_performance_history.rs` does. No response of a well-formed request changes.

## Evidence

The crate's stated rule, on the handlers that follow it:
- services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget_action.rs:231-233: "// \"This field also determines the format for definition\" — the one rule tying `ActionType` to `Definition`, which no constraint trait can express. Checked before the budget is looked up, so a request that is malformed in itself is refused the same way whichever budget it names."
- src/ops/create_subscriber.rs:194-196: "// ... Checked before the budget is looked up, so a request that is malformed in itself is refused the same way whichever notification it names."

The sibling history read applies the SAME TimePeriod rule before touching the store — src/ops/describe_budget_performance_history.rs:214-234, then :239:
    // The window the caller asked about has to hold together — "The start date must
    // come before the end date" (`TimePeriod`) — and one that does not is an invalid
    // input value, not an empty history.
    if input.time_period.as_ref().is_some_and(|window| !crate::state::time_period_ordered(window)) {
        return Err(...InvalidParameterException...);
    }
    ...
    let store = crate::state::store_at(&state, now);

This handler reverses the order — src/ops/describe_budget_action_histories.rs:206-239:
    let mut store = crate::state::store_at(&state, now);
    ...
    let held = store
        .budgets
        .get_mut(&(input.account_id.clone(), input.budget_name.clone()))
        .and_then(|stored| crate::state::find_action_mut(stored, &input.action_id, now))
        .ok_or_else(|| {
            DescribeBudgetActionHistoriesErrorModeled::NotFoundException(NotFoundException {
                message: Some(format!(
                    "Unable to get budget action history: {} - the budget action doesn't exist.",
                    input.action_id
                )),
            })
        })?;

    // The window the caller asked for, if any. Its bounds have to hold together —
    // "The start date must come before the end date" (`TimePeriod`) — and a period
    // that does not is an invalid input value, not an empty history.
    let records = crate::state::action_history_in_period(held, input.time_period.as_ref())
        .ok_or_else(|| { ...InvalidParameterException... })?;

The shared rule helper both reads call: src/state.rs:1841 `pub fn time_period_ordered(period: &TimePeriod) -> bool` ("Whether `period` satisfies the rule every `TimePeriod` carries: \"The start date must come before the end date\" (`TimePeriod`) ... the rule is the shape's, not one operation's").

Existing coverage of this operation's refusals: tests/all/budget_action_history.rs (page walk + refusals) and tests/all/client_error_envelopes.rs:71 `every_request_refusal_is_answered_at_http_400` — neither pins the ordering of the two refusals against each other.

## Steps to Reproduce

1. Create a budget and a budget action on it (CreateBudget, then CreateBudgetAction), keeping the minted ActionId.
2. Call DescribeBudgetActionHistories with that ActionId and an inverted window, e.g. {"TimePeriod": {"Start": 1700000000, "End": 1600000000}} — the answer is InvalidParameterException ("TimePeriod Start must come before End").
3. Repeat the identical call with a well-formed-but-unminted ActionId (e.g. "00000000-0000-4000-8000-000000000000") — the same malformed window now yields NotFoundException, so the refusal depends on which action was named rather than on the request being malformed.
4. Compare with the sibling: call DescribeBudgetPerformanceHistory with an unknown BudgetName and the same inverted TimePeriod — it answers InvalidParameterException, because describe_budget_performance_history.rs:217-234 refuses the window before the lookup at :239.
