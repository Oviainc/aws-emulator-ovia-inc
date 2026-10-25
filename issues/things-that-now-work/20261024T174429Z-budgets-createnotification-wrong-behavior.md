Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: d62274a9170c1371c4ddc8e9211094959c6d8c1a

# CreateNotification stores duplicate subscribers verbatim, so a notification holds two identical subscribers and they consume the per-type quota twice

_Filed: 2026-10-24T17:44:29Z_
_Operation: CreateNotification | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d62274a9170c1371c4ddc8e9211094959c6d8c1a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A budget notification's subscribers form a SET: the model gives `Subscriber` no identifier at all — it is just `{SubscriptionType, Address}` — and every subscriber operation (`CreateSubscriber`, `DeleteSubscriber`, `UpdateSubscriber`) names the subscriber it acts on by passing that whole shape back. Two identical subscribers in one set are therefore indistinguishable and unaddressable: nothing in the API can name one of them, delete one of them, or update one of them.

The emulator's `CreateNotification` handler stores `input.subscribers` verbatim (`stored.notifications.push(StoredNotification { notification, subscribers: input.subscribers })`), with no dedup and no refusal, so:

* `CreateNotification` with `Subscribers: [{EMAIL, ops@example.com}, {EMAIL, ops@example.com}]` answers 200 and the budget's notification holds TWO identical subscribers, which `DescribeSubscribersForNotification` then reports twice.
* `crate::state::subscriber_limit_violation` counts the repeats toward the documented per-type limits (`MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION = 10`, `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION = 1`), so ten copies of ONE email address exhaust the whole email quota of a notification AWS would consider to hold a single subscriber, and two copies of one SNS topic ARN are refused with `CreationLimitExceededException` even though only one SNS topic was named.

Expected: the subscriber set a notification ends up holding contains each `(SubscriptionType, Address)` pair at most once — a repeat is either collapsed into the one subscriber it names or refused (`DuplicateRecordException` is the error the service models for "this record already exists" on the subscriber write paths), and the per-type limits are counted over distinct subscribers. Actual: duplicates are persisted, reported back on every read, and charged against the limit.

## Evidence

Spec: `Subscriber` in the Smithy model carries only `SubscriptionType` (required) and `Address` (required) and no id, and `CreateSubscriber` / `DeleteSubscriber` / `UpdateSubscriber` take the whole `Subscriber` shape as the way to name one (`UpdateSubscriber` even takes `OldSubscriber` + `NewSubscriber`) — so (type, address) IS the identity and a set with a repeat is a state no later call can address. The `Notification` shape documents the limits as subscriber counts ("A notification can have one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers"), which the emulator's own `src/state.rs` quotes beside `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION` / `MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION`.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_notification.rs — the per-type check at lines 182-188 calls `crate::state::subscriber_limit_violation(&input.subscribers)`, which only counts entries per `subscription_type` (src/state.rs `subscriber_limit_violation`), and lines 240-243 push the caller's list unchanged; nothing compares two subscribers for equality. `src/state.rs` has a `same_notification` helper for notification identity but no equivalent for subscriber identity.
Tests: no test exercises a repeated subscriber. Every subscriber fixture generates distinct addresses (tests/all/notification_lifecycle.rs `email_subscriber(n)` -> `ops{n}@example.com`, tests/all/budget_creation_limits.rs `email_subscribers(count)` -> `limit{i}@example.com`, tests/all/notification_deletion.rs `subscribers(position)`), and tests/all/budget_action_subscriber_limits.rs:43 explicitly notes its SNS fixture is "past it on the subscription TYPE and not on a repeated address" — i.e. the repeated-address case is deliberately untested.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"dup-subs","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — 200.
2. CreateNotification {"AccountId":"123456789012","BudgetName":"dup-subs","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"ops@example.com"},{"SubscriptionType":"EMAIL","Address":"ops@example.com"}]} — the emulator answers 200.
3. DescribeSubscribersForNotification {"AccountId":"123456789012","BudgetName":"dup-subs","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80}} — the emulator reports the same subscriber twice; the real service holds one subscriber for one (type, address) pair, and that single subscriber is the only thing DeleteSubscriber/UpdateSubscriber could ever name.
4. Repeat step 2 on a fresh budget with the SAME SNS topic ARN listed twice ({"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:111122223333:alerts"} x2) — the emulator refuses it with CreationLimitExceededException for exceeding the one-SNS-subscriber limit, although only one SNS topic was named.
5. Repeat step 2 with ten copies of one email address plus one distinct address — the emulator refuses the eleventh entry on the email limit, although the notification names only two distinct email subscribers.
