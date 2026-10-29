Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a

# A notification whose Threshold is NaN never matches itself, so CreateNotification stores a second copy instead of refusing the duplicate and no later call can reach either

_Filed: 2026-10-28T18:27:25Z_
_Operation: CreateNotification | Class: error-response_

> _Note: this issue was identified against the code base at commit `1ce4aa1a23419b16e4e6f0ae0f19fcef3b94726a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A notification carries no id, so this crate makes the four members it alerts on its identity, and `crate::state::same_notification` compares the `Threshold` member with `==` on an `f64`. `Notification$Threshold` is a modelled `double` whose only constraint is `@range(min: 0, max: 15000000000000)`, and on awsJson1_1 a caller can put a non-finite double on the wire as the string "NaN" — the framework's own deserializer implements exactly that, and `@range` admits it because every comparison against NaN is false. A `Threshold` of NaN therefore reaches the handler, and from that moment the stored notification is not equal to ITSELF:

* `CreateNotification` asks `find_notification(stored, &input.notification).is_some()` to decide whether the budget already holds the notification. For the NaN notification that is always `None`, so the documented `DuplicateRecordException` ("the notification already exists") is never answered: the same request repeated answers 200 each time and appends another copy, up to the budget's ten-notification capacity, which then answers `CreationLimitExceededException` forever.
* Every operation that names that notification by value resolves it through the same comparison, so each one answers `NotFoundException` for a notification that `DescribeNotificationsForBudget` happily lists: `DeleteNotification`, `UpdateNotification`, `CreateSubscriber`, `DeleteSubscriber`, `UpdateSubscriber` and `DescribeSubscribersForNotification`. The notifications and their subscribers are unreachable and undeletable, and the capacity they occupy is only released by deleting the whole budget.

That is precisely the state this crate's own `attaches_duplicate_notification` doc says must never arise. `CreateBudget` has the same hole on the batch path: a `NotificationsWithSubscribers` list naming one NaN notification twice passes `attaches_duplicate_notification` (which is built on the same `same_notification`) and both entries are stored.

The fix belongs in the one shared identity helper — a `Threshold` that is no number names no notification identity the crate can resolve, so it must not be admitted into the store (or must be compared by total ordering of its bits rather than by `==`). Every path that establishes or resolves a notification already goes through `same_notification`, so there is one place to hold the rule.

## Evidence

The identity comparison, on a raw `f64` — services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/state.rs:1015-1026:

    pub fn same_notification(left: &Notification, right: &Notification) -> bool {
        fn threshold_type(notification: &Notification) -> &str { ... }
        left.notification_type == right.notification_type
            && left.comparison_operator == right.comparison_operator
            && left.threshold == right.threshold
            && threshold_type(left) == threshold_type(right)
    }

The member is a plain `f64`, not an ordered wrapper — src/ops/types/shared_types.rs:140-144:

    pub struct Notification {
        ...
        pub threshold: f64,
        pub threshold_type: Option<String>,

The model allows it: `smithy_query(query="shape",shape="Notification")` gives
`"Threshold": {"target": "NotificationThreshold", "default": 0, "required": true, "range": {"min": 0, "max": 15000000000000}, "type": "double"}` — a `double` with a `@range` and nothing else.

The wire form is reachable and the modelled constraint does not stop it — vendor/smithy-schema-server/src/json.rs:934-938:

    fn read_double(&mut self, _schema: &Schema) -> Result<f64, SerdeError> {
        // awsJson carries the special float values (`NaN`, `Infinity`,
        // `-Infinity`) as strings. `aws-smithy-json` already implements that
        // rule (and rejects non-finite numeric literals), so delegate to it
        // rather than re-parsing the string here.

and vendor/smithy-schema-server/src/constraints.rs:273-300, the ONLY numeric check the boundary walk makes:

    fn check(&self, n: f64, value: &str, path: &str) -> Option<Violation> {
        if let Some(min) = self.min { if n < min { return Some(...) } }
        if let Some(max) = self.max { if n > max { return Some(...) } }
        None
    }

`NaN < 0` and `NaN > 15000000000000` are both false, so no violation is raised and the value reaches `handle`. (`vendor/smithy-schema-server/src/constraints.rs` contains no `is_nan` check at all; the single `is_finite` at :316 is in bound RENDERING, not in validation.)

The gate that is defeated — src/ops/create_notification.rs:254-269:

    // What a notification alerts on identifies it, so a budget cannot hold two
    // of them: a repeat of one it already has is the duplicate record ...
    if crate::state::find_notification(stored, &input.notification).is_some() {
        return Err(CreateNotificationErrorModeled::DuplicateRecordException(
            DuplicateRecordException {
                message: Some(crate::state::duplicate_notification_refusal(&input.budget_name)),

The crate's own statement that the resulting state must never exist — src/state.rs:1066-1077 (`attaches_duplicate_notification` doc):

    /// A notification's identity is what it alerts on ([`same_notification`]) and every
    /// operation that acts on one names it by value, resolving the FIRST match
    /// ([`find_notification`]) — so a budget holding a second notification of one identity
    /// holds it where no later read, update or delete could reach it, and a delete of "the"
    /// notification would leave a copy behind.

and the same helper's implementation, which inherits the hole — src/state.rs:1082-1092: `set.iter().any(|held| same_notification(held, &supplied.notification))`.

The operations that then cannot reach the stored notification, all through the same helper: src/ops/delete_notification.rs:198-200, src/ops/update_notification.rs:221-223, src/ops/create_subscriber.rs:237, src/ops/update_subscriber.rs:247, src/ops/delete_subscriber.rs:220-222, src/ops/describe_subscribers_for_notification.rs:212.

Test coverage: none. A search for `NaN`/`Infinity` across services/service-com-amazonaws-budgets-awsbudgetservicegateway returns no match, so no test in this crate pins a non-finite `Threshold` in either direction. The suites that pin notification identity (tests/all/attached_notification_identity.rs, tests/all/notification_lifecycle.rs, tests/all/notification_update.rs, tests/all/subscriber_set_identity.rs) all use finite thresholds and stay unchanged by a fix that only rejects/orders non-finite ones.

## Steps to Reproduce

1. CreateBudget for the caller's own AccountId with a valid budget, e.g. {"BudgetName":"nan-budget","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}.
2. CreateNotification with {"AccountId":<caller>,"BudgetName":"nan-budget","Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":"NaN","ThresholdType":"ABSOLUTE_VALUE"},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"a@example.com"}]} — the awsJson1_1 spelling of a non-finite double is the string "NaN" (vendor/smithy-schema-server/src/json.rs:934-938). Observe HTTP 200: the `@range(0, 15000000000000)` did not refuse it.
3. Send the byte-identical CreateNotification again. Expected: DuplicateRecordException (the handler's own comment at src/ops/create_notification.rs:254-259 states the rule). Actual: HTTP 200, and DescribeNotificationsForBudget now reports TWO notifications with the same identity. Repeat until the tenth call, after which CreateNotification answers CreationLimitExceededException permanently.
4. DeleteNotification with that same Notification value. Actual: NotFoundException ("the notification doesn't exist") for a notification DescribeNotificationsForBudget just listed. Same for UpdateNotification, CreateSubscriber, DeleteSubscriber, UpdateSubscriber and DescribeSubscribersForNotification naming it — the notifications and subscribers can only be removed by DeleteBudget.
5. Batch path: CreateBudget a fresh budget whose NotificationsWithSubscribers names the same NaN notification twice. `crate::state::attaches_duplicate_notification` (src/state.rs:1078-1093) returns false, so both copies are stored, which is the state its own doc at :1066-1077 says no budget may hold.
