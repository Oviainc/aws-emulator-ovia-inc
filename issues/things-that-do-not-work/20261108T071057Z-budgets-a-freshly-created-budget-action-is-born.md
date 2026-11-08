Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: budgets.create-budget-action-starts-standby
Issue-Shape-Id: com.amazonaws.budgets#AWSBudgetServiceGateway
Issue-Key: budgets.create-budget-action-starts-standby|run#11|AssertionError|assert described["Status"] == "PENDING", (
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# budgets: a freshly created budget action is born STANDBY instead of PENDING, so it never awaits the engine's first threshold evaluation

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBudgetAction

## Description
One net-new root cause in the budgets emulator's budget-action creation path.

Cause 1 of 1: CreateBudgetAction assigns the wrong initial ActionStatus.

AWS: Smithy (services/service-com-amazonaws-budgets-awsbudgetservicegateway) models Action.Status as a required ActionStatus enum with values STANDBY | PENDING | EXECUTION_IN_PROGRESS | EXECUTION_SUCCESS | EXECUTION_FAILURE | REVERSE_IN_PROGRESS | REVERSE_SUCCESS | REVERSE_FAILURE | RESET_IN_PROGRESS | RESET_FAILURE, and ApprovalModel as a required enum AUTOMATIC | MANUAL; the model does not state which member a newly created action starts in, so for the initial value this is recorded behavior only. The behavioral recording shows that a budget action created with ApprovalModel=MANUAL, ActionType=APPLY_IAM_POLICY, ActionThreshold={80.0, PERCENTAGE} and an assumable ExecutionRoleArn is reported by DescribeBudgetAction with Status 'PENDING' immediately after creation, and that PENDING is a SETTLED state: three further independent DescribeBudgetAction reads, spaced apart, all still report 'PENDING' — the action does not advance on its own before the budget engine has evaluated it against the threshold. ApprovalModel and ActionType round-trip unchanged ('MANUAL', 'APPLY_IAM_POLICY'). The statuses here are the enum members themselves, not per-run values; only AccountId, BudgetName, ActionId and the execution-role name in the reproduction differ per run.

Emulator: DescribeBudgetAction on the just-created action reports Status 'STANDBY' (HTTP 200 on both sides; ApprovalModel and ActionType are correct). The check fails on its first read with "expected a freshly created action to be PENDING (awaiting the engine's first threshold evaluation), got 'STANDBY'", so the emulator is seeding the initial status at creation time rather than leaving the action in the pre-evaluation state.

Why: Action.Status is the only handle a client has on a budget action's lifecycle. A caller that creates an action and then waits for / branches on the post-creation status reads a different enum member against the emulator than against AWS, so approval dashboards, polling loops and IaC/test assertions that gate on the freshly created state take the wrong branch, and a MANUAL-approval action appears to be in a different phase of its state machine than AWS reports.

Fix: the CreateBudgetAction handler / the budgets state layer that constructs the stored Action record — the initial ActionStatus it writes (and, correspondingly, what DescribeBudgetAction and DescribeBudgetActionsForBudget serialize for a never-evaluated action). It must be a settled state that no timer or background transition moves off before a threshold evaluation.

NOTE for the fixer — conflicting open issue, reconcile before changing the constant: 20261029T214009Z-budgets-budget-action-lifecycle-diverges-missing-accessdenied-and.md (open) asserts, from the WIRE corpus, that an action created the same way sits in 'STANDBY' pre-execute (its Cause 1 expects ExecuteBudgetAction to be refused with ResourceLockedException 'This method is not allowed during [ActionStatus: Standby]', and its Cause 2 expects DescribeBudgetAction to still read 'STANDBY' after that refusal). The replay reports no owner for this behavioral check, so it is filed here, but a blind flip of the creation-time constant from STANDBY to PENDING will break that issue's wire steps. The two recordings must be reconciled — e.g. by distinguishing the pre-evaluation state from the armed/standby state per ApprovalModel, and by rendering the ResourceLockedException message from whatever status is actually stored rather than from a hard-coded word — and both the behavioral check listed under Traces and that issue's fixtures re-replayed together.

Traces: behavioral check budgets.create-budget-action-starts-standby.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 17/25 passed (68.0%), 8 mismatched, 0 errored, 0 skipped; all 8 failures are under service heading 'budgets', and 7 of them are reported OWNED by 20261029T211259Z-budgets-six-error-fidelity-divergences-four-wrong-invalidparameter.md and are not part of this issue.

[mismatch] budgets.create-budget-action-starts-standby
  check_budgets_create_budget_action_starts_standby.py line 49:
    assert described["Status"] == "PENDING"
  AssertionError: expected a freshly created action to be PENDING (awaiting the engine's first threshold evaluation), got 'STANDBY'

So for DescribeBudgetAction on a just-created action: $.Action.Status expected 'PENDING', got 'STANDBY' (HTTP 200 on both sides). The same read's $.Action.ApprovalModel ('MANUAL') and $.Action.ActionType ('APPLY_IAM_POLICY') matched. The check never reached its freeze detector (three more reads at 5s spacing, each of which must also see 'PENDING'), so whether the emulator's status is stable over time is unverified by this run — only its value is wrong.

All names/ids in the reproduction below (budget name sim2real-budget-actstandby-*, role name sim2real-budgets-role-*, the generated ActionId, the account id) are per-run values and are not part of the requirement; the required value is the ActionStatus enum member.

## Steps to Reproduce
Against the emulator's budgets endpoint, with AccountId from sts:GetCallerIdentity:

1. CreateBudget(AccountId, Budget={BudgetName=B, BudgetLimit={Amount='200.0', Unit='USD'}, TimeUnit='MONTHLY', BudgetType='COST'}).
2. In IAM, create a role R whose trust policy allows sts:AssumeRole for Principal Service 'budgets.amazonaws.com', and attach an inline policy permitting iam:Attach*Policy / iam:Detach*Policy on '*'.
3. CreateBudgetAction(AccountId, BudgetName=B, NotificationType='ACTUAL', ActionType='APPLY_IAM_POLICY', ActionThreshold={ActionThresholdValue=80.0, ActionThresholdType='PERCENTAGE'}, Definition={IamActionDefinition={PolicyArn='arn:aws:iam::aws:policy/ReadOnlyAccess', Roles=[R]}}, ExecutionRoleArn=<R's ARN>, ApprovalModel='MANUAL', Subscribers=[{SubscriptionType='EMAIL', Address='sim2real-budgets-action@example.com'}]) -> note the returned ActionId.
4. DescribeBudgetAction(AccountId, BudgetName=B, ActionId) -> inspect Action.Status. Emulator returns 'STANDBY'; expected 'PENDING'.
5. Repeat step 4 three more times with ~5s between reads -> every read must still report 'PENDING' (no self-driven move to EXECUTION_IN_PROGRESS/EXECUTED or any other transitional or terminal member).
6. Teardown: DeleteBudgetAction, DeleteBudget, delete the inline policy and the role.

Or replay the behavioral check budgets.create-budget-action-starts-standby directly.
