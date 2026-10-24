Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f4204ce556c91dff64e4ed4bfbe851c5f3483f56

# CreateBudgetAction never answers CreationLimitExceededException for an action's per-type subscriber limits

_Filed: 2026-10-24T08:54:40Z_
_Operation: CreateBudgetAction | Class: error-response_

> _Note: this issue was identified against the code base at commit `f4204ce556c91dff64e4ed4bfbe851c5f3483f56`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreationLimitExceededException` ("You've exceeded the notification or subscriber limit") is a modeled error of `CreateBudgetAction`, and the only limit it can guard on this operation is the subscriber set: `CreateBudgetActionRequest$Subscribers` is modeled `@length(min: 1, max: 11)` — the same arithmetic the service documents for a subscriber set elsewhere in this model, "one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers" (the `Notification` shape doc).

Expected (AWS): an action whose `Subscribers` holds two `SNS` entries, or eleven `EMAIL` entries (both inside the modeled 1..11 total), is refused with `CreationLimitExceededException` and no action is created.

Actual (emulator): both are accepted with HTTP 200, a fresh `ActionId` is minted, and `DescribeBudgetAction` reports the whole oversized subscriber set. `CreationLimitExceededException` is unreachable on this operation — the only quota the handler enforces is the emulator's own per-budget action ceiling, which it answers with `ServiceQuotaExceededException`. The sibling write path for notifications (`create_notification.rs`) already enforces exactly this per-type split through `crate::state::subscriber_limit_violation`, so the rule and its error are implemented in the crate and simply not applied here.

## Evidence

Spec: Smithy model — `CreateBudgetAction` declares `CreationLimitExceededException` (doc "You've exceeded the notification or subscriber limit."); `CreateBudgetActionRequest$Subscribers` / `Action$Subscribers` are `@length(1..11)`; the `Notification` shape documents the split that total is made of ("A notification can have one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers"), which src/state.rs:641-686 encodes as `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION = 1` / `MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION = 10` plus `subscriber_limit_violation`.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget_action.rs:189-283 — validates only the `ActionType`→`Definition` pairing (line 214) and the emulator's `MAX_ACTIONS_PER_BUDGET` ceiling (line 240, answered with `ServiceQuotaExceededException`); `input.subscribers` is moved straight into the stored `Action` at line 271 with no count of either subscription type. `subscriber_limit_violation` is never called from this file.
Tests: tests/all/budget_action_execution.rs builds subscriber sets only from `sns_subscriber()` + `email_subscriber(n)` within the limits (lines 126-185, `persistence_rows`) and asserts the round-trip; the per-type refusal is asserted only for CreateNotification (tests/all/notification_lifecycle.rs:762-894), never for CreateBudgetAction.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"acted-on","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudgetAction and a valid action body whose "Subscribers" is [{"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:123456789012:a"},{"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:123456789012:b"}] — emulator answers 200 with an ActionId; AWS refuses with CreationLimitExceededException (one SNS subscriber per set).
3. Repeat with "Subscribers" holding eleven distinct {"SubscriptionType":"EMAIL",...} entries (inside the modeled 1..11 total) — emulator answers 200; AWS refuses with CreationLimitExceededException (at most ten email subscribers).
4. DescribeBudgetAction for either ActionId — the oversized subscriber set is reported back, confirming it was persisted.
