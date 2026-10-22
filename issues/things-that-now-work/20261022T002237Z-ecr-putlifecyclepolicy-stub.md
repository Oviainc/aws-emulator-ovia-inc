Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected ecr.ecracc-tagged-lifecycle-rule-without-a-tag-filter-rejected ecr.lifecycle-policy-invalid-json-rejected ecr.lifecycle-policy-round-trip ecr.op-contract-deleteLifecyclePolicy ecr.op-contract-getLifecyclePolicy ecr.op-contract-getLifecyclePolicyPreview ecr.op-contract-put-lifecycle-policy
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|PutLifecyclePolicy|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr PutLifecyclePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecyclePolicy

## Description
`PutLifecyclePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/put_lifecycle_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `PutLifecyclePolicy` call, so each one unblocks only when `PutLifecyclePolicy` works: `ecr.ecracc-lifecycle-policy-with-a-zero-image-count-rejected`, `ecr.ecracc-tagged-lifecycle-rule-without-a-tag-filter-rejected`, `ecr.lifecycle-policy-invalid-json-rejected`, `ecr.lifecycle-policy-round-trip`, `ecr.op-contract-deleteLifecyclePolicy`, `ecr.op-contract-getLifecyclePolicy`, `ecr.op-contract-getLifecyclePolicyPreview`, `ecr.op-contract-put-lifecycle-policy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/put_lifecycle_policy.rs`. Replaying the corpus, the emulator refused the `PutLifecyclePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutLifecyclePolicy operation: operation PutLifecyclePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call PutLifecyclePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
