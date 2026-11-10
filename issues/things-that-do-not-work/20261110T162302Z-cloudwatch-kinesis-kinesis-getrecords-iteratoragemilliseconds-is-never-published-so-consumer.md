Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up
Issue-Key: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up|_poll#7|Timeout|time.sleep(interval)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# Kinesis GetRecords.IteratorAgeMilliseconds is never published, so consumer lag is invisible

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the AWS/Kinesis GetRecords.IteratorAgeMilliseconds series is never emitted.
AWS: a stream whose consumer reads from behind the tip publishes GetRecords.IteratorAgeMilliseconds in AWS/Kinesis (under {StreamName}, and per shard under {StreamName, ShardId}) with a non-zero age reflecting how far behind the consumer is; once the consumer catches up to the tip the series reads 0. Metric emission is not in the API model: documented/recorded behavior only.
Emulator: the check writes records, reads deliberately behind the tip and polls that series until its own 600s budget expires; no datapoint ever appears, so neither the lagging non-zero reading nor the caught-up zero can be observed. The run reports only the expired budget.
Why: iterator age is the primary lag signal for every Kinesis consumer; a client alarming on it never evaluates, and a backed-up stream looks identical to a healthy one.
Fix: the kinesis data plane in services/service-com-amazonaws-kinesis-kinesis-20131202 must publish the GetRecords family (including IteratorAgeMilliseconds) into the cloudwatch metric store (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) under {StreamName} and the per-shard dimension set.
Why this is not covered by an existing issue: 20261109T183935Z-cloudwatch-kinesis-no-aws-kinesis-stream-metric-series.md owns the sibling check of this heading but not this replay key.
Traces: cloudwatch__kinesis.metrics-states-stream-iterator-age-nonzero-while-consumer-lags-and-zero-once-caught-up.

## Evidence
Expected: AWS/Kinesis GetRecords.IteratorAgeMilliseconds datapoints > 0 while the consumer reads behind the tip, then 0 once caught up; actual: 'Error: Check timed out after 600.0s' — the series produced no datapoint at all.

## Steps to Reproduce
1. CreateStream and PutRecord a handful of payloads, then one more at the tip.
2. GetShardIterator at TRIM_HORIZON and GetRecords only part way, leaving the consumer behind the tip.
3. Poll cloudwatch GetMetricStatistics, Namespace AWS/Kinesis, MetricName GetRecords.IteratorAgeMilliseconds, Dimensions=[{StreamName}] -> no datapoints within 600s; the same after reading to the tip.
