Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.delete-metric-stream-status-immediately-absent-from-list cloudwatch.list-metric-streams-list-order-stable-across-repeated-calls cloudwatch.list-metric-streams-pagination-order-stable-across-pages cloudwatch.list-metric-streams-reflects-created-and-deleted-streams cloudwatch.metric-stream-lifecycle-status-and-deletion-visible-in-list cloudwatch.mstrx-cli-solve-queries-print-none-for-include-streams-and-tab-joined-names cloudwatch.mstrx-derivation-selects-only-the-exclude-stream-across-single-entry-pages cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle cloudwatch.mstrx-list-entries-carry-no-filters-so-the-exclusion-needs-a-get cloudwatch.op-contract-listMetricStreams-state-visibility
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|ListMetricStreams|NotImplemented
Issue-Key: cloudwatch.mstrx-cli-solve-queries-print-none-for-include-streams-and-tab-joined-names|cli#12|AssertionError|assert proc.returncode == 0, f"aws cloudwatch {' '.join(args)} exited {proc.returncode}: {proc.stderr}"
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Implement cloudwatch ListMetricStreams: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMetricStreams

## Description
`ListMetricStreams` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/list_metric_streams.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `ListMetricStreams` call, so each one unblocks only when `ListMetricStreams` works: `cloudwatch.delete-metric-stream-status-immediately-absent-from-list`, `cloudwatch.list-metric-streams-list-order-stable-across-repeated-calls`, `cloudwatch.list-metric-streams-pagination-order-stable-across-pages`, `cloudwatch.list-metric-streams-reflects-created-and-deleted-streams`, `cloudwatch.metric-stream-lifecycle-status-and-deletion-visible-in-list`, `cloudwatch.mstrx-cli-solve-queries-print-none-for-include-streams-and-tab-joined-names`, `cloudwatch.mstrx-derivation-selects-only-the-exclude-stream-across-single-entry-pages`, `cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle`, `cloudwatch.mstrx-list-entries-carry-no-filters-so-the-exclusion-needs-a-get`, `cloudwatch.op-contract-listMetricStreams-state-visibility`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/list_metric_streams.rs`. Replaying the corpus, the emulator refused the `ListMetricStreams` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListMetricStreams operation: operation ListMetricStreams is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatch emulator
Call ListMetricStreams and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
