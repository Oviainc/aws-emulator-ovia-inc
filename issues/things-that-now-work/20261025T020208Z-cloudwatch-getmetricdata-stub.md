Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-math-and-anomaly-band-materializes-and-honours-excluded-range cloudwatch.get-metric-data-invalid-next-token-rejected cloudwatch.get-metric-data-multi-query-response-order-matches-request-order cloudwatch.get-metric-data-query-at-retention-boundary-returns-exact-cutoff cloudwatch.get-metric-data-query-result-order-matches-request-order cloudwatch.metrics-math-and-emf-get-metric-data-pagination-reassembles-every-query-in-scan-order cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error cloudwatch.metrics-math-and-emf-metric-math-semantically-invalid-expressions-rejected cloudwatch.metrics-math-and-emf-metric-math-time-series-functions-evaluate-per-datapoint cloudwatch.metrics-math-and-emf-percentile-family-statistics-follow-the-raw-distribution cloudwatch.metrics-math-and-emf-period-buckets-anchor-at-minute-rounded-start-time cloudwatch.metrics-math-and-emf-search-partial-dimension-match-feeds-sum-and-avg cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch|GetMetricData|NotImplemented
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Implement cloudwatch GetMetricData: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
`GetMetricData` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_metric_data.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

13 recorded unit(s) fail on their `GetMetricData` call, so each one unblocks only when `GetMetricData` works: `cloudwatch.alarm-math-and-anomaly-band-materializes-and-honours-excluded-range`, `cloudwatch.get-metric-data-invalid-next-token-rejected`, `cloudwatch.get-metric-data-multi-query-response-order-matches-request-order`, `cloudwatch.get-metric-data-query-at-retention-boundary-returns-exact-cutoff`, `cloudwatch.get-metric-data-query-result-order-matches-request-order`, `cloudwatch.metrics-math-and-emf-get-metric-data-pagination-reassembles-every-query-in-scan-order`, `cloudwatch.metrics-math-and-emf-metric-math-non-finite-points-dropped-with-arithmetic-error`, `cloudwatch.metrics-math-and-emf-metric-math-semantically-invalid-expressions-rejected`, `cloudwatch.metrics-math-and-emf-metric-math-time-series-functions-evaluate-per-datapoint`, `cloudwatch.metrics-math-and-emf-percentile-family-statistics-follow-the-raw-distribution`, `cloudwatch.metrics-math-and-emf-period-buckets-anchor-at-minute-rounded-start-time`, `cloudwatch.metrics-math-and-emf-search-partial-dimension-match-feeds-sum-and-avg`, `cloudwatch.search-expression-discovers-series-server-side-and-is-refused-in-an-alarm`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/get_metric_data.rs`. Replaying the corpus, the emulator refused the `GetMetricData` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetMetricData operation: operation GetMetricData is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatch emulator
Call GetMetricData and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
