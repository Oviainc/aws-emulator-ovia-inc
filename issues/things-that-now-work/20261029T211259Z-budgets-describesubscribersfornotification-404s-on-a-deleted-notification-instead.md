Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: budgets.delete-last-subscriber-removes-notification budgets.delete-notification-cascades-subscribers budgets.create-budget-action-invalid-execution-role-rejected budgets.create-budget-action-starts-standby
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|DescribeSubscribersForNotification|NotFoundException
Issue-Key: budgets.create-budget-action-invalid-execution-role-rejected|_error#8|AssertionError|raise AssertionError(f"expected AccessDeniedException, but the call SUCCEEDED and returned {result!r}")
Issue-Key: budgets.create-budget-action-starts-standby|run#11|AssertionError|assert described["Status"] == "PENDING", (
Issue-Commit: 95ee1f215215607c18ab877072afcef7ad3742b1

# budgets: DescribeSubscribersForNotification 404s on a deleted notification instead of answering 200 with no Subscribers, CreateBudgetAction accepts a non-assumable ExecutionRoleArn, and a fresh action is born STANDBY instead of PENDING

> _Note: this issue was identified against the code base at commit `95ee1f215215607c18ab877072afcef7ad3742b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSubscribersForNotification

## Description
Bundle of 3 net-new root causes in the budgets emulator's notification-read semantics and budget-action lifecycle. No open budgets issue covers them: 20261029T163337Z-budgets-createbudget-event-propagation.md is about cross-account SNS subscribers, 20261024T085440Z-budgets-executebudgetaction-event-propagation.md is about an executed action not applying its Definition, and 20261029T064018Z / 20261029T093141Z are pagination concerns.

Cause 1 of 3: DescribeSubscribersForNotification refuses a notification that no longer exists instead of reporting an empty subscriber set.
AWS: Smithy: DescribeSubscribersForNotification output member Subscribers is OPTIONAL (list, @length min 1 max 11), so 'no subscribers' is expressed by omitting the member, not by an empty list; NotFoundException (404) is modeled but recorded behavior shows it is not used for this case. Recorded behavior: after the notification is gone (either because DeleteNotification removed it, or because deleting its last subscriber removed it), DescribeSubscribersForNotification for that Notification answers HTTP 200 with a body that carries no 'Subscribers' key at all — never an error, and never the deleted subscriber.
Emulator: raises NotFoundException: 'Unable to get subscribers: <BudgetName> - the notification doesn't exist.' (fixed text 'Unable to get subscribers: ' + ' - the notification doesn't exist.', with the budget name from the request).
Why: a client verifying that a cascade completed — the normal way to confirm DeleteNotification or last-subscriber cleanup — gets a hard 404 ClientError where AWS gives it a 200 with an absent member. The call raises in botocore and aborts the caller's flow instead of returning an empty result, so correct post-delete reconciliation code breaks against the emulator.
Fix: the DescribeSubscribersForNotification handler / its state lookup — a missing notification under an existing budget must serialize an empty success response (Subscribers member omitted) rather than take the not-found path.
Traces: budgets.delete-last-subscriber-removes-notification, budgets.delete-notification-cascades-subscribers (both [error]: the NotFoundException propagated out of the check).

Cause 2 of 3: CreateBudgetAction does not verify the ExecutionRoleArn is an existing, Budgets-assumable role — it accepts a ghost role and creates the action.
AWS: Smithy: CreateBudgetAction.ExecutionRoleArn is required, @length 32..618, pattern ^arn:aws(...)?:iam::\d{12}:role(/[!-~]+/|/)[\w+=,.@-]+$, and the operation declares AccessDeniedException (errorKind client, httpError 403, doc "You are not authorized to use this operation with the given parameters."). Recorded behavior: a syntactically valid ARN naming a role that does not exist (or that does not trust budgets.amazonaws.com) is refused with AccessDeniedException and the message 'Budgets permission required to assume [ExecutionRole: <ExecutionRoleArn>]. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].' — fixed text 'Budgets permission required to assume [ExecutionRole: ', ']. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].', with the ARN copied from the request. No action is created: DescribeBudgetActionsForBudget still returns no actions.
Emulator: answers HTTP 200 and returns a freshly minted ActionId, so the action is persisted against a role that cannot be assumed (observed response: {'AccountId': '000000000000', 'BudgetName': ..., 'ActionId': ...}).
Why: this is an overly permissive accept — the emulator lets a configuration AWS rejects at create time exist, so IaC and provisioning tests that assert the role must exist before the action can be created pass against the emulator and fail against AWS. It also leaves the account holding an action that could never execute, with no error surfaced anywhere.
Fix: the CreateBudgetAction handler's validation — after shape validation it must resolve ExecutionRoleArn against IAM state and check the role's trust policy for the budgets.amazonaws.com service principal, refusing with AccessDeniedException (403) and creating nothing.
Traces: budgets.create-budget-action-invalid-execution-role-rejected.

Cause 3 of 3: a newly created budget action's ActionStatus is STANDBY, not PENDING.
AWS: Smithy: ActionStatus enum = STANDBY, PENDING, EXECUTION_IN_PROGRESS, EXECUTION_SUCCESS, EXECUTION_FAILURE, REVERSE_IN_PROGRESS, REVERSE_SUCCESS, REVERSE_FAILURE, RESET_IN_PROGRESS, RESET_FAILURE. Recorded behavior: immediately after CreateBudgetAction, DescribeBudgetAction reports Status PENDING — the action is awaiting the budget engine's first evaluation of the threshold — and PENDING is a settled state that holds across repeated reads (the check re-reads 3 times, 5s apart, and requires PENDING each time); it does not advance on its own.
Emulator: DescribeBudgetAction reports 'STANDBY' for the freshly created action.
Why: a client that gates on the initial lifecycle state (waiting for PENDING, or treating STANDBY as 'already evaluated, armed and waiting for a threshold breach') reads the action's lifecycle position wrong, and any state machine driven off ActionStatus starts in the wrong state.
Fix: the CreateBudgetAction handler / budget-action state initialization, which must seed the stored ActionStatus as PENDING and leave advancement to the evaluation path.
Caution for the fixer: two OTHER checks in this same suite assert STANDBY as a positive control for a freshly created action (budgets.create-budget-action-invalid-execution-role-rejected asserts described['Status'] == 'STANDBY', and budgets.execute-budget-action-wrong-state-rejected expects the Standby-state refusal message). Those assertions were not reached in this run because each check failed earlier, so this cause is reported as the suite states it, but the initial-state change must be made together with whatever the engine's first evaluation does so those controls are satisfied too.
Traces: budgets.create-budget-action-starts-standby.

## Evidence
Suite: simparity-behavioral-aws-v1, 14/25 passed, all 11 failures in budgets.

Cause 1 — budgets.delete-last-subscriber-removes-notification [error]:
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeSubscribersForNotification operation: Unable to get subscribers: <budget-name> - the notification doesn't exist.
  budgets.delete-notification-cascades-subscribers [error]:
  botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the DescribeSubscribersForNotification operation: Unable to get subscribers: <budget-name> - the notification doesn't exist.
  In both cases the preceding assertion that DescribeNotificationsForBudget returned [] had already passed, so the cascade itself happened; only the read of the dead notification's subscribers diverges. Expected: HTTP 200 with the 'Subscribers' key absent (checks read it as resp.get('Subscribers', []) and require []).

Cause 2 — budgets.create-budget-action-invalid-execution-role-rejected [mismatch]:
  AssertionError: expected AccessDeniedException, but the call SUCCEEDED and returned {'AccountId': '000000000000', 'BudgetName': 'sim2real-budget-badrole-...', 'ActionId': 'c40471e7-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}
  Expected error: code AccessDeniedException (403), message 'Budgets permission required to assume [ExecutionRole: arn:aws:iam::<account>:role/<ghost-role>]. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].' and no action created (DescribeBudgetActionsForBudget -> Actions == []). The account id, role name and ActionId above are per-run values; the requirement is the refusal, its code/status and the message template.

Cause 3 — budgets.create-budget-action-starts-standby [mismatch]:
  AssertionError: expected a freshly created action to be PENDING (awaiting the engine's first threshold evaluation), got 'STANDBY'

## Steps to Reproduce
All against the emulator with account 000000000000.

Cause 1: CreateBudget N (BudgetLimit 50.0 USD, MONTHLY, COST); CreateNotification N with Notification={ACTUAL, GREATER_THAN, Threshold 80.0, PERCENTAGE} and one EMAIL subscriber; then either (a) DeleteNotification for that Notification, or (b) DeleteSubscriber for its only subscriber; then DescribeSubscribersForNotification(AccountId, BudgetName=N, Notification=<same>) -> emulator raises NotFoundException; AWS answers 200 with no Subscribers key.

Cause 2: CreateBudget N; CreateBudgetAction(AccountId, BudgetName=N, NotificationType=ACTUAL, ActionType=APPLY_IAM_POLICY, ActionThreshold={80.0, PERCENTAGE}, Definition={IamActionDefinition:{PolicyArn:'arn:aws:iam::aws:policy/ReadOnlyAccess', Roles:[<ghost-role-name>]}}, ExecutionRoleArn='arn:aws:iam::<account>:role/<ghost-role-name>' for a role never created, ApprovalModel=MANUAL, Subscribers=[one EMAIL]) -> emulator returns 200 + ActionId; then DescribeBudgetActionsForBudget shows the action exists.

Cause 3: CreateBudget N; create an IAM role trusting budgets.amazonaws.com; CreateBudgetAction as above with that real role; DescribeBudgetAction immediately -> Action.Status is 'STANDBY' (AWS: 'PENDING', stable across three reads 5s apart).

Replay: the check ids in trace_ids.
