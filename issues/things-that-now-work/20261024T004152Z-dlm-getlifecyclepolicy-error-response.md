Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a1b03c4f6e81626c353b3ff836c908b12c596c5c

# A PolicyId outside its modeled pattern is answered with the unmodeled ValidationException instead of a DLM error

_Filed: 2026-10-24T00:41:52Z_
_Operation: GetLifecyclePolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `a1b03c4f6e81626c353b3ff836c908b12c596c5c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetLifecyclePolicy` models exactly three errors — `InternalServerException` (500), `LimitExceededException` (429) and `ResourceNotFoundException` (404) — and no 400-class error at all, so the real service's answer to a request that names a policy id it cannot resolve (including one outside `PolicyId`'s `^policy-[a-f0-9]+$` / length 0..64) is the modeled `ResourceNotFoundException`: HTTP 404 with `Code`, `ResourceType` (`LIFECYCLE_POLICY`) and `ResourceIds`.

Expected: `GET /policies/not-a-policy-id` answers one of the operation's modeled errors — 404 `ResourceNotFoundException` with the DLM error payload (`Message`, `Code`, `ResourceType`, `ResourceIds`).

Actual: the request never reaches the handler. Because this operation is deliberately not wrapped in the service's modeled-error envelope, the constraint refusal is rendered by the framework's pre-dispatch envelope: HTTP 400 with error type `ValidationException` — a shape `com.amazonaws.dlm` does not declare — and the prose in a lowercase `message` key the DLM error shapes do not have. An SDK therefore surfaces an unmodeled error type for a policy id a client could plausibly pass (a user-typed id, an id from another service, an id with uppercase hex), instead of the not-found error every DLM client branches on.

## Evidence

Spec: `smithy_query(query="operation", operation="GetLifecyclePolicy")` lists only `InternalServerException` (500), `LimitExceededException` (429), `ResourceNotFoundException` (404) — no 400 error is modeled for this operation, and the model declares no `ValidationException` shape anywhere; `GetLifecyclePolicyRequest$PolicyId` is `@httpLabel` with `@pattern(^policy-[a-f0-9]+$)`.
Handler/service: `src/service.rs` `EmulatedService::new` wraps only the six operations that model `InvalidRequestException` in `InModelledEnvelope` and states in its own comment "`GetLifecyclePolicy` and `DeleteLifecyclePolicy` declare no client error, so they are not here"; the module doc (lines 61-66) spells out the consequence — those two "are left to that envelope", which "would answer the right status under the error type `ValidationException`, a shape this service's model does not declare, with the prose in a lowercase `message` key the modelled shape does not have". `src/ops/get_lifecycle_policy.rs` `handle` only ever answers `ResourceNotFoundException`, so nothing downstream rewords the refusal.
Tests: `tests/all/error_paths.rs::a_policy_that_does_not_exist_is_a_resource_not_found_exception` exercises only WELL-FORMED absent ids (`policy-0000000000000001` and a deleted id); no test sends a `PolicyId` outside the pattern to `GET /policies/{PolicyId}`, so the current answer is unasserted.

## Steps to Reproduce

1. GET /policies/not-a-policy-id (SigV4-signed for dlm)
2. Observe HTTP 400 with x-amzn-errortype ValidationException and a body carrying a lowercase `message` key
3. Expected: HTTP 404 ResourceNotFoundException with Message/Code (ResourceNotFound)/ResourceType (LIFECYCLE_POLICY)/ResourceIds, the only client error this operation models
4. Compare with GET /policies/policy-0000000000000001, which does answer the modeled 404 — the two id forms must not produce different error shapes
