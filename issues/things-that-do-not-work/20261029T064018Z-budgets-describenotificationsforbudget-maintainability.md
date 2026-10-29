Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2f84fd4c7d638614ea26115568d3a3113f96aac5

# Two paginated reads bypass crate::state::page_size, the helper whose own doc calls itself the ONE home for page-size resolution

_Filed: 2026-10-29T06:40:18Z_
_Operation: DescribeNotificationsForBudget | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2f84fd4c7d638614ea26115568d3a3113f96aac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::page_size` was introduced as the single place this crate resolves a paginated read's effective page size (caller-named size, else the member's documented default, else a full `@range(max)` page, clamped to [1, max]). Its doc states that rule as holding for EVERY paginated read of the service. Six of the eight paginated reads call it. Two — `DescribeNotificationsForBudget` and `DescribeSubscribersForNotification` — hand `input.max_results` straight to `crate::state::page` instead, so for those two the "no MaxResults" case is resolved by `page`'s own `None => items.len()` branch rather than by `page_size`, and the ceiling clamp is never applied at all.

The rule therefore lives in two places: in `page_size` for six reads, and implicitly in `page`'s `None` arm for the other two, which is exactly the drift the previously-fixed maintainability issue (20261027T124250Z-budgets-describebudgetactionsforaccount-maintainability) folded four hand-rolled copies into one helper to prevent. Routing these two handlers through `page_size(input.max_results, None, crate::state::MAX_RESULTS_CEILING)` leaves every response byte-identical: a budget holds at most `MAX_NOTIFICATIONS_PER_BUDGET` = 10 notifications and a notification at most 11 subscribers (`Subscribers` is modelled `@length(min: 1, max: 11)`), both below the 100-entry ceiling, so `page_size`'s answer of 100 and `page`'s `items.len()` select the same single page; and a named `MaxResults` outside `@range(1, 100)` is already refused at request validation, making the clamp a no-op.

## Evidence

The stated rule, in the helper's own doc — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1599-1600:
    /// The EFFECTIVE page size one paginated read serves, handed to [`page`] — the ONE home
    /// for that resolution, so every paginated read of this service resolves it the same way.
and the helper itself at src/state.rs:1612-1622 (`pub fn page_size(max_results, documented_default, modelled_max) -> i32`).

The six reads that honour it:
- src/ops/describe_budgets.rs:197-204: `crate::state::page(&budgets, Some(crate::state::page_size(input.max_results, Some(DEFAULT_MAX_RESULTS), MAX_RESULTS_CEILING)), ...)`
- src/ops/describe_budget_notifications_for_account.rs:208-216: same shape with `Some(DEFAULT_MAX_RESULTS)` / `MAX_RESULTS_CEILING`
- src/ops/describe_budget_actions_for_account.rs:206-213: `Some(crate::state::page_size(input.max_results, None, crate::state::MAX_RESULTS_CEILING))`
- src/ops/describe_budget_actions_for_budget.rs:216-223: same
- src/ops/describe_budget_action_histories.rs:251-258: same
- src/ops/describe_budget_performance_history.rs:302-309: same

The two that do not — src/ops/describe_notifications_for_budget.rs:215-219:
        let (page, next_token) = crate::state::page(
            &notifications,
            input.max_results,
            input.next_token.as_deref(),
        )
and src/ops/describe_subscribers_for_notification.rs:221-225:
        let (page, next_token) = crate::state::page(
            &held.subscribers,
            input.max_results,
            input.next_token.as_deref(),
        )

Why the two copies agree today (so the fold changes no response) — src/state.rs:1580-1586 (`page`):
        let per_page = match max_results {
            Some(max) => usize::try_from(max).unwrap_or(1).max(1),
            None => items.len(),
        };
with the bounds that keep `items.len()` under the ceiling: src/state.rs:1189 `pub const MAX_NOTIFICATIONS_PER_BUDGET: usize = 10;` (enforced at src/ops/create_notification.rs:271 and via `notification_set_limit_violation` at src/ops/create_budget.rs:242) and the modelled `Subscribers` `@length(min: 1, max: 11)`.

Test coverage that must stay unchanged: tests/all/default_page_sizes.rs:262 `an_absent_max_results_is_served_the_page_size_the_model_documents`, whose table carries a row per operation including "DescribeNotificationsForBudget, no documented default" (tests/all/default_page_sizes.rs:174-189) and "DescribeSubscribersForNotification, no documented default" (:190-205) with `documented_default: None` and `published_max: 100`; and tests/all/page_size_resolution.rs:442 `every_paginated_read_resolves_the_same_effective_page_size` plus :499 `each_published_page_size_bound_is_served_and_one_past_it_is_refused`, whose own `Row::effective` (tests/all/page_size_resolution.rs:104-109) spells the same resolution `page_size` implements.

## Steps to Reproduce

1. Open services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1599-1622 and read the `page_size` doc: it declares itself "the ONE home for that resolution, so every paginated read of this service resolves it the same way".
2. Grep the crate for `page_size(` under src/ops/: six handlers call it (describe_budgets.rs:199, describe_budget_notifications_for_account.rs:210, describe_budget_actions_for_account.rs:208, describe_budget_actions_for_budget.rs:218, describe_budget_action_histories.rs:253, describe_budget_performance_history.rs:304).
3. Open src/ops/describe_notifications_for_budget.rs:215-219 and src/ops/describe_subscribers_for_notification.rs:221-225: both pass `input.max_results` directly to `crate::state::page`, so neither applies the documented-default/ceiling resolution — the `None` case falls through to `page`'s `items.len()` branch at src/state.rs:1585 instead.
4. Confirm the fold is response-preserving: with `MAX_NOTIFICATIONS_PER_BUDGET = 10` (src/state.rs:1189) and `Subscribers` modelled `@length(1, 11)`, `page_size(None, None, 100) = 100` is at least `items.len()` for both lists, so the served page and the minted `NextToken` are identical either way.
5. Re-run tests/all/default_page_sizes.rs and tests/all/page_size_resolution.rs after routing both handlers through `crate::state::page_size(input.max_results, None, crate::state::MAX_RESULTS_CEILING)`: every assertion and every response body is unchanged.
