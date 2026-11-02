Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.metrics-from-logs-default-value-zero-fills-non-matching-lambda-lines cloudwatch__lambda__logs.metrics-from-logs-dimensions-with-default-value-rejected cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric cloudwatch__lambda__logs.metrics-from-logs-insights-stats-by-agrees-with-dimensioned-metric-filter cloudwatch__lambda__logs.metrics-from-logs-json-selector-filter-counts-only-parsed-json-error-lines cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected cloudwatch__lambda__logs.metrics-from-logs-no-matching-line-emits-no-datapoint-and-an-empty-complete-query cloudwatch__lambda__logs.metrics-from-logs-regex-pattern-agrees-across-metric-filter-insights-and-filter-log-events cloudwatch__lambda__logs.metrics-from-logs-size-field-metric-value-is-extracted-not-counted cloudwatch__lambda__logs.metrics-from-logs-space-delimited-filter-counts-lambda-lines-under-declared-dimension cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights cloudwatch__logs.deep-cwlogs-semantics-filter-log-events-and-metric-filter-agree-on-pattern-verdicts cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected cloudwatch__logs__sns__sqs.log-alarm-notification-blocked-without-sqs-queue-policy cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm
Issue-Key: cloudwatch-logs|PutMetricFilter|NotImplemented
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-dimensions-with-default-value-rejected|run#9|AssertionError|assert err == expected, (
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected|run#9|AssertionError|assert err == expected, (
Issue-Key: cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected|run#9|AssertionError|assert err == expected, (
Issue-Key: cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected|run#14|AssertionError|assert err == expected, (
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# Implement cloudwatch-logs PutMetricFilter: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
`PutMetricFilter` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_metric_filter.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `PutMetricFilter` call, so each one unblocks only when `PutMetricFilter` works: `cloudwatch__lambda__logs.metrics-from-logs-default-value-zero-fills-non-matching-lambda-lines`, `cloudwatch__lambda__logs.metrics-from-logs-dimensions-with-default-value-rejected`, `cloudwatch__lambda__logs.metrics-from-logs-fourth-dimension-rejected`, `cloudwatch__lambda__logs.metrics-from-logs-insights-parse-named-capture-agrees-with-extracted-metric`, `cloudwatch__lambda__logs.metrics-from-logs-insights-stats-by-agrees-with-dimensioned-metric-filter`, `cloudwatch__lambda__logs.metrics-from-logs-json-selector-filter-counts-only-parsed-json-error-lines`, `cloudwatch__lambda__logs.metrics-from-logs-literal-dimension-value-rejected`, `cloudwatch__lambda__logs.metrics-from-logs-no-matching-line-emits-no-datapoint-and-an-empty-complete-query`, `cloudwatch__lambda__logs.metrics-from-logs-regex-pattern-agrees-across-metric-filter-insights-and-filter-log-events`, `cloudwatch__lambda__logs.metrics-from-logs-size-field-metric-value-is-extracted-not-counted`, `cloudwatch__lambda__logs.metrics-from-logs-space-delimited-filter-counts-lambda-lines-under-declared-dimension`, `cloudwatch__lambda__logs__sfn.obs-sfn-3-express-failures-counted-by-metric-filter-subscriber-and-insights`, `cloudwatch__logs.deep-cwlogs-semantics-filter-log-events-and-metric-filter-agree-on-pattern-verdicts`, `cloudwatch__logs.deep-cwlogs-semantics-metric-filter-dimensions-with-default-value-rejected`, `cloudwatch__logs.obs-logs-2-transformer-without-leading-parser-rejected`, `cloudwatch__logs__sns__sqs.log-alarm-notification-blocked-without-sqs-queue-policy`, `cloudwatch__logs__sns__sqs.metric-filter-default-value-fills-only-ingested-periods`, `cloudwatch__logs__sns__sqs.metric-filter-match-drives-alarm-and-notifies`, `cloudwatch__logs__sns__sqs.metric-filter-non-matching-pattern-no-datapoint-no-alarm`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_metric_filter.rs`. Replaying the corpus, the emulator refused the `PutMetricFilter` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutMetricFilter operation: operation PutMetricFilter is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cloudwatch-logs emulator
Call PutMetricFilter and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
