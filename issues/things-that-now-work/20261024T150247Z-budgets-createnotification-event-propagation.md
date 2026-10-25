Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: f5120b87affb905e7d968cf57a1e02e568fab681

# A budget notification in the ALARM state never delivers a budget alert to its SNS topic (or email) subscribers

_Filed: 2026-10-24T15:02:47Z_
_Operation: CreateNotification | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `f5120b87affb905e7d968cf57a1e02e568fab681`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A budget notification exists to deliver an alert: `Subscriber` is "the subscriber to a budget notification … either an Amazon SNS topic or an email address", `Subscriber$Address` is "The address that Amazon Web Services sends budget notifications to", and `SubscriptionType` is "The type of notification that Amazon Web Services sends to a subscriber". In AWS, when a notification goes into the `ALARM` state (`Notification$NotificationState`: "Specifies whether this notification is in alarm. If a budget notification is in the ALARM state, you passed the set threshold for the budget") the service publishes a budget-alert message to the SNS topic named by each `SNS` subscriber (and mails each `EMAIL` subscriber).

The emulator stores subscribers and reads them back, but no code path ever delivers anything: nothing in the crate references SNS or any cross-service publish (grep for `sns` in `src/` only matches the per-type subscriber-count limit constants). This is observable today without any spend engine, because `crate::state::stamp_notification` keeps a caller-supplied `NotificationState` (state.rs:725-729, asserted by tests/all/notification_lifecycle.rs:187-276): a notification created with `"NotificationState":"ALARM"` and an SNS subscriber is stored in alarm, and the SNS topic it names receives no message — `sns.Receive`/`ListSubscriptions`-side state in the emulator is untouched, where AWS would have published a budget alert naming the budget, the threshold and the notification type.

Expected: a notification that is in `ALARM` (created that way, or moved there once budget spend is modeled) causes a budget-alert publish to each `SNS` subscriber's topic. Actual: the subscriber list is inert storage — a subscriber is never contacted, so the whole point of a budget notification is unobservable end-to-end.

## Evidence

Spec: Smithy model — `Subscriber` doc ("The subscriber to a budget notification … either an Amazon SNS topic or an email address"), `Subscriber$Address` doc ("The address that Amazon Web Services sends budget notifications to"), `SubscriptionType` doc ("The type of notification that Amazon Web Services sends to a subscriber"), `Notification$NotificationState` doc ("Specifies whether this notification is in alarm …"). Well-established AWS behavior: AWS Budgets publishes budget alerts to the SNS topic configured on a notification once its threshold is crossed.
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_notification.rs:155-246 — the subscribers are only count-checked (`subscriber_limit_violation`) and then moved into `crate::state::StoredNotification` (line 240-243); no publish, no engine call to another service. services/.../src/state.rs:725-729 (`stamp_notification`) keeps a caller-supplied `NotificationState`, so `ALARM` is reachable from the wire; services/.../src/state.rs holds no SNS/topic plumbing at all (grep `sns` in src/ matches only `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION`/`SUBSCRIPTION_TYPE_SNS`).
Tests: tests/all/notification_lifecycle.rs and notification_deletion.rs assert that subscribers round-trip through DescribeSubscribersForNotification; no test asserts any delivery, so nothing covers this.
Not a duplicate: the open CreateNotification/validation issue covers address well-formedness; the open ExecuteBudgetAction/event-propagation issue covers an action's `Definition` never being applied — neither covers notification alert delivery to subscribers.

## Steps to Reproduce

1. Create an SNS topic in the emulator (e.g. `arn:aws:sns:us-east-1:123456789012:budget-alerts`) and subscribe something observable to it (SQS queue subscription).
2. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"123456789012","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
3. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateNotification` and body {"AccountId":"123456789012","BudgetName":"b1","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80,"NotificationState":"ALARM"},"Subscribers":[{"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:123456789012:budget-alerts"}]} → 200.
4. DescribeNotificationsForBudget for b1 reports the notification with NotificationState ALARM — i.e. the service claims the threshold was passed.
5. Poll the SQS queue subscribed to the topic: no message ever arrives. In AWS a budget alert naming the budget/threshold is published to that topic.
