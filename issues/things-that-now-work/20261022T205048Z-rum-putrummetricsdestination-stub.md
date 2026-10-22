Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.batch-create-metric-definition-duplicate-name-conflict rum.batch-create-metric-definition-duplicate-rejected rum.batch-create-metric-definitions-malformed-namespace-rejected rum.batch-delete-metric-definition-removes-it rum.batch-delete-metric-definitions-missing-permission-denied rum.batch-delete-metric-definitions-partial-errors rum.batch-get-metric-definitions-unknown-destination-rejected rum.batch-get-rum-metric-definitions-invalid-destination-rejected rum.delete-app-monitor-with-active-metric-destination-cleans-up rum.delete-metrics-destination-then-list-empty rum.e2e-app-monitor-metric-pipeline rum.metric-definition-unicode-name-round-trip rum.put-metrics-destination-invalid-arn-rejected rum.representation-metric-definition-numeric-precision-preserved rum.telemetry-batch-create-metric-definitions-then-get-reflects rum.telemetry-batch-create-partial-failure-isolated rum.telemetry-batch-create-then-get-definitions rum.telemetry-batch-delete-definitions-removed rum.telemetry-batch-delete-metric-definitions-removes-definition rum.telemetry-delete-destination-removes-from-list rum.telemetry-delete-metrics-destination-removes-from-list rum.telemetry-put-destination-then-list-reflects rum.telemetry-put-metrics-destination-then-list-reflects rum.telemetry-update-metric-definition-persists-change rum.update-metric-definition-changes-filter-persists rum.update-metric-definition-concurrent-modification-conflict rum.update-rum-metric-definition-concurrent-conflict
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum|PutRumMetricsDestination|NotImplemented
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# Implement rum PutRumMetricsDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRumMetricsDestination

## Description
`PutRumMetricsDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rum-rum/src/ops/put_rum_metrics_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

27 recorded unit(s) fail on their `PutRumMetricsDestination` call, so each one unblocks only when `PutRumMetricsDestination` works: `rum.batch-create-metric-definition-duplicate-name-conflict`, `rum.batch-create-metric-definition-duplicate-rejected`, `rum.batch-create-metric-definitions-malformed-namespace-rejected`, `rum.batch-delete-metric-definition-removes-it`, `rum.batch-delete-metric-definitions-missing-permission-denied`, `rum.batch-delete-metric-definitions-partial-errors`, `rum.batch-get-metric-definitions-unknown-destination-rejected`, `rum.batch-get-rum-metric-definitions-invalid-destination-rejected`, `rum.delete-app-monitor-with-active-metric-destination-cleans-up`, `rum.delete-metrics-destination-then-list-empty`, `rum.e2e-app-monitor-metric-pipeline`, `rum.metric-definition-unicode-name-round-trip`, `rum.put-metrics-destination-invalid-arn-rejected`, `rum.representation-metric-definition-numeric-precision-preserved`, `rum.telemetry-batch-create-metric-definitions-then-get-reflects`, `rum.telemetry-batch-create-partial-failure-isolated`, `rum.telemetry-batch-create-then-get-definitions`, `rum.telemetry-batch-delete-definitions-removed`, `rum.telemetry-batch-delete-metric-definitions-removes-definition`, `rum.telemetry-delete-destination-removes-from-list` (+7 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rum-rum/src/ops/put_rum_metrics_destination.rs`. Replaying the corpus, the emulator refused the `PutRumMetricsDestination` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutRumMetricsDestination operation: operation PutRumMetricsDestination is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rum emulator
Call PutRumMetricsDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
