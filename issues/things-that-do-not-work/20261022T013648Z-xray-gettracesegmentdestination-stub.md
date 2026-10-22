Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: xray.annotation-value-types-are-indexed-by-type-not-stringified xray.e2e-trace-lifecycle-put-retrieve-cancel xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods xray.get-trace-segment-destination-default xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span xray.get-trace-summaries-returns-time-descending-order xray.get-trace-summaries-string-operators-match-the-submitted-url xray.get-trace-summaries-time-range-type-selects-start-or-end-time xray.in-progress-segment-is-overwritten-by-its-completed-twin xray.put-telemetry-records-malformed-resource-arn-rejected xray.put-trace-segments-missing-trace-id-rejected xray.service-and-edge-filter-keywords-select-by-submitted-topology xray.telemetry-only-host-absent-from-service-graph xray.trace-assembled-from-independently-submitted-out-of-order-segments xray.trace-segment-destination-cloudwatch-logs-delivery-enforced xray.trace-summaries-ordered-by-time xray.update-trace-segment-destination-invalid-value-fails xray.update-trace-segment-destination-without-logs-policy-fails
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetTraceSegmentDestination|NotImplemented
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Implement xray GetTraceSegmentDestination: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTraceSegmentDestination

## Description
`GetTraceSegmentDestination` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_segment_destination.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `GetTraceSegmentDestination` call, so each one unblocks only when `GetTraceSegmentDestination` works: `xray.annotation-value-types-are-indexed-by-type-not-stringified`, `xray.e2e-trace-lifecycle-put-retrieve-cancel`, `xray.get-time-series-service-statistics-buckets-two-segments-into-two-periods`, `xray.get-trace-segment-destination-default`, `xray.get-trace-summaries-number-keyword-filters-derive-from-the-submitted-span`, `xray.get-trace-summaries-returns-time-descending-order`, `xray.get-trace-summaries-string-operators-match-the-submitted-url`, `xray.get-trace-summaries-time-range-type-selects-start-or-end-time`, `xray.in-progress-segment-is-overwritten-by-its-completed-twin`, `xray.put-telemetry-records-malformed-resource-arn-rejected`, `xray.put-trace-segments-missing-trace-id-rejected`, `xray.service-and-edge-filter-keywords-select-by-submitted-topology`, `xray.telemetry-only-host-absent-from-service-graph`, `xray.trace-assembled-from-independently-submitted-out-of-order-segments`, `xray.trace-segment-destination-cloudwatch-logs-delivery-enforced`, `xray.trace-summaries-ordered-by-time`, `xray.update-trace-segment-destination-invalid-value-fails`, `xray.update-trace-segment-destination-without-logs-policy-fails`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-xray-awsxray/src/ops/get_trace_segment_destination.rs`. Replaying the corpus, the emulator refused the `GetTraceSegmentDestination` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetTraceSegmentDestination operation: operation GetTraceSegmentDestination is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the xray emulator
Call GetTraceSegmentDestination and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
