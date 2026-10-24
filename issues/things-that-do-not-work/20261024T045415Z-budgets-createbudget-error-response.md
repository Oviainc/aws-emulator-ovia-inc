Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1266dfca2a9321742ae6c3c2504880e1e0dae7dd

# CreateBudget never returns CreationLimitExceededException when a budget exceeds the documented notification/subscriber limits

_Filed: 2026-10-24T04:54:15Z_
_Operation: CreateBudget | Class: error-response_

> _Note: this issue was identified against the code base at commit `1266dfca2a9321742ae6c3c2504880e1e0dae7dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreationLimitExceededException` ("You've exceeded the notification or subscriber limit", @httpError 405) is a modeled error of CreateBudget, and the operation's own model documentation states the limits it guards: `CreateBudget$NotificationsWithSubscribers` — "A budget can have up to five notifications, and each notification can have one SNS subscriber and up to 10 email subscribers."

The handler never counts anything. `NotificationWithSubscribersList` is modeled `@length(0..10)` and `Subscribers` is `@length(1..11)`, so the boundary validator lets through combinations that are above the service's real per-budget limits, and `create_budget.rs` stores every one of them verbatim:

1. Six to ten entries in `NotificationsWithSubscribers` — expected `CreationLimitExceededException` (the budget may hold at most five notifications); actual HTTP 200 and `DescribeNotificationsForBudget` reports all ten.
2. A single notification whose `Subscribers` holds 11 `EMAIL` subscribers (or 2+ `SNS` subscribers) — expected `CreationLimitExceededException` (one SNS subscriber and up to 10 email subscribers per notification); actual HTTP 200 with all of them persisted.

Net effect: the only limit-related error CreateBudget models is unreachable in the emulator, and budgets exist in emulator state that cannot exist in AWS. This is a different rule set from the already-filed CreateBudget/validation issue (missing BudgetLimit, end-date bound, AutoAdjustData, subscriber address format), which says nothing about notification or subscriber counts.

## Evidence

Spec: Smithy model — `CreateBudget` declares `CreationLimitExceededException` (doc: "You've exceeded the notification or subscriber limit.", @httpError 405), and `CreateBudgetRequest$NotificationsWithSubscribers` documents "A budget can have up to five notifications, and each notification can have one SNS subscriber and up to 10 email subscribers" (the same sentence is quoted in the handler's own header, src/ops/create_budget.rs:63-67).
Handler: src/ops/create_budget.rs:251-271 maps `notifications_with_subscribers` straight into `StoredNotification`s with no count check; the only quota checks in the handler are the per-account/total budget ceilings (lines 216-243, which answer ServiceQuotaExceededException). `grep CreationLimitExceeded` over the crate's src/ returns only the generated error-enum variant (create_budget.rs:111) — it is never constructed.
Tests: tests/all/budget_lifecycle.rs:693-731 (`notifications_paginate_by_max_results`) deliberately creates a budget with TEN notifications and asserts HTTP 200, i.e. the suite currently enshrines the un-limited behavior; no test asserts a CreationLimitExceededException, and no test exercises an 11-subscriber notification.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"123456789012","Budget":{"BudgetName":"too-many-notifications","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"1","Unit":"USD"}},"NotificationsWithSubscribers":[ ...6 distinct notifications, each with one EMAIL subscriber... ]} — emulator answers 200; AWS answers CreationLimitExceededException because a budget may hold at most five notifications.
2. DescribeNotificationsForBudget for that budget — all six notifications are reported, confirming they were persisted.
3. CreateBudget with a single notification whose `Subscribers` list holds 11 {"SubscriptionType":"EMAIL",...} entries (inside the modeled @length(1..11)) — emulator answers 200; AWS answers CreationLimitExceededException (at most 10 email subscribers per notification).
4. CreateBudget with a single notification carrying two {"SubscriptionType":"SNS",...} subscribers — emulator answers 200; AWS answers CreationLimitExceededException (one SNS subscriber per notification).
