Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected iam.op-contract-create-policy-version iam.op-contract-deletePolicyVersion iam.policy-version-delete-default-rejected iam.policy-version-limit iam.policy-version-limit-exceeded iam.policy-versions-create-and-list iam.set-default-policy-version-invalid-version-rejected iam.set-default-policy-version-lifecycle
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam|CreatePolicyVersion|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Implement iam CreatePolicyVersion: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyVersion

## Description
`CreatePolicyVersion` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy_version.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

9 recorded unit(s) fail on their `CreatePolicyVersion` call, so each one unblocks only when `CreatePolicyVersion` works: `iam.iam-bound-deep-iam-3-managed-policy-version-over-6144-significant-chars-rejected`, `iam.op-contract-create-policy-version`, `iam.op-contract-deletePolicyVersion`, `iam.policy-version-delete-default-rejected`, `iam.policy-version-limit`, `iam.policy-version-limit-exceeded`, `iam.policy-versions-create-and-list`, `iam.set-default-policy-version-invalid-version-rejected`, `iam.set-default-policy-version-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_policy_version.rs`. Replaying the corpus, the emulator refused the `CreatePolicyVersion` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreatePolicyVersion operation: operation CreatePolicyVersion is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call CreatePolicyVersion and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
