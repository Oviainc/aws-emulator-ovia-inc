Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7896f740af7f000033243cc01aa41684b34c3ade

# UpdatePreferences enforces its AfterDiscounts rule only on the mode member, so turning discount visibility off leaves the refused combination stored

_Filed: 2026-10-27T10:03:20Z_
_Operation: UpdatePreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `7896f740af7f000033243cc01aa41684b34c3ade`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdatePreferences` owns a rule the crate states in terms of the state a request LEAVES: `savingsEstimationMode` = `AfterDiscounts` is only available while the organization's member-account discounts are visible. The handler computes `discounts_visible` as the post-request value (the requested visibility if the request sets one, otherwise the stored one) — but it only tests that value against the mode the REQUEST carries, never against the mode already stored.

So the rule is enforced asymmetrically over the two members that decide the resulting state:

* `UpdatePreferences{savingsEstimationMode:"AfterDiscounts"}` while visibility is `None` → 400 `ValidationException` (correct per the rule).
* `UpdatePreferences{memberAccountDiscountVisibility:"None"}` while `AfterDiscounts` is already stored → 200, and the store is left holding exactly the combination the first call is refused for. A following `GetPreferences` reports `{savingsEstimationMode: AfterDiscounts, memberAccountDiscountVisibility: None}` — a record `UpdatePreferences` refuses to be asked for in one call (`preference_gates.rs:385-396` pins that one-shot refusal).

The consequence is not confined to the preference record: both recommendation readers compute every money member under that stored mode (`state.rs:301` `cost_for_mode`, reached through `recommendation_view`), and `ListRecommendationSummaries` reports a `savingsPercentage` FIGURE instead of the "not supported … before discounts" description precisely when the mode is `AfterDiscounts` (`list_recommendation_summaries.rs:443`). After the visibility-only write, the emulator therefore keeps serving discount-net figures (and the percentage metric) for an organization whose discounts the crate says are not visible — the state the gate exists to prevent.

The missing refusal is the one the handler already builds: `after_discounts_needs_visible_discounts()`. The gate needs to consider the EFFECTIVE mode after the merge (requested mode, else the stored one), not only the requested one.

## Evidence

The check, in `services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/update_preferences.rs:200-212` — the visibility is resolved for the post-request state, the mode is not:

```
            // Whether the organization's member-account discounts are visible
            // once this request is applied: the value the request sets if it
            // sets one (which, past the gate above, is the organization's own),
            // otherwise the one already in effect.
            let discounts_visible = match input.member_account_discount_visibility.as_deref() {
                Some(requested) => requested == crate::state::DISCOUNT_VISIBILITY_ALL,
                None => store.member_account_discounts_visible(),
            };
            if input.savings_estimation_mode.as_deref() == Some(crate::state::AFTER_DISCOUNTS)
                && !discounts_visible
            {
                return Err(after_discounts_needs_visible_discounts());
            }
```

The rule the same file states, `update_preferences.rs:255-260`:

```
/// Refuse the discount-adjusted savings estimation mode while the organization
/// has not made its member accounts' discounts visible: the mode's figures are
/// net of those discounts, so the mode is unavailable until the preference that
/// shows them is set
```

The crate's own framing of the rule as being about the resulting state, `tests/all/preference_gates.rs:367-369`:

```
    // And in one request: enabling the visibility and asking for the discounted
    // mode together is accepted, because the state the request leaves satisfies
    // the rule.
```

and the module doc of the same file, `tests/all/preference_gates.rs:15-19`: "`savingsEstimationMode` = `AfterDiscounts` reports figures net of the discounts member accounts' spend carries, so it is available only while the organization has made those discounts visible (rule 1's preference set to `All`); otherwise the same `ValidationException` (400)".

The forbidden state is reachable today, and the existing tests walk straight through it without asserting it: `tests/all/preference_gates.rs:317-331` — row `("back to None", Some(DISCOUNT_VISIBILITY_NONE), false)` issues `ok(&router, "UpdatePreferences", &json!({"memberAccountDiscountVisibility": visibility}))` (an asserted 200) at a point where the preceding row (`"visibility All"`) left `AfterDiscounts` stored, and only afterwards resets the mode with `json!({"savingsEstimationMode": BEFORE_DISCOUNTS})`. `tests/all/preference_writes.rs:195-201` does the same inside `successive_writes_merge_and_can_be_restored`: `json!({"savingsEstimationMode": "AfterDiscounts", ...})` followed by `json!({"memberAccountDiscountVisibility": "None"})`, whose `expected_record` keeps `savingsEstimationMode` = `AfterDiscounts` — i.e. the suite's expectation for that write is the very record `every_combination_of_the_modelled_preference_values_is_stored` expects a 400 for (`preference_writes.rs:130-132`: `let expected_status = if *mode == AFTER_DISCOUNTS && !discounts_visible { 400 } else { 200 };`). Neither test states a reason for allowing the two-step path; both pin it only as a by-product of the merge table.

The downstream reader that keys off the stored mode: `src/ops/list_recommendation_summaries.rs:443` `if mode != Some(crate::state::AFTER_DISCOUNTS) { return Some(SAVINGS_PERCENTAGE_BEFORE_DISCOUNTS); }`, and `src/state.rs:301-303` `pub fn cost_for_mode(&self, mode: Option<&str>) -> f64 { if mode == Some(AFTER_DISCOUNTS) { self.cost_after_discounts() …`.

## Steps to Reproduce

1. Boot the emulator with the Cost Optimization Hub service mounted for an account recorded as the organization's management account (as `management_account_router()` does in `tests/all/cost_optimization_hub.rs`).
2. Call `UpdatePreferences{"memberAccountDiscountVisibility":"All"}` → 200.
3. Call `UpdatePreferences{"savingsEstimationMode":"AfterDiscounts"}` → 200, the discounted mode is now stored.
4. Call `UpdatePreferences{"memberAccountDiscountVisibility":"None"}` → observed: 200. Expected: the same `ValidationException` (400) the handler already builds in `after_discounts_needs_visible_discounts()`, because the state the request leaves has `AfterDiscounts` without visible discounts.
5. Call `GetPreferences` → observed `{"savingsEstimationMode":"AfterDiscounts","memberAccountDiscountVisibility":"None", …}` — the exact combination `UpdatePreferences{"savingsEstimationMode":"AfterDiscounts","memberAccountDiscountVisibility":"None"}` is refused with 400 for (`preference_gates.rs:385-396`).
6. Enroll the account (`UpdateEnrollmentStatus{"status":"Active"}`) and call `ListRecommendationSummaries{"groupBy":"ActionType","metrics":["SavingsPercentage"]}` → the figures are computed net of discounts and `metrics.savingsPercentage` carries a number rather than the "not supported … before discounts" description, for an organization whose discounts are not visible.
