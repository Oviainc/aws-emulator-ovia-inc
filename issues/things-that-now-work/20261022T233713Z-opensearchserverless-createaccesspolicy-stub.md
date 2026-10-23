Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.access-policy-json-document-byte-preservation opensearchserverless.access-policy-lifecycle-transitions opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name opensearchserverless.create-access-policy-idempotent-clienttoken opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing opensearchserverless.delete-collection-with-active-index-cascades opensearchserverless.delete-nonexistent-access-policy-resourcenotfound opensearchserverless.deleted-collection-cascades-to-indexes opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving opensearchserverless.e2e-collection-index-lifecycle-teardown opensearchserverless.index-lifecycle-transitions opensearchserverless.index-mapping-json-round-trip opensearchserverless.server-side-lifecycle-policy-auto-applied opensearchserverless.update-access-policy-concurrent-version-conflict opensearchserverless.update-access-policy-optimistic-lock-stale-version-rejected
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless|CreateAccessPolicy|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement opensearchserverless CreateAccessPolicy: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPolicy

## Description
`CreateAccessPolicy` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_access_policy.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

15 recorded unit(s) fail on their `CreateAccessPolicy` call, so each one unblocks only when `CreateAccessPolicy` works: `opensearchserverless.access-policy-json-document-byte-preservation`, `opensearchserverless.access-policy-lifecycle-transitions`, `opensearchserverless.collection-endpoint-requires-aoss-sigv4-service-name`, `opensearchserverless.create-access-policy-idempotent-clienttoken`, `opensearchserverless.data-access-policy-read-permission-without-write-blocks-indexing`, `opensearchserverless.delete-collection-with-active-index-cascades`, `opensearchserverless.delete-nonexistent-access-policy-resourcenotfound`, `opensearchserverless.deleted-collection-cascades-to-indexes`, `opensearchserverless.deleting-network-policy-stops-the-live-collection-endpoint-serving`, `opensearchserverless.e2e-collection-index-lifecycle-teardown`, `opensearchserverless.index-lifecycle-transitions`, `opensearchserverless.index-mapping-json-round-trip`, `opensearchserverless.server-side-lifecycle-policy-auto-applied`, `opensearchserverless.update-access-policy-concurrent-version-conflict`, `opensearchserverless.update-access-policy-optimistic-lock-stale-version-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_access_policy.rs`. Replaying the corpus, the emulator refused the `CreateAccessPolicy` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateAccessPolicy operation: operation CreateAccessPolicy is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearchserverless emulator
Call CreateAccessPolicy and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
