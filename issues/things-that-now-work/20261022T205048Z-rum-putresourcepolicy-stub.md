Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.delete-resource-policy-then-get-rejected rum.e2e-app-monitor-lifecycle-with-policy-and-events rum.get-resource-policy-unknown-app-monitor-denied rum.put-resource-policy-concurrent-revision-conflict rum.put-resource-policy-stale-revision-conflict rum.put-resource-policy-then-get-reflects rum.put-rum-events-missing-permission-denied rum.representation-resource-policy-json-round-trip rum.resource-policy-document-byte-equality-round-trip rum.resource-policy-malformed-document-rejected rum.resource-policy-put-get-delete-lifecycle rum.resource-policy-revision-mismatch-rejected rum.server-side-delete-resource-policy-idempotent-enforcement rum.stale-policy-revision-delete-rejected
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|PutResourcePolicy|NotImplemented
Issue-Key: rum.resource-policy-malformed-document-rejected|run#8|AssertionError|assert err["Code"] == "MalformedPolicyDocumentException", err
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Implement rum PutResourcePolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourcePolicy

## Description
`PutResourcePolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

14 recorded unit(s) fail on their `PutResourcePolicy` call, so each one unblocks only when `PutResourcePolicy` works: `rum.delete-resource-policy-then-get-rejected`, `rum.e2e-app-monitor-lifecycle-with-policy-and-events`, `rum.get-resource-policy-unknown-app-monitor-denied`, `rum.put-resource-policy-concurrent-revision-conflict`, `rum.put-resource-policy-stale-revision-conflict`, `rum.put-resource-policy-then-get-reflects`, `rum.put-rum-events-missing-permission-denied`, `rum.representation-resource-policy-json-round-trip`, `rum.resource-policy-document-byte-equality-round-trip`, `rum.resource-policy-malformed-document-rejected`, `rum.resource-policy-put-get-delete-lifecycle`, `rum.resource-policy-revision-mismatch-rejected`, `rum.server-side-delete-resource-policy-idempotent-enforcement`, `rum.stale-policy-revision-delete-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rum-rum/src/ops/put_resource_policy.rs`. Replaying the corpus, the emulator refused the `PutResourcePolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutResourcePolicy operation: operation PutResourcePolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rum emulator
Call PutResourcePolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
