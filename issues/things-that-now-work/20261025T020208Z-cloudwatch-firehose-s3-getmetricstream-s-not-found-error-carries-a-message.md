Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic|run#9|AssertionError|assert missing == expected_missing, (
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# GetMetricStream's not-found error carries a message where AWS sends an empty one, so a refused PutMetricStream cannot be confirmed to have created nothing (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStream

## Description
One net-new root cause.

Cause 1 of 1: GetMetricStream answers not-found with a non-empty Message.
AWS: Smithy models ResourceNotFoundException (httpError 404) on GetMetricStream, and the recorded behavior is that CloudWatch answers this particular not-found with an EMPTY Message — the error code alone carries the meaning. A client confirming that a refused PutMetricStream (here: OutputFormat='opentelemetry1.0' with an 'Average' additional statistic, which AWS refuses) left no stream behind compares the pair ('ResourceNotFoundException', '').
Emulator: answers ('ResourceNotFoundException', 'the metric stream <name> does not exist') — right code, but a message AWS does not send.
Why: this is the same defect as cloudwatch.op-contract-getMetricStream-dup (filed for service `cloudwatch`), reached through the Firehose/S3 metric-stream path; a client asserting on the message-less not-found contract — which is how CloudWatch distinguishes this not-found from its messaged ones — diverges, and the "the stream must not exist" confirmation after a refused create fails even though the stream genuinely does not exist.
Fix: the not-found error construction in the GetMetricStream handler (emit the modeled ResourceNotFoundException with no Message).
Traces: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic.

## Evidence
assert missing == expected_missing -> "AssertionError: after the refused 'opentelemetry1.0'/'Average' create, GetMetricStream returned ('ResourceNotFoundException', 'the metric stream sim2real-cwstreamfmt-0fa91f88a896 does not exist'), expected ('ResourceNotFoundException', ''); the stream must not exist".

## Steps to Reproduce
1. PutMetricStream(Name=s, OutputFormat='opentelemetry1.0', StatisticsConfigurations naming the 'Average' additional statistic) -> refused.
2. GetMetricStream(Name=s) -> ResourceNotFoundException with Message 'the metric stream s does not exist'; AWS returns ResourceNotFoundException with an empty Message.
