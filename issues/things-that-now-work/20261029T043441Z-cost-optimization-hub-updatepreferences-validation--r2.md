Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f13a20395d3dd7f956ca916372b3c37cd3e2a76c

# UpdatePreferences refuses memberAccountDiscountVisibility and AfterDiscounts to every possible client

_Filed: 2026-10-29T04:34:41Z_
_Operation: UpdatePreferences | Class: validation_

> _Note: this issue was identified against the code base at commit `f13a20395d3dd7f956ca916372b3c37cd3e2a76c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both organization rules `UpdatePreferences` enforces are gated on `store.is_management_account(caller)`, and NOTHING in `src/` ever records a management account: the store's initial value sets `organization_management_account_id: None` and no handler (or any other code path outside the test harness) ever assigns it. Consequences for every request a real client can send:

1. Any request carrying `memberAccountDiscountVisibility` — EITHER modelled value, including the no-op `None` — is refused with HTTP 400 `ValidationException` ("Only management accounts can update member account discount visibility.") and stores nothing.
2. Because the discount rule reads the visibility out of the (non-existent) management account's record, `discounts_visible` is permanently false, so any request that would leave `savingsEstimationMode` = `AfterDiscounts` is refused with HTTP 400 as well. `AfterDiscounts` is therefore unreachable, `GetPreferences` can never report it, every money figure the recommendation readers compute is locked to the undiscounted mode, and `ListRecommendationSummaries`' `savingsPercentage` metric can never report a figure (it always answers the "not supported when savings estimation mode preference is set to before discounts" description).

The handler's own module doc promises the opposite ("a request whose values are inside their modelled enums and that satisfies both rules is ACCEPTED and merged"), and the crate's test harness has to reach into the store in-process to make the accepting arm reachable at all — it says so explicitly — which is proof that no wire request can reach it. The defect is that the management-account arm of both rules is dead for every client of the emulator, so two modelled input members are permanently un-settable.

## Evidence

Smithy (model text that says these members are settable):
- `UpdatePreferencesRequest$memberAccountDiscountVisibility`: "Sets the \"member account discount visibility\" preference." (enum `All | None`)
- `UpdatePreferencesRequest$savingsEstimationMode`: "Sets the \"savings estimation mode\" preference." (enum `AfterDiscounts | BeforeDiscounts`)

The gates, `services/.../src/ops/update_preferences.rs:198`:
```rust
            if input.member_account_discount_visibility.is_some()
                && !store.is_management_account(&account_id)
            {
                return Err(discount_visibility_is_the_management_accounts());
            }
```
and `:211` / `:225`:
```rust
            let discounts_visible = match input.member_account_discount_visibility.as_deref() {
                Some(requested) => requested == crate::state::DISCOUNT_VISIBILITY_ALL,
                None => store.member_account_discounts_visible(),
            };
            ...
            if resulting_mode == crate::state::AFTER_DISCOUNTS && !discounts_visible {
```

The other side — nothing can ever satisfy those gates. `src/state.rs:949` (doc on `is_management_account`):
"False whenever no management account is recorded, which is the only state this emulator has: an organization-only feature is then available to nobody."
`src/state.rs:964` (`member_account_discount_visibility`): `let management = self.organization_management_account_id.as_deref()?;`
`src/state.rs:1017` (the store's initial value): `organization_management_account_id: None,`
`grep -n organization_management_account_id src/` returns only `state.rs:945` (the field), `:954`, `:965`, `:1017` (the `None` default) and `:1573` (a unit test) — no handler writes it.

The handler doc it contradicts, `src/ops/update_preferences.rs:28`:
"Everything else the model declares is the caller's to set, so a request whose values are inside their modelled enums and that satisfies both rules is ACCEPTED and merged".

The harness admits the arm is unreachable over the wire, `tests/all/cost_optimization_hub.rs:45` and `:70`:
"Which account is the management account is an Organizations fact, and this emulator has no Organizations control plane to learn it from, so the store starts with no organization recorded (`state::organization_management_account_id`) and every caller is a member account. A suite that needs the management-account arm of an organization rule ... records the organization here."
```rust
    state.lock().organization_management_account_id =
        Some(engine::emulator_account_id().to_string());
```

Knock-on, `src/ops/list_recommendation_summaries.rs:443`:
```rust
    if mode != Some(crate::state::AFTER_DISCOUNTS) {
        return Some(SAVINGS_PERCENTAGE_BEFORE_DISCOUNTS);
    }
```

Test coverage: the refusing arm is pinned by `tests/all/preference_gates.rs:74` / `:231` using the plain `router()`; every accepting-arm test (`preference_gates.rs:143`, `:291`, `recommendation_summaries.rs:689`, `summary_metrics_and_refusals.rs:135`) uses `management_account_router()`, i.e. the in-process store mutation above, never a wire request.

## Steps to Reproduce

1. Boot the emulator and point an SDK client for `cost-optimization-hub` at it (awsJson1_0, `X-Amz-Target: CostOptimizationHubService.<Op>`).
2. Call `UpdatePreferences` with `{"memberAccountDiscountVisibility": "None"}` — observe HTTP 400 `ValidationException` ("Only management accounts can update member account discount visibility.") instead of the stored record; the member can never be set by any client.
3. Call `UpdatePreferences` with `{"savingsEstimationMode": "AfterDiscounts"}` — observe HTTP 400 `ValidationException` ("Invalid permission to update preferences. To use AfterDiscounts savings estimation mode, ..."), because `member_account_discounts_visible()` reads a management-account record that never exists.
4. Call `GetPreferences` — `savingsEstimationMode` is always `BeforeDiscounts` and `memberAccountDiscountVisibility` is never reported.
5. Call `ListRecommendationSummaries` with `{"groupBy": "ActionType", "metrics": ["SavingsPercentage"]}` — `metrics.savingsPercentage` is always the "not supported when savings estimation mode preference is set to before discounts" sentence, never a figure.
