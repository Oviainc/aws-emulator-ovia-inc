Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4af528f360b03ab2b05e9ad5a75be6ebefdc29e9

# A notification reads back in the NotificationState the caller supplied, so a budget can report an ALARM notification although nothing crossed its threshold

_Filed: 2026-10-24T21:45:23Z_
_Operation: DescribeNotificationsForBudget | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4af528f360b03ab2b05e9ad5a75be6ebefdc29e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Notification$NotificationState` is the service's own alarm state: "Specifies whether this notification is in alarm. If a budget notification is in the ALARM state, you passed the set threshold for the budget." It is derived by the service from the budget's actual/forecasted spend against the notification `Threshold` — it is not a value a caller chooses, exactly as `Budget$CalculatedSpend` is not (and that one is already discarded on the write path).

The emulator only fills the member in when the request omitted it: `crate::state::stamp_notification` is `if notification.notification_state.is_none() { … = OK }`. A `CreateNotification` (or a `CreateBudget` carrying `NotificationsWithSubscribers`) that sends `"NotificationState": "ALARM"` therefore has that value persisted and echoed by every read — `DescribeNotificationsForBudget` and `DescribeBudgetNotificationsForAccount` both report the notification as being in alarm even though this emulator tracks no spend at all and nothing ever crossed the threshold.

Expected: the alarm state reported for a notification is the service's own; with no spend behind the budget every notification reads back `OK`, and a caller-supplied `NotificationState` is discarded on the write path. Actual: the caller decides the alarm state and the read repeats it, which is a hollow echo of an input for an output-only member (and it contradicts the service's own notion that a notification in ALARM has passed its threshold).

## Evidence

Spec: `Notification$NotificationState` doc ("Specifies whether this notification is in alarm. If a budget notification is in the ALARM state, you passed the set threshold for the budget") — a state the service reports from the spend it tracks, like `Budget$CalculatedSpend`, whose caller-supplied value the emulator already drops in `drop_service_derived`.
Handler: services/.../src/ops/describe_notifications_for_budget.rs returns `stored.notifications[..].notification.clone()` unchanged; the value comes from the write path, src/state.rs `stamp_notification` (only defaults `notification_state` when absent) used by src/ops/create_notification.rs and src/ops/create_budget.rs. src/ops/describe_budget_notifications_for_account.rs reports the same stored member.
Tests: tests/all/notification_lifecycle.rs (rows around lines 188-193 and the assertion at 276-278) and tests/all/budget_read_projection.rs ("NotificationState — the one the caller set, or OK for a notification that was not …", line 844) assert the current echo, i.e. a test enshrines the divergence rather than the documented semantics; no test asserts that a caller-supplied ALARM is ignored.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"alarm-echo","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} — 200.
2. CreateNotification {"AccountId":"123456789012","BudgetName":"alarm-echo","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80,"ThresholdType":"PERCENTAGE","NotificationState":"ALARM"},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"ops@example.com"}]} — the emulator answers 200.
3. DescribeNotificationsForBudget {"AccountId":"123456789012","BudgetName":"alarm-echo"} — the emulator reports the notification with NotificationState ALARM, although no spend is tracked behind the budget and its 80% threshold was never crossed; the real service reports the alarm state it computed (OK here).
4. DescribeBudgetNotificationsForAccount {"AccountId":"123456789012"} — the same caller-supplied ALARM is reported for the account-wide listing.
