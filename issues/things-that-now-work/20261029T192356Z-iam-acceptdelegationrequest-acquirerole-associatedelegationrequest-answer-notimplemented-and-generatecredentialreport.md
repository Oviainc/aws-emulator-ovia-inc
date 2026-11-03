Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-acceptDelegationRequest iam.op-contract-acquireRole iam.op-contract-associateDelegationRequest iam.op-contract-generateCredentialReport
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-acceptDelegationRequest|run#4|AssertionError|assert code == "NoSuchEntity", f"expected NoSuchEntity, got {code}"
Issue-Key: iam.op-contract-acquireRole|run#7|AssertionError|assert code == "NoSuchEntity", f"expected NoSuchEntity, got {code}"
Issue-Key: iam.op-contract-associateDelegationRequest|run#6|AssertionError|assert code == "NoSuchEntity", f"expected NoSuchEntity, got {code}"
Issue-Key: iam.op-contract-generateCredentialReport|run#3|AssertionError|assert gen_code == "LimitExceeded", f"unexpected error code: {gen_code!r}"
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# iam: AcceptDelegationRequest / AcquireRole / AssociateDelegationRequest answer NotImplemented, and GenerateCredentialReport is not routed at all (InvalidAction 'unknown action')

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcquireRole

## Description
Two distinct net-new root causes. Each must be closed before this issue is submitted. (The open issue 20261025T000319Z-iam-10-net-new-behavioral-root-causes-four.md describes a related credential-report symptom and owns other checks; it does not carry the replay keys below, so it does not cover them.)

Cause 1 of 2: AcceptDelegationRequest, AcquireRole and AssociateDelegationRequest refuse every call with NotImplemented instead of the operation's modelled refusal.
AWS: each of these operations is served; given an identifier that names no entity, the refusal is NoSuchEntity (HTTP 404), the standard IAM not-found classification (Smithy: NoSuchEntityException, awsQueryError code NoSuchEntity, HTTP 404).
Emulator: each answers code NotImplemented — a code the IAM model does not declare for any operation — so the caller receives an unmodelled error instead of a modelled one.
Why: a client branching on NoSuchEntity (or on any modelled IAM error) sees an error code no AWS SDK's IAM error surface knows, and the delegation/role-acquisition flows cannot be exercised at all.
Fix: the handlers for these three operations — they must resolve the identifier against IAM's state and return the modelled NoSuchEntity refusal (and the success path) rather than the scaffold's NotImplemented.
Traces: iam.op-contract-acceptDelegationRequest, iam.op-contract-acquireRole, iam.op-contract-associateDelegationRequest

Cause 2 of 2: GenerateCredentialReport is not routed — the Query dispatcher answers InvalidAction 'unknown action: GenerateCredentialReport'.
AWS: GenerateCredentialReport is a bound IAM operation; it answers 200 with State/Description, or refuses with LimitExceeded (HTTP 409) when a report was generated too recently. Smithy: GenerateCredentialReport takes Unit input, returns State (ReportStateType) + Description, and models LimitExceededException (LimitExceeded, 409) and ServiceFailureException (ServiceFailure, 500).
Emulator: the action is not recognised by the service's Query action dispatch, so the caller gets InvalidAction with 'unknown action: GenerateCredentialReport' — the response of a service that does not have the operation at all, not of one that refused it.
Why: a client cannot generate or poll a credential report, and the error it gets (InvalidAction) tells it the API does not exist rather than naming a modelled condition.
Fix: the service's Query action routing table plus the generate_credential_report handler.
Traces: iam.op-contract-generateCredentialReport

## Evidence
Cause 1 — `AssertionError: expected NoSuchEntity, got NotImplemented` for all three of op-contract-acceptDelegationRequest, op-contract-acquireRole and op-contract-associateDelegationRequest.
Cause 2 — `AssertionError: unexpected error code: 'InvalidAction'` where the check expected LimitExceeded (or success); the underlying response is `An error occurred (InvalidAction) when calling the GenerateCredentialReport operation: unknown action: GenerateCredentialReport`.

## Steps to Reproduce
(1) Call iam:AcceptDelegationRequest, iam:AcquireRole and iam:AssociateDelegationRequest with an identifier that names no entity and read the error code. (2) Call iam:GenerateCredentialReport and read the error code.
