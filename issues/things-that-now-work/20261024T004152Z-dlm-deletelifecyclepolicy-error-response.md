Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A PolicyId outside its modeled pattern is answered with the unmodeled ValidationException instead of a DLM error

_Filed: 2026-10-24T00:41:52Z_
_Operation: DeleteLifecyclePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteLifecyclePolicy` models only `InternalServerException` (500), `LimitExceededException` (429) and `ResourceNotFoundException` (404) — no 400-class error — so a delete that names a policy id the service cannot resolve, including one outside `PolicyId`'s `^policy-[a-f0-9]+$` / length 0..64, is answered with the modeled `ResourceNotFoundException` (HTTP 404, `Code` `ResourceNotFound`, `ResourceType` `LIFECYCLE_POLICY`, `ResourceIds`).

Expected: `DELETE /policies/not-a-policy-id` answers 404 `ResourceNotFoundException` in the DLM error shape.

Actual: the constraint refusal is rendered by the framework's pre-dispatch envelope as HTTP 400 with error type `ValidationException`, a shape the DLM model does not declare, and the prose in a lowercase `message` key. A client that treats "delete anything, tolerate not-found" as idempotent cleanup sees an unmodeled error type and a status the operation cannot produce on the real service.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteLifecyclePolicy")` — errors are `InternalServerException` (500), `LimitExceededException` (429), `ResourceNotFoundException` (404); the model declares no `ValidationException`. `DeleteLifecyclePolicyRequest$PolicyId` is `@httpLabel` with `@pattern(^policy-[a-f0-9]+$)`.
Handler/service: `src/service.rs` `EmulatedService::new` wraps only the operations that model `InvalidRequestException` and excludes `DeleteLifecyclePolicy` by name ("declare no client error, so they are not here"); the module doc describes the resulting envelope as `ValidationException` with a lowercase `message`. `src/ops/delete_lifecycle_policy.rs` `handle` answers `ResourceNotFoundException` for a well-formed unknown id, so the divergence is confined to ids the constraint walk rejects before the handler runs.
Tests: `tests/all/error_paths.rs::a_policy_that_does_not_exist_is_a_resource_not_found_exception` covers only well-formed absent/deleted ids for the DELETE; no test sends a malformed `PolicyId`.

## Steps to Reproduce

1. DELETE /policies/not-a-policy-id (SigV4-signed for dlm)
2. Observe HTTP 400 with x-amzn-errortype ValidationException and a lowercase `message` body key
3. Expected: HTTP 404 ResourceNotFoundException with Message/Code/ResourceType/ResourceIds, the only client error this operation models
