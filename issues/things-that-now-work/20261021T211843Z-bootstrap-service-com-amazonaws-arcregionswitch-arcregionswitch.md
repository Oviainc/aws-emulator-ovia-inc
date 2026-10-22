Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 34e6e58e29d15417fc6ff05c31f4f08593fa6d9a

# Bootstrap the ArcRegionSwitch emulator

_Filed: 2026-10-21T21:18:43Z_
_Operation: service-com-amazonaws-arcregionswitch-arcregionswitch | Class: missing_

> _Note: this issue was identified against the code base at commit `34e6e58e29d15417fc6ff05c31f4f08593fa6d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 22 operations in `src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state, so no client can do anything with this service. Implement a FIRST, SMALL stateful slice: the plan lifecycle plus one execution, wired to real persisted state.

Happy path to implement, in order (wire protocol `awsJson1_0`, `POST /` with `X-Amz-Target: ArcRegionSwitch.<OperationName>`):

1. **CreatePlan** — input `name` (required, 1..32, `^[a-zA-Z0-9](?:[a-zA-Z0-9-]{0,30}[a-zA-Z0-9])?$`), `regions` (required, exactly 2), `recoveryApproach` (required, `activeActive|activePassive`), `workflows` (required), `executionRole` (required, IAM role ARN pattern), optional `description`, `primaryRegion`, `recoveryTimeObjectiveMinutes`, `associatedAlarms`, `triggers`, `reportConfiguration`, `serviceQuotaChecksEnabled`, `tags`. Must mint a plan ARN matching `arn:aws:arc-region-switch::<12-digit-account>:plan/<name>:<6 lowercase alnum suffix>`, store the whole plan (including tags), set server-owned `owner` (account id), `version`, `updatedAt`, and return it in output `plan` built from the stored record — not from the request echo.
2. **GetPlan** — input `arn`; returns output `plan` read from the store, with every field supplied at CreatePlan/UpdatePlan preserved. Unknown ARN ⇒ `ResourceNotFoundException` (HTTP 404).
3. **ListPlans** — returns `plans` as `AbbreviatedPlan` entries (`arn`, `owner`, `name`, `regions`, `recoveryApproach`, `primaryRegion`, `version`, `updatedAt`, `description`, `executionRole`, `recoveryTimeObjectiveMinutes`, `activePlanExecution` when an execution is in progress) projected from the stored plans. Honour `maxResults` (1..100) and round-trip `nextToken` (1..2048) so paging works; a created plan must appear and a deleted plan must disappear.
4. **StartPlanExecution** — input `planArn`, `targetRegion`, `action` (`activate|deactivate|postRecovery`), `mode` (default `graceful`), optional `comment`, `latestVersion`, `recoveryExecutionId`, `clientToken` (idempotency token). Must create and persist a new execution record against the plan, returning `executionId`, `plan` (the plan ARN), `planVersion`, and `activateRegion`/`deactivateRegion` derived from the plan's two `regions` plus the requested `action`/`targetRegion`. Unknown `planArn` ⇒ `ResourceNotFoundException`; repeating the same `clientToken` must return the same `executionId` rather than creating a second execution; starting a second concurrent execution for the same plan ⇒ `ConflictException` (HTTP 409).
5. **GetPlanExecution** — input `planArn`, `executionId`, `maxResults` (default 100); returns the persisted execution: `planArn`, `executionId`, `startTime`, `mode`, `executionState` (one of the modeled `ExecutionState` values), `executionAction`, `executionRegion`, plus `version`, `comment`, `updatedAt`, `stepStates`, `plan`. Must reflect the values StartPlanExecution recorded. Unknown execution ⇒ `ResourceNotFoundException`.
6. **DeletePlan** — input `arn`; `@idempotent`. Removes the plan (and its tags/executions) from the store; a following GetPlan/ListPlans must show it gone with `ResourceNotFoundException` / absence. Deleting a plan with an in-progress execution ⇒ `IllegalStateException` (HTTP 400).

Every one of these six must compute its response from persisted state held in the shared engine via `crate::state::service_state(&*self.engine)`. No canned responses, no echoing the input back, no minting an id that is never stored. All other operations (ListPlansInRegion, GetPlanInRegion, UpdatePlan, Update/Cancel/ApprovePlanExecution*, ListPlanExecutions, ListPlanExecutionEvents, GetPlanEvaluationStatus, ListRoute53HealthChecks*, ListServiceQuotaWarnings, Tag/Untag/ListTagsForResource) stay at 501 for this slice and are tracked as separate gap issues.

