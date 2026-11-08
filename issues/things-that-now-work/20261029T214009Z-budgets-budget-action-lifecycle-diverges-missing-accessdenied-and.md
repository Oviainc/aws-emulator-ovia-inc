Issue-Source: trace-sweep-divergence
Issue-Trace: budgets-budget-action-execute-rejected-and-history
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: budgets-budget-action-execute-rejected-and-history budgets-budget-action-lifecycle budgets-budget-action-crud-lifecycle budgets-budget-action-history-pagination
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets|ExecuteBudgetAction|4xx->2xx|$.Message
Issue-Key: budgets|ExecuteBudgetAction|4xx->2xx|$.__type
Issue-Key: budgets|ExecuteBudgetAction|4xx->2xx|status
Issue-Key: budgets|DescribeBudgetAction|2xx->2xx|$.Action.Status
Issue-Key: budgets|DeleteBudgetAction|2xx->4xx|status
Issue-Key: budgets|CreateBudgetAction|4xx->2xx|$.Message
Issue-Key: budgets|CreateBudgetAction|4xx->2xx|$.__type
Issue-Key: budgets|CreateBudgetAction|4xx->2xx|status
Issue-Key: budgets|DescribeBudgetAction|4xx->4xx|$.Message
Issue-Key: budgets|DescribeBudgetActionHistories|2xx->2xx|$.ActionHistories[*].ActionHistoryDetails.Message
Issue-Key: budgets|DescribeBudgetActionHistories|2xx->2xx|$.ActionHistories[*]
Issue-Related: 20261029T214009Z-budgets-read-responses-drop-aws-fields-and-add.md
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# budgets budget-action lifecycle diverges: missing AccessDenied and ResourceLocked refusals, wrong post-refusal status, blocked delete, non-AWS history and not-found messages

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteBudgetAction

## Description
Bundle of 6 net-new root causes in the budgets emulator's BUDGET-ACTION lifecycle: two refusals AWS makes that the emulator answers 200 to, the corrupted action state that follows one of them, a delete that then fails, and two message templates the emulator words itself. Related open issues exist but do NOT cover these wire replay keys (the replay reports no owner for any of them): 20261029T211259Z-budgets-six-error-fidelity-divergences-four-wrong-invalidparameter.md describes the ExecuteBudgetAction mis-classification and the action-not-found message as seen from the BEHAVIORAL suite (its Cause 6 and Cause 5), and 20261029T211259Z-budgets-describesubscribersfornotification-404s-on-a-deleted-notification-instead.md describes the non-assumable-role accept (its Cause 2). The wire corpus shows those same defects plus four more; a fixer must satisfy the wire steps named here, which neither of those issues carries. 20261024T085440Z-budgets-executebudgetaction-event-propagation.md is about an executed action not applying its Definition, which is a different concern from the refusal/state causes below.

Cause 1 of 6: ExecuteBudgetAction succeeds on an action that is not in an executable state.
AWS: Smithy: ExecuteBudgetAction declares ResourceLockedException (errorKind client, doc "The request was received and recognized by the server, but the server rejected that particular method for the requested resource.") alongside InvalidParameterException and NotFoundException. The recording shows that executing an action whose ActionStatus is Standby is REFUSED: the response is an error body with __type 'ResourceLockedException' and Message 'This method is not allowed during [ActionStatus: Standby]' — fixed text 'This method is not allowed during [ActionStatus: ' + the action's current status as held in service state + ']'. The recorded HTTP status for that refusal is 400 (the model's httpError for the shape is 423; the wire recording is the requirement for the replayed step).
Emulator: answers HTTP 200 with a success body — no __type, no Message — and proceeds to execute.
Why: the whole point of the refusal is that a MANUAL-approval action in Standby must not be fired; a client (or an operator script) that relies on AWS rejecting a premature execution instead observes a successful execution, and nothing in the response tells it the state gate was skipped. It also makes the action's state machine advance from a state AWS never leaves this way.
Fix: the ExecuteBudgetAction handler's action-state gate — check the stored ActionStatus before doing any work and emit the ResourceLockedException error body (status and code as recorded, message rendered from the stored status) while leaving the action untouched.
Traces: budgets-budget-action-execute-rejected-and-history#task#4.

Cause 2 of 6: after that un-refused execute, the action is left in EXECUTION_IN_PROGRESS instead of remaining in its pre-execute state.
AWS: Smithy: Action.Status is a required ActionStatus enum (STANDBY, PENDING, EXECUTION_IN_PROGRESS, EXECUTION_SUCCESS, EXECUTION_FAILURE, REVERSE_*, RESET_*). The recording shows that because the execute was refused, the subsequent DescribeBudgetAction still reports Status 'STANDBY' — a refused call must not mutate the resource.
Emulator: DescribeBudgetAction reports 'EXECUTION_IN_PROGRESS' (and the same read also drops Subscribers[0].VerificationStatus, which is Cause 3 of the companion serialization issue, not this cause).
Why: the action is now parked in a transient state it can never leave, so any client polling for a terminal status hangs, and the resource's state no longer reflects what the API told the caller.
Fix: follows from Cause 1 — the state transition must happen only on an accepted execute; the handler must not write EXECUTION_IN_PROGRESS before the state gate passes.
Traces: budgets-budget-action-execute-rejected-and-history#task#5.

