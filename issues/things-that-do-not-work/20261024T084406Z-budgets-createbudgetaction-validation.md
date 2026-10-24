Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# CreateBudgetAction accepts an ExecutionRoleArn belonging to a different account than AccountId

_Filed: 2026-10-24T08:44:05Z_
_Operation: CreateBudgetAction | Class: validation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Action$ExecutionRoleArn` is documented as "The role passed for action execution and reversion. Roles and actions must be in the same account." The modelled `@pattern` only fixes the ARN's shape (`arn:aws…:iam::\d{12}:role/...`); it cannot tie the 12 digits in the ARN to the request's `AccountId`, so that cross-member rule is the handler's to enforce, exactly like the `ActionType`→`Definition` pairing the handler already checks.

The handler never compares them: `CreateBudgetAction` with `AccountId: "123456789012"` and `ExecutionRoleArn: "arn:aws:iam::999999999999:role/BudgetActionRole"` is accepted with HTTP 200, a fresh `ActionId` is minted, and `DescribeBudgetAction` reports the foreign-account role as the action's execution role.

Expected: the create is refused (AWS answers a client error — `InvalidParameterException`/`AccessDeniedException` — for a role that is not in the action's account) and no action is persisted.
Actual: HTTP 200 and a stored action whose execution role AWS would never have accepted.

## Evidence

Spec: Smithy model — `Action$ExecutionRoleArn` doc "Roles and actions must be in the same account."; `CreateBudgetActionRequest$AccountId` is the account the action is created in; `CreateBudgetAction` declares `InvalidParameterException` (HTTP 400) and `AccessDeniedException` (HTTP 403).
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/create_budget_action.rs:189-283 — the only cross-member check is `crate::state::validate_action_definition(&input.action_type, &input.definition)` (line 214); `input.execution_role_arn` is copied into the stored `Action` at line 264 without being inspected or compared against `input.account_id`.
Tests: tests/all/budget_action_execution.rs `create_body`/`simple_create_body` (lines 140-185) always build the role ARN from the request's own account, and `a_created_action_round_trips_as_supplied` only asserts the round-trip; no test supplies a foreign-account role, so nothing asserts the refusal.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"b1","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. POST / with X-Amz-Target: AWSBudgetServiceGateway.CreateBudgetAction and body {"AccountId":"123456789012","BudgetName":"b1","NotificationType":"ACTUAL","ActionType":"APPLY_IAM_POLICY","ActionThreshold":{"ActionThresholdValue":80,"ActionThresholdType":"PERCENTAGE"},"Definition":{"IamActionDefinition":{"PolicyArn":"arn:aws:iam::123456789012:policy/Deny","Roles":["BudgetRole"]}},"ExecutionRoleArn":"arn:aws:iam::999999999999:role/BudgetActionRole","ApprovalModel":"AUTOMATIC","Subscribers":[{"SubscriptionType":"EMAIL","Address":"team@example.com"}]}
3. Emulator answers 200 with an ActionId; AWS refuses because the role is not in the action's account.
4. DescribeBudgetAction with that ActionId — the foreign-account ExecutionRoleArn is reported back, confirming the action was persisted.
