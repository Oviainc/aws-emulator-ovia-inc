Issue-Source: trace-sweep-divergence
Issue-Trace: budgets-account-wide-listings
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: budgets-account-wide-listings budgets-cost-budget-lifecycle budgets-planned-budget-limits budgets-notification-subscriber-workflow
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|DescribeBudgets|2xx->2xx|$.Budgets[*]
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.CostTypes
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.CreatedAt
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.HealthStatus
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.TimePeriod
Issue-Key: budgets|DescribeBudget|2xx->2xx|$.Budget.BudgetLimit
Issue-Key: budgets|DescribeNotificationsForBudget|2xx->2xx|$.Notifications[*]
Issue-Commit: 67b7a4ea014b7cfd9cdb5213b26731b0db184c45

# Budgets read paths drop stored budget and notification state

> _Note: this issue was identified against the code base at commit `67b7a4ea014b7cfd9cdb5213b26731b0db184c45`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeBudget

## Description
The budgets emulator accepts CreateBudget/CreateNotification input and then serves back only a thin subset of the stored resource: the Budget structure it returns on read omits every member it did not echo verbatim from the create call, and notifications attached to a budget are not visible at all. Four causes, all in the budgets read path and the state it reads from.

Cause 1 of 4 - DescribeBudgets projects an incomplete Budget.
AWS: Smithy (com.amazonaws.budgets, awsJson1_1) DescribeBudgets -> DescribeBudgetsResponse.Budgets: list of Budget. Budget models TimePeriod, CostTypes, HealthStatus and LastUpdatedTime as members; Budget.TimePeriod's own documentation states that a budget created without a start/end date defaults to the start of the chosen TimeUnit period and an end date of 06/15/87 00:00 UTC, so a created COST budget always has a TimePeriod on read. CostTypes defaulting for a COST budget (all Include* true, UseBlended/UseAmortized false), HealthStatus {Status: HEALTHY, LastUpdatedTime} and the CreatedAt field are recorded behavior only (CreatedAt is not in the on-disk model); the recording shows all of them present on a listed budget. Status 200.
Emulator: status 200, but the listed budget carries only BudgetName, BudgetLimit, TimeUnit, BudgetType and LastUpdatedTime - TimePeriod, CostTypes, CreatedAt and HealthStatus are absent. The LastUpdatedTime it does return is the time of the read rather than the time of the last create/update (recorded: it equals HealthStatus.LastUpdatedTime, i.e. the create time; exact epoch values are environment-specific and are not the requirement, the fact that it is recomputed per request is).
Why: a client listing budgets cannot see the period a budget covers, whether the budget is healthy, or which cost types are included, and sees every budget as just-modified, so change detection and any period-aware rendering or diffing is wrong.
Fix: the DescribeBudgets handler's Budget projection/serializer plus the CreateBudget state write that must persist the defaulted TimePeriod/CostTypes/HealthStatus/CreatedAt and a stable LastUpdatedTime, in the budgets service crate.
Traces: budgets-account-wide-listings#task#3 (also budgets-cost-budget-lifecycle#task#3).

Cause 2 of 4 - DescribeBudget projects the same incomplete Budget.
AWS: Smithy DescribeBudget -> DescribeBudgetResponse.Budget: Budget. Same members as cause 1; the recorded 200 response carries CostTypes, CreatedAt, HealthStatus and TimePeriod.
Emulator: status 200, Budget present but Budget.CostTypes, Budget.CreatedAt, Budget.HealthStatus and Budget.TimePeriod are all null/absent.
Why: the single-resource read is the call a client makes after create to confirm what AWS stored; it reports a budget with no period and no health, so round-trip verification and any read-modify-write via UpdateBudget loses those fields.
Fix: the DescribeBudget handler's Budget projection/serializer and the shared Budget state record it reads.
Traces: budgets-cost-budget-lifecycle#task#2.

