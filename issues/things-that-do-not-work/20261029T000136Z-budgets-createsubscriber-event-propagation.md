Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: c441cdb97fe099c25286b36cfc473da57b1ed5b6

# An SNS subscriber whose topic ARN carries no Region is accepted and stored, but its budget alert is silently dropped by the delivery path

_Filed: 2026-10-29T00:01:36Z_
_Operation: CreateSubscriber | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `c441cdb97fe099c25286b36cfc473da57b1ed5b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The write-path address gate and the alert-delivery path disagree about what makes an address "an SNS topic a budget alert can be published to", so the gate stores a class of SNS subscriber the delivery path can never publish to.

`crate::state::subscriber_address_violation` is the one gate every write that establishes a subscriber runs (CreateSubscriber, CreateNotification, CreateBudget, UpdateSubscriber, CreateBudgetAction, UpdateBudgetAction). Its own documentation states the guarantee it exists for: "An address that could never receive a budget notification is therefore never stored, so no read reports one." For an `SNS` subscriber it delegates to `crate::state::names_sns_topic`, which requires only a well-formed ARN in the `sns` namespace with a non-empty account and a resource field free of `:`/`/` — the shared parse it uses (`service_common::arn::parse_of`) deliberately admits an EMPTY Region field.

The delivery path adds a requirement the gate does not have: `crate::state::alerts::publishable` requires `topic_region(address).is_some()`, i.e. `service_common::arn::region_of`, which answers `None` for "a partition-global ARN whose Region field is empty" (and for a Region field outside `[a-z0-9-]{1,20}`). So a subscriber such as `{"SubscriptionType":"SNS","Address":"arn:aws:sns::123456789012:budget-alerts"}` is accepted with HTTP 200, persisted, and reported by `DescribeSubscribersForNotification` as a live SNS subscriber of the notification — yet when the notification is in ALARM, `alerts::owed` filters that subscriber out before any publish is attempted. Nothing is delivered and nothing is even logged: the `tracing::warn!` in `alerts::deliver` is only reached for a publish that was attempted, and this alert is never built. The same hole exists for an SNS ARN whose Region field is not a Region label (e.g. `US-EAST-1`, or a label longer than 20 characters), which `names_sns_topic` accepts and `region_of` rejects.

So the stored subscriber is exactly what the gate's documentation promises will never be stored: an address that can never receive the notification's alert. Either the gate must hold an SNS address to the same "topic in a Region" rule the delivery path applies (the helper it needs, `region_of`/`topic_region`, is already used two modules away), or the delivery path's silent drop contradicts the acceptance. The two cannot both be right.

## Evidence

The gate's documented guarantee — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1260-1265 (doc of `subscriber_address_violation`): "so every operation that establishes a subscriber checks the set it would leave behind here, before anything is written, and refuses one that fails it with the client error each of them models ... An address that could never receive a budget notification is therefore never stored, so no read reports one."

What the gate actually requires of an SNS address — src/state.rs:1308-1326: "/// Whether `address` names an Amazon SNS TOPIC — what an `SNS` subscriber's address has to be. ... the value has to be a well-formed resource ARN in SNS's own namespace ([`SNS_NAMESPACE`]), which is what makes it a topic a budget alert can be published to at all ([`alerts`])" over
```
fn names_sns_topic(address: &str) -> bool {
    !address.chars().any(char::is_whitespace)
        && ::service_common::arn::parse_of(address, SNS_NAMESPACE).is_some_and(|arn| {
            !arn.account.is_empty() && !arn.resource.contains([':', '/'])
        })
}
```
— no Region requirement, and the shared parse it calls admits an empty one: services/common/src/arn.rs:19-22 "`region` and `account` are deliberately allowed to be EMPTY: AWS writes a partition-global or account-less resource that way (`arn:aws:s3:::bucket`), and those ARNs are well formed."

What the delivery path requires — src/state/alerts.rs:114-130: "/// * it names an SNS topic in a Region, which is what makes it an address this emulator can route a publish to at all (see [`topic_region`]);" over
```
fn publishable(source: &AlertSource, address: &str) -> bool {
    topic_region(address).is_some()
        && ::service_common::arn::parse_of(address, SNS_NAMESPACE)
            .is_some_and(|topic| topic.account == source.account_id)
}
```
and src/state/alerts.rs:218-220 `fn topic_region(address) -> Option<&str> { ::service_common::arn::region_of(address, SNS_NAMESPACE) }`, whose contract is services/common/src/arn.rs:208-213: "`None` therefore means \"this value names no Region of that vendor\", which covers a value that is not an ARN, an ARN of another vendor, a partition-global ARN whose Region field is empty, and a Region field outside the grammar."

The filtered-out alert never reaches the logged-failure path either — src/state/alerts.rs:94-112 (`owed` filters on `publishable` before building any `BudgetAlert`), so src/state/alerts.rs:204-209 (`tracing::warn!("a budget alert could not be published to a subscriber's topic")`) is unreachable for it.

Test coverage: tests/all/subscriber_address_validation.rs:172-175 pins the ACCEPTANCE half — row `("an SNS topic ARN whose Region field is empty", subscriber("SNS", "arn:aws:sns::111122223333:budget-alerts"))` in `well_formed_rows()`, and the module doc at tests/all/subscriber_address_validation.rs:22 lists "a Region-less ARN" among the well-formed forms. No test anywhere asserts what such a subscriber receives when its notification is in ALARM, so the delivery half of the pair is untested and the two halves were never checked against each other. The alert-delivery suite tests/all/notification_alert_delivery.rs only uses Region-carrying topic ARNs.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBudgetServiceGateway.CreateBudget` and body {"AccountId":"<caller account>","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. CreateNotification {"AccountId":"<caller account>","BudgetName":"b1","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"LESS_THAN","Threshold":50},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"ops@example.com"}]} → 200. (LESS_THAN/50 is in ALARM, since every budget here reports ActualSpend "0" — see src/state.rs:1450 ATTRIBUTED_SPEND_AMOUNT.)
3. Create an SNS topic in the emulator and note its ARN, then CreateSubscriber for that notification with {"SubscriptionType":"SNS","Address":"<that ARN>"} → 200, and observe that the alert is published to the topic (the control case).
4. Now CreateSubscriber for the same notification with {"SubscriptionType":"SNS","Address":"arn:aws:sns::<caller account>:budget-alerts"} — the address gate accepts it (pinned by tests/all/subscriber_address_validation.rs:172-175) and it answers 200.
5. DescribeSubscribersForNotification for that notification — the Region-less SNS address is reported as a live subscriber, so the store holds it.
6. Observe that no publish is attempted for it and no warning is logged: `alerts::publishable` returned false, so `alerts::owed` never built a `BudgetAlert` for it (src/state/alerts.rs:94-112, 126-130). The subscriber the gate promised "could never receive a budget notification is therefore never stored" is stored, and receives nothing — on every later write that leaves the notification in alarm as well (UpdateBudget, UpdateNotification, UpdateSubscriber).
