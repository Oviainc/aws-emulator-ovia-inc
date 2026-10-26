Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 232a76c0adfd6241e7fe920f3ef518cb863b87b4

# CreateBudget stores two notifications with the same identity instead of refusing the repeat with DuplicateRecordException

_Filed: 2026-10-26T00:34:32Z_
_Operation: CreateBudget | Class: error-response_

> _Note: this issue was identified against the code base at commit `232a76c0adfd6241e7fe920f3ef518cb863b87b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateBudget` pushes every entry of `NotificationsWithSubscribers` into the stored budget verbatim. It collapses repeated SUBSCRIBERS inside one notification (`crate::state::distinct_subscribers`), but it never applies the notification-identity rule this crate states and enforces everywhere else, so a list that names the SAME notification twice (same `NotificationType` / `ComparisonOperator` / `Threshold` / `ThresholdType`) is accepted with HTTP 200 and the budget ends up holding two StoredNotification entries with one identity.

The sibling write path refuses exactly this input: `CreateNotification` answers the modelled `DuplicateRecordException` when `find_notification` already matches, and `UpdateNotification` answers it for a `NewNotification` that collides with another notification of the budget. `CreateBudget` models `DuplicateRecordException` too and never answers it for this case.

The stored state that results is not addressable:
* `crate::state::find_notification` / `find_notification_mut` return the FIRST match, so `CreateSubscriber`, `UpdateSubscriber` and `DescribeSubscribersForNotification` can only ever reach the first copy — the subscribers attached to the second copy are stored where no operation can read, update or delete them.
* `DeleteNotification` removes one position only, so after a successful delete of "the" notification a `DescribeNotificationsForBudget` still lists it.
* `notification_set_limit_violation` checks the per-type subscriber halves per supplied entry, so two copies each carrying 10 EMAIL subscribers put one notification identity over the "up to 10 email subscribers" half the same helper exists to enforce, while each copy also burns a slot of `MAX_NOTIFICATIONS_PER_BUDGET`.

Expected: the repeat is refused with the modelled `DuplicateRecordException` (as `CreateNotification` refuses it) and nothing is persisted. Actual: HTTP 200 and a budget holding one notification twice.

## Evidence

Sibling handler enforces the rule — services/.../src/ops/create_notification.rs:254-267:
```
// What a notification alerts on identifies it, so a budget cannot hold two
// of them: a repeat of one it already has is the duplicate record — not a
// second notification, and not an overwrite of the first one's subscribers.
if crate::state::find_notification(stored, &input.notification).is_some() {
    return Err(CreateNotificationErrorModeled::DuplicateRecordException(
        DuplicateRecordException {
            message: Some(format!(
                "Unable to create notification: {} - the notification already exists.",
```

The rule lives in this crate's state module — src/state.rs:832-845 (`same_notification` doc):
"Whether `left` and `right` name the SAME notification of a budget. / A notification carries no id of its own, so every operation that acts on one (`DeleteNotification`, `UpdateNotification`, `DeleteSubscriber`, …) names it by passing the whole `Notification` shape back. Its identity is therefore the members that describe WHICH notification it is …"
and src/state.rs:860-868 resolves only the first match:
```
pub fn find_notification<'a>(... ) -> Option<&'a StoredNotification> {
    stored.notifications.iter().find(|held| same_notification(&held.notification, wanted))
}
```

CreateBudget skips it — src/ops/create_budget.rs:239-243 only counts capacities:
```
crate::state::notification_set_limit_violation(
    0,
    input.notifications_with_subscribers.as_deref().unwrap_or(&[]),
)
```
and src/ops/create_budget.rs:328-344 stores the list as supplied, de-duplicating only the subscribers:
```
let notifications: Vec<crate::state::StoredNotification> = input
    .notifications_with_subscribers
    .unwrap_or_default()
    .into_iter()
    .map(|supplied| {
        let mut notification = supplied.notification;
        crate::state::stamp_notification(&mut notification);
        crate::state::StoredNotification {
            notification,
            subscribers: crate::state::distinct_subscribers(supplied.subscribers),
```

No test pins the duplicate case; the capacity suite deliberately avoids it — tests/all/budget_creation_limits.rs:52-53:
```
/// A notification identified by its threshold, so `index` notifications built this way
/// are `index` DIFFERENT notifications rather than repeats of one.
```
and tests/all/notification_lifecycle.rs:15 states the rule for the per-call path: "`ComparisonOperator`, `Threshold`, `ThresholdType` — so a repeat is a duplicate". tests/all/subscriber_set_identity.rs:448-451 asserts CreateBudget collapses repeated SUBSCRIBERS ("a notification attached with the budget establishes its subscriber set the same way `CreateNotification` does") — the notification half of the same set rule is missing.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget and body {"AccountId":"<caller account>","Budget":{"BudgetName":"dupe","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}},"NotificationsWithSubscribers":[{"Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"a@example.com"}]},{"Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"b@example.com"}]}]} — answers 200 (expected: DuplicateRecordException, the refusal CreateNotification gives for the same repeat).
2. Call DescribeNotificationsForBudget for budget "dupe" — two identical notifications are listed.
3. Call DescribeSubscribersForNotification for that notification — only a@example.com is reported; b@example.com is stored in the second copy and is unreachable.
4. Call DeleteNotification for that notification (200), then DescribeNotificationsForBudget again — the notification is still listed, because only the first copy was removed.
