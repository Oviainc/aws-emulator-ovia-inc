Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: d3024881b49e6a38960b221dd9fb71d2129efe77

# UpdateBudget never publishes the budget alerts its write leaves owed, unlike every other write path

_Filed: 2026-10-26T03:08:34Z_
_Operation: UpdateBudget | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `d3024881b49e6a38960b221dd9fb71d2129efe77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other write path in this crate decides the alerts it leaves owed (`crate::state::alerts::owed`) under the store lock and publishes them (`crate::state::alerts::deliver`) once the lock is released: CreateBudget, CreateNotification, CreateSubscriber, UpdateNotification and UpdateSubscriber all do. `UpdateBudget` is the one write path that does not — it replaces the stored budget and returns, with `let _ = ctx;` at the top of the handler so the callback seam the delivery needs is explicitly discarded.

That matters because the budget members `UpdateBudget` replaces are exactly the figures the ALARM decision is read from: `crate::state::notification_state` reads `budget.calculated_spend` (`watched_spend`) and `budget.budget_limit` (`threshold_amount`), and both come out of the budget this operation overwrites. So an `UpdateBudget` call can move a stored notification from `OK` into `ALARM` — every later read (`DescribeNotificationsForBudget`, `DescribeBudgetNotificationsForAccount`) then reports `NotificationState: ALARM` — while no alert is ever published to the SNS subscriber held with that notification. An identical notification/subscriber pairing established by `CreateNotification` or `CreateSubscriber` against the very same budget DOES publish. The alert owed by this write is simply dropped.

## Evidence

The path that skips the helper — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/update_budget.rs:162 and :219-222:
```
        let _ = ctx;
...
        crate::state::stamp_service_owned(&mut budget, now, stored.budget.health_status.clone());
        stored.budget = budget;

        Ok(UpdateBudgetOutput {})
```
(no `alerts::owed`, no `alerts::deliver`; `grep 'alerts::(owed|deliver|source)'` over `src/` lists only update_subscriber.rs, update_notification.rs, create_subscriber.rs, create_notification.rs and create_budget.rs)

The sibling that does it — src/ops/update_notification.rs:286-293:
```
            // A notification this write leaves in alarm has passed its threshold, so it
            // owes the alert it exists to deliver to every subscriber still attached to
            // it — decided from the state this call leaves behind, the way every other
            // write that establishes such a pairing decides it.
            crate::state::alerts::owed(&source, &held.notification, &held.subscribers)
        };

        crate::state::alerts::deliver(ctx, &alerts).await;
```

The rule the crate states for itself — src/state/alerts.rs:24-26:
```
//! The work splits in two so that no delivery is made while the store is locked:
//! [`owed`] decides which alerts a write leaves owed (under the lock, from the state it
//! wrote) and [`deliver`] publishes them (after the lock is released).
```
and the suite that pins it — tests/all/notification_alert_delivery.rs:13-14:
```
//! for the budget"), so every write that leaves such a notification holding an `SNS`
//! subscriber publishes ONE alert, naming the budget and what the notification alerts
```
That table's rows cover `CreateBudget`, `CreateNotification`, `CreateSubscriber` (file docs :26-27) plus `UpdateNotification` (:766) and `UpdateSubscriber` (:913); there is no `UpdateBudget` row and no stated exclusion for it.

That `UpdateBudget` can flip the state is proved by the decision code it feeds — src/state.rs:917-927 (`threshold_amount`, which reads `budget.budget_limit.as_ref()?.amount`) and src/state.rs:895-896 (`watched_spend`, which reads `budget.calculated_spend`), both taken from the budget this handler overwrites at update_budget.rs:220.

## Steps to Reproduce

1. Mount budgets beside SNS on one engine (as tests/all/notification_alert_delivery.rs does) and create an SNS topic in the caller's account.
2. CreateBudget with a COST budget `b1`, `BudgetLimit {Amount: "0", Unit: "USD"}`, and one `NotificationsWithSubscribers` entry whose notification is `{NotificationType: ACTUAL, ComparisonOperator: LESS_THAN, Threshold: 50}` with a single `SNS` subscriber addressing that topic. No alert is published: the limit is 0, so the PERCENTAGE threshold is 0 and `0 < 0` is false (state `OK`).
3. DescribeNotificationsForBudget b1 — the notification reads back `NotificationState: OK`.
4. UpdateBudget with `NewBudget` identical except `BudgetLimit {Amount: "100", Unit: "USD"}`.
5. DescribeNotificationsForBudget b1 — the notification now reads back `NotificationState: ALARM` (threshold 50, actual spend 0, LESS_THAN).
6. Receive on the SNS topic: nothing was published. Create the SAME notification with CreateNotification on a budget whose limit is already "100" and one alert IS published — the two paths disagree on the alert owed for the identical end state.
