Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 3e20db6d733c4a1cf0674d75bed6ce3984e4f3c1

# UpdateBudgetAction applies none of the cross-member rules an action must satisfy: a foreign-account ExecutionRoleArn and an oversized per-type Subscribers set are accepted

_Filed: 2026-10-24T12:38:27Z_
_Operation: UpdateBudgetAction | Class: validation_

> _Note: this issue was identified against the code base at commit `3e20db6d733c4a1cf0674d75bed6ce3984e4f3c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateBudgetAction` replaces individual members of a stored `Action`, so the result has to satisfy the same rules the action's shape documents — exactly the argument the crate itself makes for budgets (src/state.rs:1105-1108: "Every rule holds for the whole `Budget` shape, so both write paths run all of them"). The action update path does not do that: the only cross-member rule it checks is the `ActionType`→`Definition` pairing. Two documented rules are skipped, so an update can move an existing, compliant action into a shape AWS would never accept:

1. `ExecutionRoleArn` — `Action$ExecutionRoleArn` is documented "The role passed for action execution and reversion. Roles and actions must be in the same account." The modelled `@pattern` only fixes the ARN's shape; it cannot tie the 12 digits in `iam::<account>:role/...` to the request's `AccountId`. An `UpdateBudgetAction` naming `ExecutionRoleArn: arn:aws:iam::999999999999:role/X` against an action in account `123456789012` is accepted with HTTP 200, and `DescribeBudgetAction` then reports the foreign-account role.

2. `Subscribers` — the subscriber set is documented as "one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers" (the `Notification` shape doc, which the crate already encodes as `MAX_SNS_SUBSCRIBERS_PER_NOTIFICATION = 1` / `MAX_EMAIL_SUBSCRIBERS_PER_NOTIFICATION = 10` and enforces on the notification write path through `crate::state::subscriber_limit_violation`). A replacement `Subscribers` list holding two `SNS` entries, or eleven `EMAIL` entries (both inside the modelled `@length(1,11)` total), is stored as sent.

Expected (AWS): the update is refused with a client error (`InvalidParameterException`, which `UpdateBudgetAction` declares) and the stored action is left exactly as it was — neither `OldAction` nor `NewAction` is returned, and no `UPDATE_ACTION` history record is appended.
Actual (emulator): HTTP 200, the offending value is persisted, `OldAction`/`NewAction` are reported, and an `UPDATE_ACTION` event is recorded in the action's history.

This is not covered by the two open create-side issues (CreateBudgetAction/validation for the foreign-account role, CreateBudgetAction/error-response for the per-type subscriber limits): fixing only `CreateBudgetAction` leaves the update path as a back door — a caller creates a compliant action, then updates it into the refused shape.

## Evidence

Spec (Smithy model): `Action$ExecutionRoleArn` doc "The role passed for action execution and reversion. Roles and actions must be in the same account."; `UpdateBudgetActionRequest$AccountId` (len 12..12, `^\d{12}$`) is the account the action lives in; `UpdateBudgetActionRequest$ExecutionRoleArn` carries only the `^arn:aws...:iam::\d{12}:role/...$` pattern; `UpdateBudgetActionRequest$Subscribers` is `@length(min: 1, max: 11)` and the `Notification` shape doc states the split that total is made of ("A notification can have one SNS subscriber and up to 10 email subscribers, for a total of 11 subscribers"); `UpdateBudgetAction` declares `InvalidParameterException` (HTTP 400, "An error on the client occurred. Typically, the cause is an invalid input value.").
Handler: services/service-com-amazonaws-budgets-awsbudgetservicegateway/src/ops/update_budget_action.rs:171-262 — the only validation is `crate::state::validate_action_definition(&old_action.action_type, definition)` at lines 214-224; `input.execution_role_arn` is assigned straight onto the stored action at lines 239-241 with no comparison against `input.account_id`, and `input.subscribers` at lines 245-247 with no per-type count. `crate::state::subscriber_limit_violation` (src/state.rs:785-807), which already implements the per-type rule and is called from create_notification.rs:182-188, is never called from this file.
Tests: tests/all/budget_action_history.rs:470-523 drives one update per accepted member and asserts HTTP 200 for each, but the `ExecutionRoleArn` patch uses `OTHER_EXECUTION_ROLE` = "arn:aws:iam::123456789012:role/budget-action-role-two" (same account as the request) and the `Subscribers` patch uses `subscribers(3)`, which alternates subscription types and therefore stays inside both per-type limits. No test supplies a foreign-account role or an oversized per-type subscriber set to `UpdateBudgetAction`, so nothing asserts a refusal.

## Steps to Reproduce

1. CreateBudget {"AccountId":"123456789012","Budget":{"BudgetName":"acted-on","TimeUnit":"MONTHLY","BudgetType":"COST","BudgetLimit":{"Amount":"100","Unit":"USD"}}} → 200.
2. CreateBudgetAction for that budget with a valid same-account ExecutionRoleArn (arn:aws:iam::123456789012:role/budget-action-role), ActionType APPLY_IAM_POLICY, a matching Definition.IamActionDefinition, ApprovalModel AUTOMATIC and one EMAIL subscriber → 200, note the ActionId.
3. POST / with X-Amz-Target: AWSBudgetServiceGateway.UpdateBudgetAction and body {"AccountId":"123456789012","BudgetName":"acted-on","ActionId":"<id>","ExecutionRoleArn":"arn:aws:iam::999999999999:role/BudgetActionRole"} — emulator answers 200 with OldAction/NewAction; AWS refuses the role as not being in the action's account.
4. DescribeBudgetAction for that ActionId — the foreign-account ExecutionRoleArn is reported, confirming it was persisted.
5. POST / with X-Amz-Target: AWSBudgetServiceGateway.UpdateBudgetAction and body {"AccountId":"123456789012","BudgetName":"acted-on","ActionId":"<id>","Subscribers":[{"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:123456789012:a"},{"SubscriptionType":"SNS","Address":"arn:aws:sns:us-east-1:123456789012:b"}]} — emulator answers 200 and DescribeBudgetAction reports both SNS subscribers; AWS refuses the set (one SNS subscriber per action).
6. Repeat step 5 with eleven distinct EMAIL subscribers (inside the modelled 1..11 total) — emulator answers 200; AWS refuses (at most ten email subscribers).
