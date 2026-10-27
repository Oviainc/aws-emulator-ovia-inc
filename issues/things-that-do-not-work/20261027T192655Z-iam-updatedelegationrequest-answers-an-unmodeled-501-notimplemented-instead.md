Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.delegation-request-workflow-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.delegation-request-workflow-lifecycle|run#17|AssertionError|assert code_update == "NoSuchEntity", (
Issue-Commit: 74cfda6b1b1701e11661fc7b5d0b64a8c57e063b

# iam: UpdateDelegationRequest answers an unmodeled 501 NotImplemented instead of 404 NoSuchEntity for an unknown DelegationRequestId

> _Note: this issue was identified against the code base at commit `74cfda6b1b1701e11661fc7b5d0b64a8c57e063b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDelegationRequest

## Description
Cause 1 of 1: UpdateDelegationRequest never reaches the delegation-request lookup — it refuses every well-formed call with the scaffold's 501 `NotImplemented`, so the modeled NoSuchEntity refusal for an unknown request id is unreachable. Failing check id: iam.delegation-request-workflow-lifecycle.

AWS: UpdateDelegationRequest takes `DelegationRequestId` (required, delegationRequestIdType, @length 16..128, @pattern ^[\w-]+$) and optional `Notes` (notesType, @length 0..500). Smithy: UpdateDelegationRequest models exactly four errors — ConcurrentModificationException (409, awsQueryError ConcurrentModification), InvalidInputException (400, InvalidInput), NoSuchEntityException (404, NoSuchEntity), ServiceFailureException (500, ServiceFailure). NoSuchEntityException is documented as "The request was rejected because it referenced a resource entity that does not exist. The error message describes the resource." When the supplied id satisfies every modeled constraint but names no delegation request in the account, AWS therefore refuses with HTTP 404, error code `NoSuchEntity`. Recorded behavior for the message: the fixed text is `Delegation request not found: ` followed by the `DelegationRequestId` value taken verbatim from the request (the sibling ops GetDelegationRequest and AcceptDelegationRequest spell the entity compactly, `DelegationRequest not found: <DelegationRequestId>`; Update/Reject/Associate/SendDelegationToken spell it spaced, `Delegation request not found: <DelegationRequestId>`). Nothing in the id itself is part of the template — it is whatever the caller sent. `NotImplemented` is NOT a modeled error of this operation at all, and 501 is not a status this operation can return.

Emulator: the call is refused with error code `NotImplemented` and message "operation UpdateDelegationRequest is scaffolded but not implemented in this emulator" at HTTP 501. The replay asserted the code and got `AssertionError: expected NoSuchEntity for UpdateDelegationRequest, got NotImplemented`. This is specific to UpdateDelegationRequest and not a blanket gap in the feature: in the same check CreateDelegationRequest already refuses an out-of-range SessionDuration with `ValidationError` and the exact AWS message, GetDelegationRequest already refuses the same fabricated id with `NoSuchEntity` and the compact message, and ListDelegationRequests already returns a `DelegationRequests` list — so the delegation-request store exists and is queryable; only this operation's handler is still the generated stub. (The open issue 20261027T161925Z-iam-createdelegationrequest-stub.md names CreateDelegationRequest only and does not cover UpdateDelegationRequest.)

Why: a client cannot distinguish "this delegation request does not exist" from "this emulator cannot do it". Error code `NotImplemented` is outside the operation's modeled error union, so a typed SDK deserializes it as an unmodeled/unknown error rather than NoSuchEntityException, and the 501 status puts it in the server-fault/retryable class instead of the 404 client-fault class. Delegation-request workflows that branch on NoSuchEntity (treat a missing request as terminal, retry a 5xx) take the wrong branch: they retry a permanent failure, surface a server outage to the operator, and never run their not-found cleanup path. It also masks the rest of the lifecycle: because the check stops here, the identical refusal contract for AcceptDelegationRequest, RejectDelegationRequest, AssociateDelegationRequest, SendDelegationToken and AcquireRole is never exercised.

Fix: the UpdateDelegationRequest operation handler — resolve `DelegationRequestId` against the service's delegation-request state and, when it resolves to nothing, return the modeled `NoSuchEntityException` (HTTP 404, code `NoSuchEntity`) with the spaced message template above, instead of the scaffold's `not_implemented` refusal; the mutation must also leave no delegation request behind under the rejected id, since the check re-reads it with GetDelegationRequest afterwards and that read must still be NoSuchEntity.

Traces: iam.delegation-request-workflow-lifecycle — fails at the UpdateDelegationRequest assertion after Create/Get/List have already matched AWS. Handler file: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/update_delegation_request.rs (the `SCAFFOLDED, NOT IMPLEMENTED` marker is still on line 4; `handle` returns `Err(crate::scaffolded::Scaffolded::not_implemented("UpdateDelegationRequest"))` at line 182).

## Evidence
Check iam.delegation-request-workflow-lifecycle, verdict mismatch.

Expected (AWS): UpdateDelegationRequest with a well-formed but never-created DelegationRequestId -> HTTP 404, Error.Code = "NoSuchEntity", Error.Message = "Delegation request not found: <DelegationRequestId>" where the leading text is fixed and <DelegationRequestId> is echoed from the request. Smithy: UpdateDelegationRequest errors = {ConcurrentModificationException 409, InvalidInputException 400, NoSuchEntityException 404 (awsQueryError NoSuchEntity), ServiceFailureException 500}; `NotImplemented` is not among them.

Actual (emulator): HTTP 501, Error.Code = "NotImplemented", Error.Message = "operation UpdateDelegationRequest is scaffolded but not implemented in this emulator".

Replay output:
  File "check_iam_delegation_request_workflow_lifecycle.py", line 75, in run
    assert code_update == "NoSuchEntity", (
  AssertionError: expected NoSuchEntity for UpdateDelegationRequest, got NotImplemented

Contrast within the same check (these steps passed, so the gap is this one operation, not the feature):
  - CreateDelegationRequest(SessionDuration=999999) -> ValidationError, "1 validation error detected: Value at 'sessionDuration' failed to satisfy constraint: Member must have value less than or equal to 43200"
  - GetDelegationRequest(<fake id>) -> NoSuchEntity, "DelegationRequest not found: <fake id>"
  - ListDelegationRequests() -> response contains "DelegationRequests"

Source: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/update_delegation_request.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 182 (`Err(crate::scaffolded::Scaffolded::not_implemented("UpdateDelegationRequest"))`).

## Steps to Reproduce
1. Boot the emulator and point an IAM client at it.
2. Build a syntactically valid DelegationRequestId that was never created, e.g. `sim2real-behav-<32 hex chars>` (satisfies @length 16..128 and @pattern ^[\w-]+$).
3. Confirm the id really is unknown and that the service answers correctly on the read path:
     iam.get_delegation_request(DelegationRequestId=<id>)
   -> observe HTTP 404 / NoSuchEntity / "DelegationRequest not found: <id>".
4. Call the update path with the same id:
     iam.update_delegation_request(DelegationRequestId=<id>, Notes="sim2real-behav")
5. Observe HTTP 501 with Error.Code "NotImplemented" instead of HTTP 404 with Error.Code "NoSuchEntity" and message "Delegation request not found: <id>".
6. Re-replay the behavioral check to confirm a fix: iam.delegation-request-workflow-lifecycle.
