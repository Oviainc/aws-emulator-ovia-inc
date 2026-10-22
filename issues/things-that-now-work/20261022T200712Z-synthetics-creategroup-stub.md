Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.associate-resource-duplicate-association-conflict-rejected synthetics.concurrent-associate-resource-single-winner synthetics.disassociate-resource-removes-association synthetics.list-associated-groups-reflects-membership synthetics.tag-resource-and-list-tags-roundtrip synthetics.untag-resource-removes-only-specified-key
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics|CreateGroup|NotImplemented
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Implement synthetics CreateGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
`CreateGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-synthetics-synthetics/src/ops/create_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateGroup` call, so each one unblocks only when `CreateGroup` works: `synthetics.associate-resource-duplicate-association-conflict-rejected`, `synthetics.concurrent-associate-resource-single-winner`, `synthetics.disassociate-resource-removes-association`, `synthetics.list-associated-groups-reflects-membership`, `synthetics.tag-resource-and-list-tags-roundtrip`, `synthetics.untag-resource-removes-only-specified-key`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-synthetics-synthetics/src/ops/create_group.rs`. Replaying the corpus, the emulator refused the `CreateGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateGroup operation: operation CreateGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the synthetics emulator
Call CreateGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
