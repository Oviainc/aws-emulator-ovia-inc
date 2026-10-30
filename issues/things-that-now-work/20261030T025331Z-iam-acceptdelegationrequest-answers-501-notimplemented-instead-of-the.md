Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.delegation-request-workflow-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.delegation-request-workflow-lifecycle|run#20|AssertionError|assert code_accept == "NoSuchEntity", (
Issue-Commit: 3cbee5fc05633571f18b42c799709a3e75b70289

# iam: AcceptDelegationRequest answers 501 NotImplemented instead of the modelled NoSuchEntity 404 'DelegationRequest not found: <id>' for an unknown delegation request id

> _Note: this issue was identified against the code base at commit `3cbee5fc05633571f18b42c799709a3e75b70289`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptDelegationRequest

## Description
One net-new root cause, grounded in the failing check iam.delegation-request-workflow-lifecycle. (The open issue 20261029T192356Z-iam-acceptdelegationrequest-acquirerole-associatedelegationrequest-answer-notimplemented-and-generatecredentialreport.md describes a related symptom on AcceptDelegationRequest/AcquireRole/AssociateDelegationRequest, but it carries only the iam.op-contract-* replay keys and does NOT carry this check's key, so it does not cover this failure.)

Cause 1 of 1: AcceptDelegationRequest is an unimplemented scaffold and refuses every call with an unmodelled error instead of resolving the DelegationRequestId against IAM state.
AWS: AcceptDelegationRequest is a served IAM operation whose input is DelegationRequestId (Smithy: delegationRequestIdType, @length min 16 max 128, pattern ^[\w-]+$, required) and whose modelled errors are exactly NoSuchEntityException (awsQueryError code NoSuchEntity, HTTP 404), ConcurrentModificationException (ConcurrentModification, 409) and ServiceFailureException (ServiceFailure, 500); it returns Unit on success. Given a syntactically valid DelegationRequestId that names no delegation request, the answer is NoSuchEntity at HTTP 404, with the message (recorded behavior, matching the emulator's own already-correct GetDelegationRequest answer for the same id) whose fixed text is "DelegationRequest not found: " followed by the variable part: the DelegationRequestId taken verbatim from the request. Note the two spellings AWS uses in this family: Get and Accept spell the entity "DelegationRequest", while Update/Reject/Associate/SendDelegationToken spell it "Delegation request" — the message is otherwise identical.
Emulator: the call answers error code NotImplemented at HTTP 501 with the message 'operation AcceptDelegationRequest is scaffolded but not implemented in this emulator'. NotImplemented is not an error the IAM model declares for this (or any) operation. The handler body is still the scaffold's Err(crate::scaffolded::Scaffolded::not_implemented("AcceptDelegationRequest")), while the sibling reads on the same entity (CreateDelegationRequest's out-of-range SessionDuration ValidationError, GetDelegationRequest, ListDelegationRequests and UpdateDelegationRequest) already answer correctly in the same run — so this is an isolated gap in one operation, not a missing feature of the whole delegation surface.
Why: a client that branches on NoSuchEntity (every AWS SDK's IAM error surface models it, and none models NotImplemented) sees an unknown, non-retryable 501 server-class error where AWS gives a 404 client error naming the missing request id. The delegation acceptance flow (accept, then SendDelegationToken) cannot be driven at all, and a caller cannot distinguish "this request id does not exist" from "IAM is broken".
Fix: the AcceptDelegationRequest handler — it must look the DelegationRequestId up in the service's delegation-request state and return the modelled NoSuchEntityException (NoSuchEntity, 404) with the "DelegationRequest not found: <DelegationRequestId>" message when nothing resolves, and the Unit success response (plus the ConcurrentModification path) otherwise, in place of the scaffold's 501.
Traces: iam.delegation-request-workflow-lifecycle (assertion at check line 83: expected NoSuchEntity for AcceptDelegationRequest, got NotImplemented). Handler under test: services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/accept_delegation_request.rs:178 (the SCAFFOLDED marker is on line 4).

## Evidence
Replay of iam.delegation-request-workflow-lifecycle:

  AssertionError: expected NoSuchEntity for AcceptDelegationRequest, got NotImplemented
  (check_iam_delegation_request_workflow_lifecycle.py, line 83, in run: assert code_accept == "NoSuchEntity")

Expected (AWS / Smithy com.amazonaws.iam#AcceptDelegationRequest): HTTP 404, Error.Code NoSuchEntity, Error.Message "DelegationRequest not found: <DelegationRequestId>" — fixed text "DelegationRequest not found: ", variable part the request's DelegationRequestId.
Actual (emulator): HTTP 501, Error.Code NotImplemented, Error.Message "operation AcceptDelegationRequest is scaffolded but not implemented in this emulator".

The same check's earlier beats PASSED against the emulator in the same run — CreateDelegationRequest returned ValidationError with the message "1 validation error detected: Value at 'sessionDuration' failed to satisfy constraint: Member must have value less than or equal to 43200", and GetDelegationRequest/UpdateDelegationRequest returned NoSuchEntity with the "DelegationRequest not found: <id>" / "Delegation request not found: <id>" messages — so only AcceptDelegationRequest diverges here.

## Steps to Reproduce
1. Point an AWS SDK/botocore IAM client at the emulator.
2. Build a syntactically valid but never-created delegation request id (matching ^[\w-]+$, 16..128 chars), e.g. "sim2real-behav-" + uuid4().hex.
3. Call iam:GetDelegationRequest(DelegationRequestId=<id>) and observe the emulator already answers NoSuchEntity / HTTP 404 with "DelegationRequest not found: <id>".
4. Call iam:AcceptDelegationRequest(DelegationRequestId=<id>) with the same id and read Error.Code and the HTTP status: the emulator answers NotImplemented / HTTP 501 instead of NoSuchEntity / HTTP 404 with the same not-found message.
5. Replay check iam.delegation-request-workflow-lifecycle to confirm.
