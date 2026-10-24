Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# CreateNotification accepts subscriber addresses that are neither an email address nor an SNS topic ARN (and addresses containing line breaks)

_Filed: 2026-10-24T08:44:05Z_
_Operation: CreateNotification | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Subscriber` is documented as "a subscription type and either an Amazon SNS topic or an email address", `Subscriber$Address` as "The address that Amazon Web Services sends budget notifications to, either an SNS topic or an email", and explicitly: "When you create a subscriber, the value of `Address` can't contain line breaks." AWS refuses a mismatched or malformed address with `InvalidParameterException` (HTTP 400, "An error on the client occurred. Typically, the cause is an invalid input value."), which this operation models.

The handler validates only the per-type subscriber COUNTS and never looks at the address at all, so every one of the following is stored and read back by `DescribeSubscribersForNotification` with HTTP 200:
- `{"SubscriptionType":"EMAIL","Address":"not-an-email"}` — no `@` / no domain;
- `{"SubscriptionType":"SNS","Address":"team@example.com"}` — an email supplied where a topic ARN is required (and the reverse, an `arn:aws:sns:...` value under `EMAIL`);
- an `Address` containing `\n` / `\r`, which the model's own prose forbids (the modelled `@pattern` `^(.*[\n\r\t\f\ ]?)*$` does not exclude it, so nothing upstream refuses it either).

Expected: HTTP 400 `InvalidParameterException` and nothing persisted. Actual: HTTP 200 and a notification whose subscriber could never exist in AWS — a later read reports an undeliverable address as if the service had accepted it.

## Evidence

Spec: Smithy model — `Subscriber` doc ("either an Amazon SNS topic or an email address"), `Subscriber$Address` doc ("either an SNS topic or an email" … "the value of `Address` can't contain line breaks"), `SubscriptionType` enum {SNS, EMAIL}; `CreateNotification` declares `InvalidParameterException` (HTTP 400).
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_notification.rs:155-246 — the only subscriber check is `crate::state::subscriber_limit_violation(&input.subscribers)` (counts per type, src/state.rs:664-686); `input.subscribers` is then moved straight into `StoredNotification` (line 240-243) with no inspection of `Address`.
Tests: tests/all/notification_lifecycle.rs builds subscribers with `email_subscriber`/`sns_subscriber` helpers (lines 71-88) that always use well-formed values, and notification_deletion.rs asserts addresses round-trip; no test sends a malformed or cross-type address, so nothing asserts the refusal.
Not a duplicate: the open CreateBudget/validation issue covers the `Budget`-level rules on the CreateBudget path; this is the CreateNotification path and the subscriber address rule.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateNotification and body {"AccountId":"123456789012","BudgetName":"b1","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"not-an-email"}]} — emulator answers 200; AWS answers InvalidParameterException (HTTP 400).
3. Repeat with {"SubscriptionType":"SNS","Address":"team@example.com"} (an email under SNS) and with an Address containing a line break — both answer 200 in the emulator.
4. DescribeSubscribersForNotification for that notification — the malformed addresses are reported back, confirming they were persisted.
