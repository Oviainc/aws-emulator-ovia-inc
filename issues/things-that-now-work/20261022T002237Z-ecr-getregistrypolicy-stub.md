Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr.concurrent-registry-policy-put-last-writer-wins ecr.delete-registry-policy-nonexistent-rejected ecr.delete-registry-policy-not-found-rejected ecr.get-registry-policy-not-found-before-put ecr.get-registry-policy-not-found-rejected ecr.put-registry-policy-invalid-json-rejected ecr.put-registry-policy-malformed-json-rejected ecr.put-registry-policy-overly-broad-principal-rejected ecr.registry-policy-delete-then-not-found ecr.registry-policy-document-unicode-round-trip ecr.registry-policy-invalid-document-rejected ecr.registry-policy-lifecycle-transitions ecr.registry-policy-unicode-document-round-trip ecr.registry-policy-unicode-round-trip
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|GetRegistryPolicy|NotImplemented
Issue-Commit: ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3

# Implement ecr GetRegistryPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ace69a9d3348e9c8c81d3eff309bd15ea7c6e8c3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRegistryPolicy

## Description
`GetRegistryPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/get_registry_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `GetRegistryPolicy` call, so each one unblocks only when `GetRegistryPolicy` works: `ecr.concurrent-registry-policy-put-last-writer-wins`, `ecr.delete-registry-policy-nonexistent-rejected`, `ecr.delete-registry-policy-not-found-rejected`, `ecr.get-registry-policy-not-found-before-put`, `ecr.get-registry-policy-not-found-rejected`, `ecr.put-registry-policy-invalid-json-rejected`, `ecr.put-registry-policy-malformed-json-rejected`, `ecr.put-registry-policy-overly-broad-principal-rejected`, `ecr.registry-policy-delete-then-not-found`, `ecr.registry-policy-document-unicode-round-trip`, `ecr.registry-policy-invalid-document-rejected`, `ecr.registry-policy-lifecycle-transitions`, `ecr.registry-policy-unicode-document-round-trip`, `ecr.registry-policy-unicode-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/src/ops/get_registry_policy.rs`. Replaying the corpus, the emulator refused the `GetRegistryPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetRegistryPolicy operation: operation GetRegistryPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ecr emulator
Call GetRegistryPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