Cause 3 of 6: DeleteBudgetAction then fails with 400 on an action AWS deletes successfully.
AWS: Smithy: DeleteBudgetAction returns AccountId, BudgetName and the deleted Action (required), and declares NotFoundException and ResourceLockedException. The recording shows the teardown delete of this action answering HTTP 200 with the deleted action.
Emulator: answers HTTP 400.
Why: the action cannot be removed, so the resource leaks and any teardown/cleanup path (IaC destroy, test fixture teardown) fails for every action that has been through this code path. Hypothesis: the delete is refusing because the action was left in the transient EXECUTION_IN_PROGRESS state by Cause 2; if so, fixing Causes 1-2 removes the trigger, but the delete path must still be verified against an action in a non-terminal state rather than returning a 400 for it.
Fix: the DeleteBudgetAction handler's pre-delete state gate (and the state left behind by the execute path).
Traces: budgets-budget-action-execute-rejected-and-history#teardown#102.

Cause 4 of 6: CreateBudgetAction accepts an ExecutionRoleArn that Budgets cannot assume.
AWS: Smithy: CreateBudgetAction.ExecutionRoleArn is required (RoleArn, @length 32..618, @pattern ^arn:aws(-eusc|-cn|-us-gov|-iso|-iso-[a-z]{1})?:iam::\d{12}:role(/[!-~]+/|/)[\w+=,.@-]+$) and the operation declares AccessDeniedException (errorKind client, httpError 403, doc "You are not authorized to use this operation with the given parameters."). The recording shows a syntactically valid ARN naming a role that does not exist / does not trust the Budgets service principal being refused with __type 'AccessDeniedException' and Message 'Budgets permission required to assume [ExecutionRole: <ExecutionRoleArn>]. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].' — fixed text 'Budgets permission required to assume [ExecutionRole: ' + the ARN copied from the request + ']. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].'; recorded HTTP status 400 for the replayed step.
Emulator: answers HTTP 200 and mints an ActionId, persisting an action bound to a role that can never be assumed.
Why: an overly permissive accept — provisioning code and IaC tests that depend on AWS rejecting a mis-wired execution role pass against the emulator and fail against AWS, and the account is left holding an action that could never execute with no error surfaced anywhere.
Fix: the CreateBudgetAction handler's validation — after shape validation, resolve ExecutionRoleArn against IAM state and check its trust policy for the budgets.amazonaws.com service principal; refuse with the AccessDeniedException body and create nothing.
Traces: budgets-budget-action-lifecycle#task#4.

Cause 5 of 6: the action-not-found message uses a budget-shaped template.
AWS: Smithy: DescribeBudgetAction declares NotFoundException (doc "We can't locate the resource that you specified."). The recording shows a missing action id answered with Message 'Action for [AccountId: <AccountId>, ActionId: <ActionId>] is not found!' — fixed text 'Action for [AccountId: ', ', ActionId: ', '] is not found!', with the account id and action id taken from the request; recorded status 400, which the emulator already matches.
Emulator: same status but Message "Unable to get budget action: <ActionId> - the action doesn't exist." — the budget-not-found template, dropping AccountId entirely.
Why: NotFoundException is shared by every missing-resource case in this service, so the message is the only discriminator between a missing budget and a missing action; a client that distinguishes them by text cannot.
Fix: the DescribeBudgetAction handler / budget-action lookup not-found error construction.
Traces: budgets-budget-action-crud-lifecycle#task#7.

Cause 6 of 6: action-history entries carry emulator-authored Message text instead of AWS's.
AWS: Smithy: ActionHistoryDetails.Message is a required string and ActionHistory carries Timestamp, Status, EventType and those details. The recording shows AWS wording each history event from the caller's point of view, with entirely fixed text per EventType: a CREATE_ACTION event has Message 'Customer created a budget action.' and an UPDATE_ACTION event has Message 'Customer updated a budget action.' (no variable parts).
Emulator: emits 'The budget action was created.' and 'The budget action was updated.' respectively — locally invented templates with no AWS provenance. Because the embedded Action is compared as a whole, the whole history entry diverges on the paginated read as well.
Why: the history Message is the human-facing audit record and the only place the actor of a change is recorded ('Customer' vs the system); log scrapers, audit exports and snapshot tests keyed on the AWS wording do not match, and the distinction between a customer-initiated and a system-generated event is lost.
Fix: the budget-action history writer that renders an event's Message per EventType in the budgets state layer.
Traces: budgets-budget-action-execute-rejected-and-history#task#3, budgets-budget-action-history-pagination#task#5, budgets-budget-action-history-pagination#task#6 (the whole-entry diff on the paginated page).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 69/89 steps passed, 20 mismatched, score 77.5%, emulator_up=true. All entries below are verdict=mismatch.

