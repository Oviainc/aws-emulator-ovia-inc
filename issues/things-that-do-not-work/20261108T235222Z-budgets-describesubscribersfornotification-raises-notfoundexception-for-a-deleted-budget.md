Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: budgets.delete-budget-cascades-notifications-and-subscribers
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|DescribeSubscribersForNotification|NotFoundException
Issue-Commit: 20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6

# budgets: DescribeSubscribersForNotification raises NotFoundException for a deleted budget where AWS answers 200 with no Subscribers

> _Note: this issue was identified against the code base at commit `20f62a5c947e5bdfd6ecb788e0936a3bc521f6a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubscribersForNotification

## Description
One net-new root cause in the budgets emulator's DescribeSubscribersForNotification handler, found by the behavioral suite's delete-budget cascade check. No open budgets issue covers it: 20261029T214009Z-budgets-read-responses-drop-aws-fields-and-add.md is about member sets/renderings in 200 bodies (Notification.ConsecutiveBreachCount, Budget.CreatedAt, Subscriber.VerificationStatus, Amount formatting, empty ResourceTags) and never touches the missing-parent path of this operation; 20261024T085440Z-budgets-executebudgetaction-event-propagation.md and 20261108T071057Z-budgets-a-freshly-created-budget-action-is-born.md are budget-action findings.

Cause 1 of 1: after its parent budget is deleted, DescribeSubscribersForNotification refuses the call with NotFoundException instead of answering empty.
AWS: recorded behavior only for the status choice — Smithy (com.amazonaws.budgets#AWSBudgetServiceGateway) models NotFoundException (HTTP 404) on DescribeSubscribersForNotification, and its output member Subscribers is OPTIONAL (target Subscribers, @length min 1 max 11), so an empty result is expressed by OMITTING the member, not by an empty list. The recording that grounds this check shows that once the parent budget is gone, AWS answers DescribeSubscribersForNotification with HTTP 200 and a body carrying no Subscribers key — it does NOT raise, even though the sibling read DescribeNotificationsForBudget on the same dead budget DOES raise NotFoundException with the message "Unable to get budget: <BudgetName> - the budget doesn't exist." (fixed text "Unable to get budget: " / " - the budget doesn't exist.", with <BudgetName> taken verbatim from the request). The two reads deliberately differ, so the missing-budget check cannot be shared between them.
Emulator: answers DescribeSubscribersForNotification for the deleted budget with HTTP 404 NotFoundException, message "Unable to get budget: <BudgetName> - the budget doesn't exist." (replay shows botocore raising NotFoundException on the post-delete call, which aborts the check). Pre-delete the same call returns the one subscriber correctly, and the post-delete DescribeNotificationsForBudget refusal (code and message) matches AWS, so the defect is isolated to this operation's handling of an absent parent budget.
Why: a client draining subscribers after a budget teardown — the normal cascade-verification and cleanup idiom — gets a ClientError where AWS gives it an empty 200. Code written as `resp.get("Subscribers", [])` works against AWS and throws against the emulator, so teardown paths abort mid-way and leave the caller unable to distinguish "budget gone, nothing to clean up" from a genuine failure.
Fix: the DescribeSubscribersForNotification handler in the budgets crate — when the named budget (or its notification) does not exist, return 200 with the Subscribers member omitted rather than looking the budget up through the shared must-exist helper that DescribeNotificationsForBudget uses.
Traces: behavioral check budgets.delete-budget-cascades-notifications-and-subscribers.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 23/25 passed; this check reported verdict=error.

[error] budgets.delete-budget-cascades-notifications-and-subscribers
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeSubscribersForNotification operation: Unable to get budget: <BudgetName> - the budget doesn't exist.
  (the concrete budget name in the run is a per-run unique_name and is not part of the requirement)

The check reached that call only after these assertions PASSED, which fixes the surrounding behavior as correct and isolates the cause:
  - pre-delete DescribeNotificationsForBudget -> exactly 1 notification;
  - pre-delete DescribeSubscribersForNotification -> exactly 1 subscriber;
  - post-delete DescribeNotificationsForBudget -> NotFoundException with message "Unable to get budget: <BudgetName> - the budget doesn't exist."

Expected at the failing call: HTTP 200 with no Subscribers member (the check reads `.get("Subscribers", [])` and asserts it is empty).
Actual: HTTP 404, Code=NotFoundException, Message="Unable to get budget: <BudgetName> - the budget doesn't exist."

Because the call raised, the check's remaining assertions (mutation on the dead budget name refused with NotFoundException, and the name being reusable with zero notifications) were never evaluated and are not part of this finding.

## Steps to Reproduce
Against the emulator's budgets endpoint, with AccountId = the account STS reports:
1. CreateBudget B (BudgetType COST, TimeUnit MONTHLY, BudgetLimit {Amount '50.0', Unit 'USD'}).
2. CreateNotification on B with Notification={NotificationType: ACTUAL, ComparisonOperator: GREATER_THAN, Threshold: 80.0, ThresholdType: PERCENTAGE} and Subscribers=[{SubscriptionType: EMAIL, Address: <addr>}].
3. Confirm DescribeNotificationsForBudget(AccountId, BudgetName=B) returns 1 notification and DescribeSubscribersForNotification(AccountId, BudgetName=B, Notification=<same>) returns 1 subscriber.
4. DeleteBudget(AccountId, BudgetName=B).
5. Call DescribeSubscribersForNotification(AccountId, BudgetName=B, Notification=<same>) again -> the emulator raises 404 NotFoundException; AWS answers 200 with no Subscribers key.

Or replay the behavioral check budgets.delete-budget-cascades-notifications-and-subscribers.
