Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f04518a3ee65f2e7815d98c430f437f66966ee67

# CreateBudget refuses 6-10 NotificationsWithSubscribers with CreationLimitExceededException although the model allows up to 10

_Filed: 2026-10-24T13:03:16Z_
_Operation: CreateBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f04518a3ee65f2e7815d98c430f437f66966ee67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBudgetRequest$NotificationsWithSubscribers` targets `NotificationWithSubscribersList`, whose modeled constraint is `@length(min: 0, max: 10)` — the service's own wire contract accepts up to ten notifications in one create — and `Notification`'s own doc states the resource limit as "A budget can have up to ten notifications" (matching the AWS Budgets quota of 10 notifications per budget / 11 subscribers per notification).

The emulator adds a second, tighter ceiling of five for a single create: `crate::state::MAX_NOTIFICATIONS_PER_CREATE = 5`, enforced by `notification_set_limit_violation` from create_budget.rs, which answers `CreationLimitExceededException` (HTTP 405) for any create carrying 6-10 notifications. That figure comes from a stale prose sentence in the request member doc ("A budget can have up to five notifications"), which the machine-readable `@length(0, 10)` on the very same member contradicts.

The result is an over-strict refusal: a valid request AWS answers 200 for is refused 405, and nothing is persisted. It is also internally inconsistent — the same budget can reach 6-10 notifications by calling CreateNotification repeatedly (the handler allows `MAX_NOTIFICATIONS_PER_BUDGET = 10`), so only the batch form of the identical end state is rejected.

Expected: CreateBudget with 6-10 well-formed NotificationsWithSubscribers entries (each within the 1 SNS + 10 email per-type limits) answers 200, and DescribeNotificationsForBudget reports all of them.
Actual: HTTP 405 CreationLimitExceededException, no budget created at all.

Not covered by the resolved issue `20261024T045415Z-budgets-createbudget-error-response`, which was the opposite direction (limits never enforced at all); this is about the per-create cap that fix introduced being 5 instead of 10.

## Evidence

Spec: `smithy_query(query="shape", shape="NotificationWithSubscribersList")` → list with `@length(min: 0, max: 10)`; Smithy `Notification` doc — "A notification that's associated with a budget. A budget can have up to ten notifications."; AWS Budgets quotas: 10 notifications per budget. CreateBudget's `CreationLimitExceededException` is documented as "You've exceeded the notification or subscriber limit" — i.e. only for a set that exceeds those limits.
Handler: src/ops/create_budget.rs calls `crate::state::notification_set_limit_violation(0, input.notifications_with_subscribers...)`, which refuses `attaching.len() > MAX_NOTIFICATIONS_PER_CREATE` where `MAX_NOTIFICATIONS_PER_CREATE = 5` (src/state.rs); the per-budget ceiling used by CreateNotification in the same file is `MAX_NOTIFICATIONS_PER_BUDGET = 10`.
Tests: tests/all/budget_creation_limits.rs asserts the 5-entry cap is enforced (it pins the emulator's current behavior for a 6-entry create as a refusal), so the divergence is deliberate-but-wrong rather than untested — a trace against real AWS with 6 notifications will diverge.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget, AccountId 123456789012, a valid MONTHLY/COST Budget, and NotificationsWithSubscribers holding 6 distinct notifications (differing ComparisonOperator/Threshold), each with one EMAIL subscriber.
2. Observe HTTP 405 CreationLimitExceededException from the emulator; AWS answers 200 (the modeled list bound is 10).
3. Confirm the inconsistency: CreateBudget with 5 notifications (200), then CreateNotification five more times for the same budget — the emulator allows the budget to hold 10 notifications, the state the 6-entry create was refused for.
4. DescribeNotificationsForBudget after step 1 — no budget exists at all, so the whole create was lost.