Cause 1 — fixture budgets-budget-action-execute-rejected-and-history step task#4, ExecuteBudgetAction:
  status: expected 400, got 200
  $.__type: missing (expected 'ResourceLockedException', got None)
  $.Message: missing (expected 'This method is not allowed during [ActionStatus: Standby]', got None)

Cause 2 — same fixture, step task#5, DescribeBudgetAction:
  $.Action.Status: value_mismatch (expected 'STANDBY', got 'EXECUTION_IN_PROGRESS')
  ($.Action.Subscribers[0].VerificationStatus also missing — filed as Cause 3 of the companion serialization bundle.)

Cause 3 — same fixture, step teardown#102, DeleteBudgetAction:
  status: expected 200, got 400

Cause 4 — fixture budgets-budget-action-lifecycle step task#4, CreateBudgetAction:
  status: expected 400, got 200
  $.__type: missing (expected 'AccessDeniedException', got None)
  $.Message: missing (expected 'Budgets permission required to assume [ExecutionRole: arn:aws:iam::000000000000:role/<ghost-role>]. Please follow the instruction to grant assumeRole access to [Service Principal: budgets.amazonaws.com].', got None) — the role name/account in the recording are per-run values filling the template; the fixed text and the service principal are the requirement.

Cause 5 — fixture budgets-budget-action-crud-lifecycle step task#7, DescribeBudgetAction (status 400 on both sides):
  $.Message: value_mismatch (expected 'Action for [AccountId: 000000000000, ActionId: <action-id>] is not found!', got "Unable to get budget action: <action-id> - the action doesn't exist.") — the account id and action id are request values; the bracketed template is fixed text.

Cause 6 — fixture budgets-budget-action-execute-rejected-and-history step task#3, DescribeBudgetActionHistories (200/200):
  $.ActionHistories[0].ActionHistoryDetails.Message: value_mismatch (expected 'Customer created a budget action.', got 'The budget action was created.')
  fixture budgets-budget-action-history-pagination step task#5: $.ActionHistories[0]...Message expected 'Customer created a budget action.' got 'The budget action was created.'; $.ActionHistories[1]...Message expected 'Customer updated a budget action.' got 'The budget action was updated.'
  fixture budgets-budget-action-history-pagination step task#6: $.ActionHistories[0], [1] and [2] unequal as whole objects; the only non-per-run difference inside each is the same ActionHistoryDetails.Message wording (Timestamp values are per-run).

## Steps to Reproduce
All against the emulator's budgets endpoint with account 000000000000; replay the fixtures/steps in Issue-Trace/Issue-Step and Issue-Trace-Ids for the exact recorded requests.

Causes 1-3: CreateBudget B (COST, MONTHLY, BudgetLimit); CreateBudgetAction on B with ActionType=APPLY_IAM_POLICY, ApprovalModel=MANUAL, ActionThreshold={80.0, PERCENTAGE}, Definition={IamActionDefinition:{PolicyArn:'arn:aws:iam::aws:policy/ReadOnlyAccess', Roles:[<role-name>]}}, an assumable ExecutionRoleArn and one EMAIL subscriber (the action sits in Standby); then (1) ExecuteBudgetAction(AccountId, BudgetName=B, ActionId, ExecutionType=APPROVE_BUDGET_ACTION) -> emulator returns 200 instead of the ResourceLockedException body; (2) DescribeBudgetAction -> Status is 'EXECUTION_IN_PROGRESS' instead of 'STANDBY'; (3) DeleteBudgetAction for the same ActionId -> HTTP 400 instead of 200 with the deleted Action.

Cause 4: CreateBudget B; CreateBudgetAction on B exactly as above but with ExecutionRoleArn='arn:aws:iam::000000000000:role/<role-that-was-never-created>' -> emulator returns 200 and an ActionId; AWS refuses with AccessDeniedException and the assume-role message and creates nothing (DescribeBudgetActionsForBudget stays empty).

Cause 5: CreateBudget B; DescribeBudgetAction(AccountId, BudgetName=B, ActionId=<a well-formed uuid that was never created>) -> inspect the NotFoundException Message.

Cause 6: CreateBudget B; CreateBudgetAction on B; UpdateBudgetAction on it (e.g. change ActionThreshold to 70.0); DescribeBudgetActionHistories(AccountId, BudgetName=B, ActionId) and again with MaxResults set so the page is served from the paginator -> inspect each ActionHistories[*].ActionHistoryDetails.Message.
