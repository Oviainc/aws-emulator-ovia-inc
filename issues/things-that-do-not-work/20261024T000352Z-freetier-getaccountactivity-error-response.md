Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetAccountActivity answers an unknown activityId with HTTP 404 while this service answers ResourceNotFoundException with HTTP 400

_Filed: 2026-10-24T00:03:52Z_
_Operation: GetAccountActivity | Class: error-response_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For a well-formed `activityId` that names no activity, the handler raises the modelled `ResourceNotFoundException` and lets the framework use the modelled `@httpError 404`. The recorded wire behaviour of this service answers `ResourceNotFoundException` with HTTP 400 (body unchanged: `__type: ResourceNotFoundException` plus `message`), which is why the sibling handler `get_account_plan_state.rs` deliberately restates the status to 400 via `Scaffolded::restated`. The same error shape in the same service should not change status per operation.

Expected: HTTP 400 with an awsJson error body whose `__type` is `ResourceNotFoundException`.

Actual: HTTP 404 with the same body, so a client that branches on status (or an SDK that maps 404 differently) sees a different outcome for the activity-detail not-found path than for the plan-state not-found path.

## Evidence

Model: GetAccountActivity declares ResourceNotFoundException (client error, @httpError 404). Recorded behaviour: the already-fixed issue 20261022T062051Z ("freetier responses use awsJson1_0 content-type and skip ResourceNotFoundException") states for this same service "the model annotates @httpError 404, but the recording shows the service answering 400 for this condition — match the recording", and the emulator acted on that only in get_account_plan_state.rs lines 192-209 (RESOURCE_NOT_FOUND_STATUS = 400, `Scaffolded::restated`). Handler: src/ops/get_account_activity.rs lines 201-212 returns the plain modelled error, so the framework answers 404. Tests: tests/all/free_tier.rs `unknown_and_malformed_activity_ids_are_told_apart` pins the current 404, i.e. the test encodes the modelled status rather than the recorded one.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. POST / with X-Amz-Target: AWSFreeTierService.GetAccountActivity and body {"activityId": "00000000000000000000000000000000"} (32 alphanumerics naming no seeded activity).
3. Observe HTTP 404 with __type ResourceNotFoundException; the service's recorded status for ResourceNotFoundException is 400 (as already applied to GetAccountPlanState).
