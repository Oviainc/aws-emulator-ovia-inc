Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.emf-log-to-metric-extraction-exactness
Issue-Key: cloudwatch__logs.emf-log-to-metric-extraction-exactness|run#58|AssertionError|assert set(catalogue) == expected_catalogue, (
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# EMF extraction drops a declared metric projection: a metric whose EMF envelope declares an empty dimension set never appears in ListMetrics

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLogEvents

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: the EMF-to-metric extractor does not materialise the projection with no dimensions.
AWS: recorded behavior only (EMF extraction is not described in the Smithy model). An embedded-metric-format log line materialises EXACTLY the metric series its `_aws.CloudWatchMetrics` envelope declares — one series per declared dimension projection, including the empty projection `[]` (a metric declared with no dimension set), and nothing else. ListMetrics for the namespace must therefore return exactly the declared (name, dimension-set) pairs.
Emulator: ListMetrics returned `[('OrderCount', ('Operation','Service')), ('OrderCount', ('Service',)), ('OrderLatency', ('Operation','Service')), ('OrderLatency', ('Service',))]` — the declared `('OrphanMetric', ())` series is missing. The dimensioned projections are extracted; the zero-dimension projection is dropped.
Why: a metric a customer intentionally publishes without dimensions (the account/namespace-wide aggregate) is silently absent, so dashboards and alarms built on it stay in INSUFFICIENT_DATA with no error anywhere; the check notes that extra entries would mean invented series and missing ones a dropped projection — this is the latter.
Fix: the EMF extractor in the CloudWatch Logs ingestion path must emit a series for every declared projection, including the empty dimension set.
Traces: cloudwatch__logs.emf-log-to-metric-extraction-exactness.

## Evidence
cloudwatch__logs.emf-log-to-metric-extraction-exactness:
  AssertionError: ListMetrics for the EMF namespace returned [('OrderCount', ('Operation', 'Service')), ('OrderCount', ('Service',)), ('OrderLatency', ('Operation', 'Service')), ('OrderLatency', ('Service',))]; expected exactly [('OrderCount', ('Operation', 'Service')), ('OrderCount', ('Service',)), ('OrderLatency', ('Operation', 'Service')), ('OrderLatency', ('Service',)), ('OrphanMetric', ())]. Extra entries mean the extractor invented series the envelope never declared; missing ones mean it dropped a declared projection

## Steps to Reproduce
Replay cloudwatch__logs.emf-log-to-metric-extraction-exactness: PutLogEvents an EMF line whose `_aws.CloudWatchMetrics[0].Dimensions` declares `[["Service","Operation"],["Service"],[]]`-style projections (including one metric with an empty dimension set), then call ListMetrics for the EMF namespace and compare the exact set of (MetricName, dimension-names) pairs.
