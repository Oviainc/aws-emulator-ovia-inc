Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f7ae49092a7217fe38189780203a00baa32a4bc7

# CreateSubscriber accepts subscriber addresses that are neither an email address nor an SNS topic ARN (and addresses containing line breaks)

_Filed: 2026-10-24T20:38:07Z_
_Operation: CreateSubscriber | Class: validation_

> _Note: this issue was identified against the code base at commit `f7ae49092a7217fe38189780203a00baa32a4bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateSubscriber` adds one subscriber to an existing notification, and the `Subscriber` shape it takes is documented as "a subscription type and either an Amazon SNS topic or an email address". `Subscriber$Address` is "The address that Amazon Web Services sends budget notifications to, either an SNS topic or an email", with the explicit rule "When you create a subscriber, the value of `Address` can't contain line breaks." AWS refuses an address that does not match the chosen `SubscriptionType`, an email that is not an email, or one carrying a line break with `InvalidParameterException` (HTTP 400, "An error on the client occurred. Typically, the cause is an invalid input value."), which this operation models.

The handler looks at the subscriber only to (a) compare it against the subscribers already held for duplicate detection and (b) count the per-type totals. It never inspects the address at all, so each of the following is accepted with HTTP 200 and persisted onto the notification:

- `{"SubscriptionType":"EMAIL","Address":"a"}` — no `@`, no domain;
- `{"SubscriptionType":"EMAIL","Address":"ops team@example.com"}` — an embedded space;
- `{"SubscriptionType":"SNS","Address":"team@example.com"}` — an email supplied where a topic ARN is required (and the reverse, `arn:aws:sns:...` under `EMAIL`);
- an `Address` containing `\n`/`\r`, which the model's own prose forbids. The modelled `@pattern` `^(.*[\n\r\t\f\ ]?)*$` does not exclude line breaks, so nothing upstream refuses it either.

Expected: HTTP 400 `InvalidParameterException` and nothing persisted. Actual: HTTP 200, and a later `DescribeSubscribersForNotification` reports an undeliverable address as a live subscriber of the notification — a notification state that could never exist in AWS.

This is the same spec rule already filed against `CreateNotification` (open issue `20261024T084406Z-budgets-createnotification-validation`), but on the other write path that establishes a subscriber: `CreateSubscriber` grows a notification's subscriber set one subscriber at a time and performs its own (different) checks, so fixing the create-notification path does not cover it.

## Evidence

Spec (Smithy model, com-amazonaws-budgets-awsbudgetservicegateway.json): `Subscriber` doc — "The subscriber consists of a subscription type and either an Amazon SNS topic or an email address"; `Subscriber$Address` / `SubscriberAddress` doc — "either an SNS topic or an email" and "When you create a subscriber, the value of `Address` can't contain line breaks"; `SubscriptionType` enum {SNS, EMAIL}; `CreateSubscriber` declares `InvalidParameterException` (HTTP 400).
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_subscriber.rs:154-261 — the only checks are the budget/notification lookup (lines 182-217), `crate::state::same_subscriber` duplicate detection (lines 222-236, which compares type+address verbatim) and `crate::state::subscriber_limit_violation` (lines 243-254, which only COUNTS per `SubscriptionType`, src/state.rs:854-888). `input.subscriber` is then pushed straight onto the stored notification (line 258) with no inspection of the address.
Tests: tests/all/subscriber_creation.rs:262-269 deliberately asserts the OPPOSITE — rows "the shortest address the model allows" (`{"SubscriptionType":"EMAIL","Address":"a"}`) and "an address carrying the space its pattern allows" (`"ops team@example.com"`) are asserted to be persisted and read back. No test anywhere asserts a refusal for a malformed or cross-type address, and no test sends a line-break address.
Not a duplicate: no existing issue targets CreateSubscriber (only its now-fixed scaffold-stub issue). The open CreateNotification/validation issue covers the CreateNotification path; the open CreateBudget/validation and CreateBudgetAction/validation issues cover budget-level rules and the ExecutionRoleArn account rule respectively.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"123456789012","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. CreateNotification {"AccountId":"123456789012","BudgetName":"b1","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"ok@example.com"}]} → 200.
3. CreateSubscriber {"AccountId":"123456789012","BudgetName":"b1","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscriber":{"SubscriptionType":"EMAIL","Address":"a"}} — emulator answers 200; AWS answers InvalidParameterException (HTTP 400).
4. Repeat CreateSubscriber with {"SubscriptionType":"SNS","Address":"team@example.com"} (an email under SNS), with {"SubscriptionType":"EMAIL","Address":"arn:aws:sns:us-east-1:111122223333:t"} (a topic ARN under EMAIL), and with an Address containing a line break — all answer 200 in the emulator.
5. DescribeSubscribersForNotification for that notification — every malformed/cross-type address is reported back, confirming it was persisted rather than refused.