## State Model

In `ComAmazonawsArcregionswitchArcregionswitchState` (src/state.rs), behind the engine lock:

- `plans: BTreeMap<String /* plan ARN */, StoredPlan>` — insertion/name-ordered so ListPlans paging is stable.
- `StoredPlan { arn, name, owner /* 12-digit account */, regions: Vec<String> /* exactly 2 */, recovery_approach, primary_region: Option<String>, description: Option<String>, execution_role, recovery_time_objective_minutes: Option<i32>, workflows: Vec<Workflow>, associated_alarms: BTreeMap<String, AssociatedAlarm>, triggers: Vec<Trigger>, report_configuration: Option<ReportConfiguration>, service_quota_checks_enabled: Option<bool>, version: String, updated_at: Timestamp, tags: BTreeMap<String, String>, executions: Vec<String> /* execution ids, newest last */, active_execution: Option<String> }`
- `executions: BTreeMap<(String /* plan ARN */, String /* executionId */), StoredExecution>`
- `StoredExecution { plan_arn, execution_id, plan_version, comment: Option<String>, mode /* graceful|ungraceful */, action /* activate|deactivate|postRecovery */, execution_state /* pending → inProgress → completed */, execution_region, activate_region, deactivate_region, recovery_execution_id: Option<String>, start_time, end_time: Option<Timestamp>, updated_at, step_states: Vec<StepState>, events: Vec<ExecutionEvent> }`
- `idempotency: BTreeMap<String /* clientToken */, String /* executionId */>` so a replayed StartPlanExecution returns the first execution.
- A monotonic counter/seed used to mint the 6-character plan ARN suffix and execution ids so they are unique and stable per record.

## Evidence

`services/service-com-amazonaws-arcregionswitch-arcregionswitch/OPERATIONS.md` lists all 22 operations with the input/output members and errors cited above (CreatePlan → output `plan`; GetPlan/`arn` → `ResourceNotFoundException`; ListPlans @paginated(items=plans, inputToken/outputToken=nextToken); StartPlanExecution → `executionId`/`plan`/`planVersion`/`activateRegion`/`deactivateRegion` with ConflictException/IllegalStateException; GetPlanExecution required outputs `startTime`/`mode`/`executionState`/`executionAction`/`executionRegion`; DeletePlan @idempotent with IllegalStateException). Smithy shapes `Plan`, `AbbreviatedPlan` and `AbbreviatedExecution` supply the server-owned fields `owner`, `version`, `updatedAt`, `activePlanExecution`. `grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 22 handler files, including `create_plan.rs`, `get_plan.rs`, `list_plans.rs`, `start_plan_execution.rs`, `get_plan_execution.rs`, `delete_plan.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and no handler touches state. No tests assert any of this behaviour.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI client for `arc-region-switch` at it.
2. CreatePlan with name=`demo-plan`, regions=[`us-east-1`,`us-west-2`], recoveryApproach=`activePassive`, primaryRegion=`us-east-1`, executionRole=`arn:aws:iam::123456789012:role/ArcRegionSwitchRole`, one workflow, tags={`env`:`test`} — expect 200 with `plan.arn` matching the plan ARN pattern, plus `plan.owner`, `plan.version`, `plan.updatedAt` populated (today: HTTP 501).
3. GetPlan with that arn — expect the exact plan just created (name, both regions, recoveryApproach, executionRole, workflow, description) read back from state.
4. ListPlans — expect the new plan present as an AbbreviatedPlan; repeat with maxResults=1 and follow nextToken to confirm paging terminates without duplicates.
5. StartPlanExecution with planArn, targetRegion=`us-west-2`, action=`activate`, mode=`graceful`, clientToken=`tok-1` — expect an `executionId` plus `activateRegion`=`us-west-2` / `deactivateRegion`=`us-east-1`; repeat the identical call with clientToken=`tok-1` and expect the SAME executionId.
6. GetPlanExecution with planArn + that executionId — expect the persisted execution (mode=`graceful`, executionAction=`activate`, executionRegion/`startTime` set, executionState a modeled ExecutionState value).
7. GetPlan with a never-created ARN — expect `ResourceNotFoundException` (HTTP 404), not 501.
8. DeletePlan with the arn, then GetPlan — expect 200/empty then `ResourceNotFoundException`, and ListPlans no longer returns the plan.