Cause 3 of 4 - a PlannedBudgetLimits budget gets no derived BudgetLimit on read.
AWS: Smithy Budget.PlannedBudgetLimits documentation: "DescribeBudget and DescribeBudgets response along with PlannedBudgetLimits also contain BudgetLimit representing the current month or quarter limit present in PlannedBudgetLimits". The recorded 200 response for a budget created with PlannedBudgetLimits therefore carries BudgetLimit (Amount/Unit taken from the current period's planned limit) in addition to the other members of cause 2.
Emulator: status 200 with Budget.BudgetLimit absent, along with CostTypes, CreatedAt, HealthStatus and TimePeriod.
Why: a client that reads BudgetLimit to show "the limit in force this month" gets nothing for every planned-limit budget, even though AWS always derives it.
Fix: the DescribeBudget/DescribeBudgets Budget projection - derive BudgetLimit from the PlannedBudgetLimits entry for the current period when the budget was created with planned limits.
Traces: budgets-planned-budget-limits#task#2.

Cause 4 of 4 - DescribeNotificationsForBudget returns an empty list for a budget that has a notification.
AWS: Smithy DescribeNotificationsForBudget -> DescribeNotificationsForBudgetResponse.Notifications: list of Notification (NotificationType, ComparisonOperator, Threshold required; NotificationState optional). A notification supplied through CreateBudget.NotificationsWithSubscribers / CreateNotification is readable here; the recorded 200 response returns it with NotificationState OK (and a ConsecutiveBreachCount field that is recorded behavior only - it is not in the on-disk model).
Emulator: status 200 with Notifications empty - $.Notifications[0] is absent.
Why: notifications and their subscribers become invisible after creation, so a client cannot verify, update or delete them, and a workflow that lists before updating concludes the budget has no alerting at all.
Fix: persist notifications on the CreateBudget/CreateNotification write path and return them from the DescribeNotificationsForBudget handler, in the budgets service crate.
Traces: budgets-notification-subscriber-workflow#task#3 (also the repeat read at budgets-notification-subscriber-workflow#task#9).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

1. budgets-account-wide-listings#task#3 DescribeBudgets - expected 200, got 200. $.Budgets[0]: expected {BudgetName, BudgetLimit{Amount:'300.0',Unit:'USD'}, TimeUnit:'MONTHLY', BudgetType:'COST', TimePeriod{Start,End}, CostTypes{...all Include* true, UseBlended/UseAmortized false}, CreatedAt, HealthStatus{Status:'HEALTHY',LastUpdatedTime}, LastUpdatedTime}; got only {BudgetName, BudgetLimit, TimeUnit, BudgetType, LastUpdatedTime}, where LastUpdatedTime is the replay-time clock value rather than the stored create/update time. Cluster also covers budgets-cost-budget-lifecycle#task#3.

2. budgets-cost-budget-lifecycle#task#2 DescribeBudget - expected 200, got 200. $.Budget.CostTypes missing (expected the full defaulted CostTypes map), $.Budget.CreatedAt missing, $.Budget.HealthStatus missing (expected {Status:'HEALTHY',LastUpdatedTime}), $.Budget.TimePeriod missing (expected {Start,End}); all four got None.

3. budgets-planned-budget-limits#task#2 DescribeBudget - expected 200, got 200. $.Budget.BudgetLimit missing (expected {Amount:'50.0',Unit:'USD'} derived from the current period's PlannedBudgetLimits entry), plus $.Budget.CostTypes, $.Budget.CreatedAt, $.Budget.HealthStatus and $.Budget.TimePeriod missing.

4. budgets-notification-subscriber-workflow#task#3 DescribeNotificationsForBudget - expected 200, got 200. $.Notifications[0] missing (expected {NotificationType:'ACTUAL', ComparisonOperator:'GREATER_THAN', Threshold:80.0, NotificationState:'OK', ConsecutiveBreachCount:0}), got None. Cluster also covers budgets-notification-subscriber-workflow#task#9.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture budgets-account-wide-listings, step task#3 - CreateBudget a MONTHLY COST budget with only BudgetName/BudgetLimit/TimeUnit/BudgetType, then DescribeBudgets for the account; inspect $.Budgets[0] for TimePeriod, CostTypes, CreatedAt, HealthStatus and whether LastUpdatedTime changes between two successive reads.
3. Cause 2: replay fixture budgets-cost-budget-lifecycle, step task#2 - DescribeBudget for the budget created in the previous step; inspect $.Budget.CostTypes / CreatedAt / HealthStatus / TimePeriod.
4. Cause 3: replay fixture budgets-planned-budget-limits, step task#2 - CreateBudget with PlannedBudgetLimits (no BudgetLimit), then DescribeBudget; inspect $.Budget.BudgetLimit.
5. Cause 4: replay fixture budgets-notification-subscriber-workflow, steps task#3 and task#9 - create a budget with a notification/subscriber, then DescribeNotificationsForBudget; inspect $.Notifications.
