Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# StartCapacityTask with DryRun=true answers 501 instead of a 200 dry-run result

_Filed: 2026-10-24T05:23:31Z_
_Operation: StartCapacityTask | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler is implemented, but it special-cases the modeled `DryRun` input: when `DryRun` is true it returns the scaffold's `Scaffolded::not_implemented("StartCapacityTask with DryRun set")`, i.e. HTTP 501 `NotImplemented`.

Expected (real AWS): a dry run is a normal, successful call. AWS evaluates the requested instance pools against the Outpost's available capacity, persists nothing, and answers HTTP 200 with a `StartCapacityTaskOutput` body — `DryRun: true` echoed back, the requested pools, `InstancesToExclude`, `TaskActionOnBlockingInstances` and a `CapacityTaskStatus` describing the verdict. `DryRun` is a first-class member of BOTH the input (with `@default: false`) and the output shape, which is only meaningful if a dry run returns a normal response.

Actual: any caller that sets `DryRun: true` — the standard way an SDK/CLI user validates a capacity plan before committing — gets a 501 with the `NotImplemented` error code, which is not one of the operation's modeled errors and is not a shape any client branch handles.

At minimum the emulator should answer 200 from a non-persisted projection of the request (echoing `DryRun: true` and the requested pools) rather than refusing the call.

## Evidence

Spec: Smithy model `StartCapacityTask` input member `DryRun` (`target: DryRun`, `default: false`) and output member `DryRun` plus `CapacityTaskStatus`, `RequestedInstancePools`, `InstancesToExclude`, `TaskActionOnBlockingInstances`; the operation's modeled errors are AccessDenied/Conflict/InternalServer/NotFound/Validation only — there is no 501 in the contract. Handler: `services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/start_capacity_task.rs` lines ~210-219 (`if input.dry_run { return Err(crate::scaffolded::Scaffolded::not_implemented(...)) }`), with the rationale in the module header lines 14-20. Test coverage: `tests/all/capacity_task_lifecycle.rs:684-692` asserts the 501/`NotImplemented` outcome for "StartCapacityTask as a dry run", so the divergence is currently locked in by a test rather than caught by one.

## Steps to Reproduce

1. Boot the outposts emulator
2. CreateSite, then CreateOutpost against that site to get an `op-…` id
3. POST /outposts/{OutpostIdentifier}/capacity with body {"InstancePools":[{"InstanceType":"m5.4xlarge","Count":1}],"DryRun":true}
4. Observe HTTP 501 with error code NotImplemented; real AWS answers HTTP 200 with a StartCapacityTask response echoing DryRun=true and persisting no task
