Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id opensearch.concurrent-start-migration-independent-domains opensearch.data-plane-list-data-source-attachments-completeness opensearch.e2e-data-source-attachment-ingest-search-detach opensearch.list-data-source-attachments-stable-creation-ordering opensearch.ordering-data-source-attachment-listing-order opensearch.start-migration-conflict-when-domain-busy-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|AttachDataSource|NotImplemented
Issue-Commit: 88eb145164ed6e7175261f755ebef46622c173d0

# Implement opensearch AttachDataSource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `88eb145164ed6e7175261f755ebef46622c173d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachDataSource

## Description
`AttachDataSource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/attach_data_source.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `AttachDataSource` call, so each one unblocks only when `AttachDataSource` works: `opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching`, `opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id`, `opensearch.concurrent-start-migration-independent-domains`, `opensearch.data-plane-list-data-source-attachments-completeness`, `opensearch.e2e-data-source-attachment-ingest-search-detach`, `opensearch.list-data-source-attachments-stable-creation-ordering`, `opensearch.ordering-data-source-attachment-listing-order`, `opensearch.start-migration-conflict-when-domain-busy-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/attach_data_source.rs`. Replaying the corpus, the emulator refused the `AttachDataSource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AttachDataSource operation: operation AttachDataSource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the opensearch emulator
Call AttachDataSource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
