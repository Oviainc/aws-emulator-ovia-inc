Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the budgets emulator

_Filed: 2026-10-21T21:18:44Z_
_Operation: service-com-amazonaws-budgets-awsbudgetservicegateway | Class: stub_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 26 operations in `services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no AWS Budgets client can do anything against this emulator. Implement a FIRST, SMALL stateful slice: the lifecycle of the primary resource, the Budget (awsJson1_1, `POST /` with `X-Amz-Target: AWSBudgetServiceGateway.<Operation>`).

Happy path to implement, in order (6 operations):

1. **CreateBudget** — input `AccountId` (required, 12 digits), `Budget` (required: `BudgetName`, `TimeUnit`, `BudgetType` required; optional `BudgetLimit` (Spend{Amount,Unit}), `TimePeriod`, `CostFilters`, `CostTypes`, `PlannedBudgetLimits`, `CalculatedSpend`, `FilterExpression`, `Metrics`), `NotificationsWithSubscribers` (0..10), `ResourceTags` (0..200). Output has no members. Must PERSIST the budget under (AccountId, BudgetName), set `LastUpdatedTime` to now, and persist each supplied notification with its subscribers. A second CreateBudget for the same (AccountId, BudgetName) must fail `DuplicateRecordException` (HTTP 409); a malformed `AccountId`/`BudgetName` or a missing required Budget member must fail `InvalidParameterException` (HTTP 400).
2. **DescribeBudget** — input `AccountId`, `BudgetName`, optional `ShowFilterExpression`. Output `Budget` must be read back from the store and equal what was created/updated (including `BudgetLimit` amount+unit, `TimeUnit`, `BudgetType`, `TimePeriod`, `LastUpdatedTime`). Unknown budget → `NotFoundException` (HTTP 404). `FilterExpression` is only echoed when `ShowFilterExpression` is true.
3. **DescribeBudgets** — input `AccountId`, `MaxResults` (1..1000), `NextToken`, `ShowFilterExpression`. Output `Budgets` must be exactly the budgets stored for that account (not a canned list) with `NextToken` honoring `MaxResults`; an opaque token that does not decode → `InvalidNextTokenException` (HTTP 400).
4. **UpdateBudget** — input `AccountId`, `NewBudget` (required). Must replace the stored budget matching `NewBudget.BudgetName` in place, refresh `LastUpdatedTime`, and be visible to the next DescribeBudget/DescribeBudgets. Unknown budget name → `NotFoundException` (404).
5. **DescribeNotificationsForBudget** — input `AccountId`, `BudgetName`, `MaxResults` (1..100), `NextToken`. Output `Notifications` must be the notifications persisted for that budget (from CreateBudget's `NotificationsWithSubscribers`), each with `NotificationType`, `ComparisonOperator`, `Threshold`, `ThresholdType`. Unknown budget → `NotFoundException` (404).
6. **DeleteBudget** — input `AccountId`, `BudgetName`. Output has no members. Must remove the budget AND its notifications/subscribers/tags from the store; a subsequent DescribeBudget must return `NotFoundException` (404), and a second DeleteBudget must also return 404.

No canned, hardcoded, or echoed responses: each of the 6 must compute its response from the persisted store reached via `crate::state::service_state(&*self.engine)` (per the crate README), and the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker must be deleted from each implemented handler.

## State Model

In `src/state.rs` (`ComAmazonawsBudgetsAwsbudgetservicegatewayState`):

- `budgets: HashMap<(AccountId /*String*/, BudgetName /*String*/), StoredBudget>`
- `StoredBudget { budget: Budget /* BudgetName, BudgetLimit: Option<Spend{amount: String, unit: String}>, PlannedBudgetLimits, CostFilters, CostTypes, TimeUnit, TimePeriod{Start,End}, CalculatedSpend, BudgetType, LastUpdatedTime, AutoAdjustData, FilterExpression, Metrics, BillingViewArn */, arn: String /* arn:aws:budgets::<AccountId>:budget/<BudgetName> */, notifications: Vec<StoredNotification>, tags: Vec<ResourceTag{Key,Value}>, last_updated: SystemTime }`
- `StoredNotification { notification: Notification{NotificationType, ComparisonOperator, Threshold, ThresholdType, NotificationState}, subscribers: Vec<Subscriber{SubscriptionType, Address}> }`
- Pagination: `NextToken` is an opaque encoding of an offset into the deterministically ordered (sorted by BudgetName) budget/notification list for the account; rejected with `InvalidNextTokenException` when it does not decode.

Everything else (budget actions: CreateBudgetAction/DescribeBudgetAction*/ExecuteBudgetAction/UpdateBudgetAction/DeleteBudgetAction, Create/Update/Delete Notification and Subscriber, DescribeSubscribersForNotification, DescribeBudgetPerformanceHistory, DescribeBudgetNotificationsForAccount, Tag/Untag/ListTagsForResource) stays at 501 and is filed separately as individual gap issues.

Done = a client script running CreateBudget → DescribeBudget → DescribeBudgets → UpdateBudget → DescribeBudget → DescribeNotificationsForBudget → DeleteBudget → DescribeBudget(404) passes with correct, state-derived bodies, not just 2xx.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/` matches all 26 operation files (create_budget.rs, describe_budget.rs, describe_budgets.rs, update_budget.rs, delete_budget.rs, describe_notifications_for_budget.rs, …), each returning `Err(crate::scaffolded::Scaffolded::not_implemented(..))` = HTTP 501, and no state is touched. Contracts cited from the crate's `OPERATIONS.md` (CreateBudget/DescribeBudget/DescribeBudgets/UpdateBudget/DeleteBudget/DescribeNotificationsForBudget sections: input/output members, pagination traits, and error list incl. DuplicateRecordException 409, NotFoundException 404, InvalidParameterException 400, InvalidNextTokenException 400) and the `Budget`/`Notification`/`Subscriber` shapes from the Smithy model (required members BudgetName/TimeUnit/BudgetType; Notification requires NotificationType/ComparisonOperator/Threshold; Subscriber requires SubscriptionType/Address). Wire protocol awsJson1_1 with `X-Amz-Target: AWSBudgetServiceGateway.<Op>` per OPERATIONS.md 'Wire protocol'. No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget and body {"AccountId":"123456789012","Budget":{"BudgetName":"monthly-cost","BudgetLimit":{"Amount":"100","Unit":"USD"},"TimeUnit":"MONTHLY","BudgetType":"COST"},"NotificationsWithSubscribers":[{"Notification":{"NotificationType":"ACTUAL","ComparisonOperator":"GREATER_THAN","Threshold":80,"ThresholdType":"PERCENTAGE"},"Subscribers":[{"SubscriptionType":"EMAIL","Address":"ops@example.com"}]}]} — expect 200 with empty body (actual: 501 NotImplemented)
2. CreateBudget again with the same AccountId/BudgetName — expect DuplicateRecordException (HTTP 409)
3. DescribeBudget {"AccountId":"123456789012","BudgetName":"monthly-cost"} — expect Budget with BudgetLimit 100 USD, TimeUnit MONTHLY, BudgetType COST, a LastUpdatedTime
4. DescribeBudgets {"AccountId":"123456789012","MaxResults":1} — expect the single created budget in Budgets (and NextToken only if more exist)
5. UpdateBudget {"AccountId":"123456789012","NewBudget":{"BudgetName":"monthly-cost","BudgetLimit":{"Amount":"250","Unit":"USD"},"TimeUnit":"MONTHLY","BudgetType":"COST"}} then DescribeBudget — expect BudgetLimit 250 USD
6. DescribeNotificationsForBudget {"AccountId":"123456789012","BudgetName":"monthly-cost"} — expect one notification ACTUAL/GREATER_THAN/80/PERCENTAGE
7. DeleteBudget {"AccountId":"123456789012","BudgetName":"monthly-cost"} — expect 200, then DescribeBudget for the same name — expect NotFoundException (HTTP 404)
8. DescribeBudget for a never-created name, and CreateBudget with "AccountId":"12345" — expect NotFoundException (404) and InvalidParameterException (400) respectively
