Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dlm.concurrent-policy-creation-unique-ids dlm.create-custom-snapshot-policy-disabled dlm.create-custom-snapshot-policy-enabled dlm.create-policy-cross-region-copy-rule-config-round-trip dlm.create-policy-invalid-description-characters-rejected dlm.create-policy-missing-required-schedule-rejected dlm.create-policy-with-initial-tags dlm.create-policy-with-multiple-schedules-round-trip dlm.delete-lifecycle-policy-removes-policy dlm.get-lifecycle-policies-filter-by-policy-ids dlm.get-lifecycle-policies-filter-by-state dlm.get-lifecycle-policies-filter-by-target-tags dlm.get-lifecycle-policy-not-found dlm.tag-resource-and-list-tags-round-trip dlm.tag-resource-on-nonexistent-policy-not-found dlm.tag-resource-reserved-aws-prefix-key-rejected dlm.update-policy-preserves-untouched-fields dlm.update-policy-state-disabled-to-enabled dlm.update-policy-state-enabled-to-disabled dlm.update-policy-type-immutable-mutation-rejected
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: iam|CreateRole|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement iam CreateRole: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRole

## Description
`CreateRole` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_role.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

20 recorded unit(s) fail on their `CreateRole` call, so each one unblocks only when `CreateRole` works: `dlm.concurrent-policy-creation-unique-ids`, `dlm.create-custom-snapshot-policy-disabled`, `dlm.create-custom-snapshot-policy-enabled`, `dlm.create-policy-cross-region-copy-rule-config-round-trip`, `dlm.create-policy-invalid-description-characters-rejected`, `dlm.create-policy-missing-required-schedule-rejected`, `dlm.create-policy-with-initial-tags`, `dlm.create-policy-with-multiple-schedules-round-trip`, `dlm.delete-lifecycle-policy-removes-policy`, `dlm.get-lifecycle-policies-filter-by-policy-ids`, `dlm.get-lifecycle-policies-filter-by-state`, `dlm.get-lifecycle-policies-filter-by-target-tags`, `dlm.get-lifecycle-policy-not-found`, `dlm.tag-resource-and-list-tags-round-trip`, `dlm.tag-resource-on-nonexistent-policy-not-found`, `dlm.tag-resource-reserved-aws-prefix-key-rejected`, `dlm.update-policy-preserves-untouched-fields`, `dlm.update-policy-state-disabled-to-enabled`, `dlm.update-policy-state-enabled-to-disabled`, `dlm.update-policy-type-immutable-mutation-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/create_role.rs`. Replaying the corpus, the emulator refused the `CreateRole` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRole operation: operation CreateRole is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the iam emulator
Call CreateRole and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
