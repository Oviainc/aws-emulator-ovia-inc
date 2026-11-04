Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: nova-act.concurrent-update-workflow-run-terminal-status-race
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act.concurrent-update-workflow-run-terminal-status-race|run#13|AssertionError|assert len(successes) == 1, f"expected exactly one racer to win the terminal transition, got {results}"
Issue-Commit: e3d5599d128a91029caef4169f509524de4e0ad7

# nova-act UpdateWorkflowRun answers 501 NotImplemented, so no terminal transition is enforced and racing callers all get the same non-modelled refusal

> _Note: this issue was identified against the code base at commit `e3d5599d128a91029caef4169f509524de4e0ad7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkflowRun

## Description
One net-new root cause. 1 cause must be closed.

Cause 1 of 1: UpdateWorkflowRun never applies a terminal status and never arbitrates concurrent terminal transitions on one workflow run; every caller — including every racer — is refused with the scaffold's 501 `NotImplemented` instead of one winner succeeding and the rest being refused with a modelled error. Failing check: nova-act.concurrent-update-workflow-run-terminal-status-race.

AWS: Smithy UpdateWorkflowRun — HTTP PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}, success code 200, `@idempotent`; input requires `workflowDefinitionName` (WorkflowDefinitionName, @length 1..40, @pattern ^[a-zA-Z0-9_-]{1,40}$, @httpLabel), `workflowRunId` (UuidString, @pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$, @httpLabel) and `status` (WorkflowRunStatus enum: RUNNING | SUCCEEDED | FAILED | TIMED_OUT | DELETING, documented as "The new status to set for the workflow run"); output UpdateWorkflowRunResponse is empty. Modelled errors: AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400 — 501/NotImplemented is NOT in that set and is not a modelled error of this service. The model does not state the state machine's edges in prose, so: recorded behavior only — the suite's recorded AWS behavior is that of N concurrent UpdateWorkflowRun calls moving one RUNNING run to a terminal status (SUCCEEDED/FAILED/TIMED_OUT), exactly ONE returns 200 and the others are refused with ConflictException (409) or ValidationException (400), and a subsequent GetWorkflowRun reports the WINNER's status. ConflictException's modelled body carries three required members — `message`, `resourceId` and `resourceType` — so the refusal's message is the fixed text AWS uses for a conflicting state transition with the run id supplied as `resourceId` (the UuidString from the request URI) and the resource type as `resourceType`; the run id value itself is per-run and is not part of the requirement. Hypothesis: once a run holds a terminal status, a further transition to a different terminal status is the conflicting case.

Emulator: every UpdateWorkflowRun call returns HTTP 501 with error code `NotImplemented` — the handler is still the generated scaffold whose `handle` body is `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateWorkflowRun"))`. In the replay all six racers came back `NotImplemented`, so no racer won, no modelled refusal was produced for the losers, and the run's status is never advanced past the one it was created with.

Why: a client cannot complete a workflow run at all. 501/NotImplemented is outside the operation's modelled error set, so botocore surfaces it as an unmodelled ClientError that no retry/branching policy written against AWS handles; a client that races two terminal transitions (the normal case when a worker and a timeout watchdog both finish) gets no winner and no ConflictException to tell it which side won, and any subsequent GetWorkflowRun keeps reporting the pre-terminal status, so orchestration built on nova-act never observes completion. Note this is the write side of the same lifecycle whose read side is filed as 20261104T064305Z-nova-act-returns-static-placeholder-state-instead-of.md (GetWorkflowRun serving a constant status and omitting endedAt); that issue does not cover UpdateWorkflowRun. The suite also filed, for a different check, an issue naming the unimplemented `UpdateWorkflowRun` stub itself; THIS issue adds the behavior that implementation must have: single-winner terminal arbitration plus the modelled refusal for the losers.

Fix: the UpdateWorkflowRun handler and the workflow-run record in the nova-act service crate's state — validate the requested WorkflowRunStatus against the stored run's current status, apply the transition atomically so exactly one concurrent terminal transition commits (and record the terminal status so GetWorkflowRun reflects the winner), return the modelled empty 200 to the winner, and refuse the losers with ConflictException (409) carrying message/resourceId/resourceType, or ValidationException (400) for a transition the state machine forbids. Replace the 501 scaffold refusal and drop the SCAFFOLDED marker line.

Traces: nova-act.concurrent-update-workflow-run-terminal-status-race (file and line citations: services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/update_workflow_run.rs lines 4-6 SCAFFOLDED marker and line 166 `Scaffolded::not_implemented("UpdateWorkflowRun")`).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against the booted emulator: 7/22 passed; nova-act 7/22 (32%), 15 failed.

[mismatch] nova-act.concurrent-update-workflow-run-terminal-status-race:
  check_nova_act_concurrent_update_workflow_run_terminal_status_race.py line 55:
  AssertionError: expected exactly one racer to win the terminal transition, got ['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented']

The check creates a workflow definition, creates one workflow run (RUNNING), then fires six concurrent UpdateWorkflowRun calls with statuses SUCCEEDED/FAILED/TIMED_OUT/SUCCEEDED/FAILED/TIMED_OUT.
Expected: exactly one 'ok' (HTTP 200, empty UpdateWorkflowRunResponse); the other five refused with ConflictException (409) or ValidationException (400); GetWorkflowRun then reports the winner's status.
Actual: all six calls refused with HTTP 501, error code 'NotImplemented' — a code outside UpdateWorkflowRun's modelled error set (AccessDenied/Conflict/InternalServer/ResourceNotFound/Throttling/Validation).

Model references used: UpdateWorkflowRun (PUT /workflow-definitions/{workflowDefinitionName}/workflow-runs/{workflowRunId}, code 200, @idempotent; input status -> WorkflowRunStatus required; errors ConflictException 409, ValidationException 400, ResourceNotFoundException 404), WorkflowRunStatus (RUNNING|SUCCEEDED|FAILED|TIMED_OUT|DELETING), ConflictException (required members message, resourceId, resourceType; httpError 409).

Scaffold evidence in the checkout: services/service-com-amazonaws-novaact-amazonnovaagentsdataplane/src/ops/update_workflow_run.rs:166 returns Scaffolded::not_implemented("UpdateWorkflowRun").

## Steps to Reproduce
1. git_clone the project and run the behavioral suite (simparity-behavioral-aws-v1) against the booted emulator.
2. Run check nova-act.concurrent-update-workflow-run-terminal-status-race: CreateWorkflowDefinition(name), ListModels to pick a GA model alias, CreateWorkflowRun -> workflowRunId (status RUNNING).
3. Fire six concurrent UpdateWorkflowRun(workflowDefinitionName, workflowRunId, status) calls with SUCCEEDED, FAILED, TIMED_OUT, SUCCEEDED, FAILED, TIMED_OUT.
4. Observe all six return HTTP 501 / 'NotImplemented' instead of exactly one HTTP 200 and five ConflictException/ValidationException refusals.
5. Call GetWorkflowRun(workflowDefinitionName, workflowRunId) and observe the status was never advanced to the winner's terminal status.
6. Re-replay nova-act.concurrent-update-workflow-run-terminal-status-race after the fix; it must pass.
