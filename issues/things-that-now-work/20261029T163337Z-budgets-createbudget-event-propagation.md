Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: f498d25066c291e3faedd02ae2bda62a819c5304

# An SNS subscriber whose topic ARN names another account is accepted and stored, but its budget alert is silently dropped — against sns_topic's own documented one-gate guarantee

_Filed: 2026-10-29T16:33:37Z_
_Operation: CreateBudget | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `f498d25066c291e3faedd02ae2bda62a819c5304`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::sns_topic` is documented as the ONE reading of an `SNS` subscriber's address, shared by the write that establishes the subscriber (`subscriber_address_violation`) and the alert published to it (`state::alerts`), and its doc states the guarantee that follows: "an address a write accepts is an address an alert can be published to, and the gate's guarantee holds instead of the delivery path quietly skipping a stored subscriber". `subscriber_address_violation` tests exactly one thing for an `SNS` subscriber — that the address parses as a topic ARN. But the delivery decision `state::alerts::publishable` tests TWO things: that it parses as a topic ARN AND that the topic's account field equals the account that owns the budget.

So the second condition is applied only on the delivery side. An `SNS` subscriber whose topic ARN names any other 12-digit account is accepted by every write path that establishes a subscriber (`CreateBudget`, `CreateNotification`, `CreateSubscriber`, `UpdateSubscriber`), is persisted, and is reported back by `DescribeSubscribersForNotification` — and then, when the service reads that notification as being in `ALARM`, `alerts::owed` filters the subscriber out before `alerts::deliver` ever runs, so no publish is attempted and not even the warning `deliver` logs for a refused publish is emitted. The stored subscriber is exactly the "stored subscriber the delivery path quietly skips" that `sns_topic`'s doc says cannot exist, and `subscriber_address_violation`'s own doc ("An address that could never receive a budget notification is therefore never stored, so no read reports one") is false for this address.

This is the same shape as the already-fixed region-less-topic gap, which was closed by tightening the WRITE-path gate so a stored SNS subscriber can always be published to. The crate already has the write-time cross-account pattern for the other caller-supplied ARN it holds: `validate_execution_role_account` refuses a role ARN whose account field is not the request's `AccountId` before the action is stored.

Note on scope: `tests/all/notification_alert_delivery.rs` pins that `alerts::owed` owes nothing on a foreign-account topic, and that unit-level assertion is not what is being questioned here — the defect is that the write path accepts and stores such an address at all, which is what `sns_topic`'s doc promises it does not.

## Evidence

The guarantee, in the crate's own doc for the shared gate — `services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1380-1384`:
```
/// The ONE reading of an `SNS` subscriber's address, for BOTH halves of what such a
/// subscriber is for: the write that establishes one ([`subscriber_address_violation`])
/// and the alert published to one ([`alerts`]) each read an address through here, so an
/// address a write accepts is an address an alert can be published to, and the gate's
/// guarantee holds instead of the delivery path quietly skipping a stored subscriber.
```
and `state.rs:1333-1334`:
```
/// An address that could never receive a budget notification is therefore never stored,
/// so no read reports one.
```

What the WRITE path actually checks for an SNS subscriber — `state.rs:1360-1362` (inside `subscriber_address_violation`, `state.rs:1348`):
```
            SUBSCRIPTION_TYPE_SNS => {
                sns_topic(&subscriber.address).is_none().then_some(SNS_ADDRESS_REFUSAL)
            }
```
No account comparison: `sns_topic` itself (`state.rs:1396-1408`) only requires a Region label, a non-empty account field and a resource field without `:`/`/` — `!topic.account.is_empty()`, never `topic.account == <the request's AccountId>`.

What the DELIVERY path checks — `services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state/alerts.rs:129-131`:
```
fn publishable(source: &AlertSource, address: &str) -> bool {
    super::sns_topic(address).is_some_and(|topic| topic.account == source.account_id)
}
```
reached from `alerts.rs:105` inside `owed`:
```
        .filter(|subscriber| publishable(source, &subscriber.address))
```
and `alerts.rs:120-125` admits the second condition is applied only here, while still claiming the first makes the gate total:
```
/// * it names an SNS topic, which — SNS being a regional service — is a topic in a Region
///   and so an address this emulator can route a publish to at all. It is the SAME
///   reading the write that established the subscriber was held to
///   ([`super::subscriber_address_violation`]), so a stored `SNS` subscriber cannot fail
///   it: an address no alert could be published to is refused before it is stored.
/// * that topic belongs to the SAME account as the budget alerting on it. ... it is
///   dropped here rather than published.
```

The handler that exercises both halves in one call — `src/ops/create_budget.rs:222-233` calls `crate::state::subscriber_address_violation(...)` over every attached subscriber, and `src/ops/create_budget.rs:382-387` then decides the alerts with `crate::state::alerts::owed(&source, &held.notification, &held.subscribers)` before `src/ops/create_budget.rs:408` publishes them.

The crate's existing write-time cross-account refusal for the other caller-supplied ARN, showing the pattern this path skips — `state.rs:441-458` `validate_execution_role_account`, documented at `state.rs:432-434`: "Fails CLOSED: a string whose account field is not an account id at all names no account to be in the same one as, so it is refused here too rather than passed through."

Test coverage today: `tests/all/subscriber_sns_topic_region.rs:113-118` deliberately holds the topic's account to the caller's ("The account is the caller's so that the only thing a row varies is the Region"), so no suite exercises a write with a foreign-account topic; `tests/all/notification_alert_delivery.rs:1084-1129` asserts only that `alerts::owed` owes 0 for "a topic of another account", never that such an address is refused when stored.

## Steps to Reproduce

1. Call `CreateBudget` with `AccountId` = the account this emulator answers for and `Budget` = {BudgetName: "cross-account", BudgetType: "COST", TimeUnit: "MONTHLY", BudgetLimit: {Amount: "100", Unit: "USD"}}, plus `NotificationsWithSubscribers`: [{Notification: {NotificationType: "ACTUAL", ComparisonOperator: "LESS_THAN", Threshold: 80}, Subscribers: [{SubscriptionType: "SNS", Address: "arn:aws:sns:us-east-1:123456789012:budget-alerts"}]}] — a well-formed topic ARN in an account that is NOT the caller's.
2. Observe HTTP 200: the subscriber was accepted even though no alert for this budget can ever be published to it (`state.rs:1360-1362` never compares the topic's account to `AccountId`).
3. Call `DescribeSubscribersForNotification` for that notification: the foreign-account SNS subscriber is reported back, contradicting `state.rs:1333-1334` ("An address that could never receive a budget notification is therefore never stored, so no read reports one").
4. Call `DescribeNotificationsForBudget`: the notification reads back with `NotificationState: "ALARM"` (attributed actual spend 0 is LESS_THAN 80% of the 100 USD limit), i.e. the service itself says an alert is owed.
5. Observe that no `sns:Publish` is attempted and nothing is logged: `alerts::owed` dropped the subscriber at `src/state/alerts.rs:105` via `publishable` (`src/state/alerts.rs:130`) before `alerts::deliver` ran, so even the warning `deliver` logs for a refused publish never appears.
6. Repeat step 1 with the same ARN carrying the caller's own account id: the alert is published. The only difference is the account field the write path never inspected.
