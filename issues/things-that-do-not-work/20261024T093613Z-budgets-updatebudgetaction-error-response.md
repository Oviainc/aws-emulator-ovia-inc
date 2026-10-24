Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a61b7e3a8f074f2acfd74e18fad3041c10471e17

# UpdateBudgetAction never answers ResourceLockedException for an action whose execution is still in flight

_Filed: 2026-10-24T09:36:13Z_
_Operation: UpdateBudgetAction | Class: error-response_

> _Note: this issue was identified against the code base at commit `a61b7e3a8f074f2acfd74e18fad3041c10471e17`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ResourceLockedException` ("The request was received and recognized by the server, but the server rejected that particular method for the requested resource") is a modeled error of exactly the three mutating budget-action operations — ExecuteBudgetAction, UpdateBudgetAction and DeleteBudgetAction. The emulator honours the lock only in ExecuteBudgetAction: `execute_budget_action.rs:227-238` refuses a second execution while `crate::state::action_is_in_progress(status)` holds (EXECUTION_IN_PROGRESS / REVERSE_IN_PROGRESS / RESET_IN_PROGRESS).

UpdateBudgetAction performs no such check. `update_budget_action.rs:196-247` resolves the action (materialising any settle the clock already made due) and then overwrites NotificationType, ActionThreshold, Definition, ExecutionRoleArn, ApprovalModel and Subscribers regardless of status.

Expected: an UpdateBudgetAction against an action in one of the `*_IN_PROGRESS` statuses is refused with the modeled `ResourceLockedException`, and the stored action is left exactly as it was (the in-flight work keeps the definition/role it was started with). Actual: HTTP 200, the action's Definition/ExecutionRoleArn/Subscribers are replaced mid-flight, an `UPDATE_ACTION` record is appended to the history, and the pending settle then completes against the mutated action — a state AWS does not allow to arise.

## Evidence

Spec: Smithy model — `UpdateBudgetAction` declares `ResourceLockedException` among its errors (see OPERATIONS.md "## UpdateBudgetAction" error list; shape doc: "the server rejected that particular method for the requested resource"). The crate's own state module states the rule: src/state.rs `action_is_in_progress` doc — "An action in one of them is the locked resource `ResourceLockedException` names".
Handler: src/ops/update_budget_action.rs:196-254 — no call to `crate::state::action_is_in_progress`; `grep -n 'ResourceLockedException {' src/ops` matches only src/ops/execute_budget_action.rs:229, so the variant is unreachable from UpdateBudgetAction.
Tests: tests/all/budget_action_history.rs exercises UpdateBudgetAction only from settled statuses (rows at :470-510, :1022-1038, :1125, :1274) and asserts 200/NotFoundException; the ResourceLocked assertions in tests/all/budget_action_execution.rs:676 and :805 are for ExecuteBudgetAction only. No test asserts a locked update.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudget to create budget "locked-update" for AccountId 123456789012.
2. CreateBudgetAction on that budget (ActionType APPLY_IAM_POLICY, matching Definition, ApprovalModel AUTOMATIC) and keep the returned ActionId.
3. ExecuteBudgetAction with ExecutionType APPROVE_BUDGET_ACTION — the action moves to EXECUTION_IN_PROGRESS and the settle is owed later (state.rs ACTION_SETTLES_AFTER).
4. Immediately call UpdateBudgetAction for the same ActionId with a replacement Definition/ExecutionRoleArn/Subscribers: the emulator answers 200 and DescribeBudgetAction shows the replaced members while the status is still EXECUTION_IN_PROGRESS; AWS refuses the update with ResourceLockedException and leaves the action unchanged.
5. Compare with ExecuteBudgetAction issued at the same moment, which the emulator correctly refuses with ResourceLockedException.
