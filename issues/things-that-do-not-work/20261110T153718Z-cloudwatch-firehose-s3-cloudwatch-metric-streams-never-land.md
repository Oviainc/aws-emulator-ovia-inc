Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors|_poll#26|Timeout|time.sleep(_POLL_S)
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace|_poll#22|Timeout|time.sleep(_POLL_S)
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream|_poll#23|Timeout|time.sleep(_POLL_S)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# cloudwatch__firehose__s3: CloudWatch metric streams never land records or publish their own stream metrics — the three obs-cloudwatch-2 checks exhaust a 600 s budget with no verdict

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream / GetMetricData (AWS/CloudWatch metric-stream metrics)

## Description
One net-new root cause in the cloudwatch__firehose__s3 suite, reached by three checks. It is NOT covered by the open issues for this heading — 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md and 20261109T135435Z-cloudwatch-firehose-s3-4-net-new-causes-metric.md carry other checks' replay keys and not these three; name them when merging, since the underlying export gap is plausibly shared.

Cause 1 of 1: a metric stream exports nothing to its Firehose/S3 destination and publishes none of the per-stream metrics CloudWatch emits about itself, so every assertion that joins a published datapoint to a landed record, or reads MetricUpdate / TotalMetricUpdate / PublishErrorRate, is never reached.
AWS: recorded behavior only for the delivery timing; the surface is modelled. A metric stream created with PutMetricStream and a Firehose delivery stream to S3 carries every matching PutMetricData datapoint to the bucket — as JSON, or as length-delimited OTLP protobuf ExportMetricsServiceRequest messages when OutputFormat is opentelemetry1.0 — and CloudWatch publishes, in the AWS/CloudWatch namespace dimensioned by MetricStreamName, MetricUpdate (reaching the published series count), TotalMetricUpdate (lifted above MetricUpdate when a StatisticsConfiguration adds statistics) and PublishErrorRate (exactly 0.0 for a stream whose role the service can assume, strictly greater than 0 for one it cannot). An ExcludeFilters stream lands no record of the excluded namespace while an including twin does; StopMetricStreams quiesces the subject stream while a control stream keeps going.
Emulator: all three checks are reported "Error: Check timed out after 600.0s" with no assertion reached — ten minutes of polling for a landed object or for a non-empty AWS/CloudWatch stream metric never satisfies the predicate, so neither the delivered bytes nor the stream's own metrics were ever compared.
Why: a client cannot export CloudWatch metrics anywhere. The observability pipeline that metric streams exist for produces no data, and because PublishErrorRate stays absent rather than positive, a stream whose IAM role is untrusted is indistinguishable from a healthy one — the failure is silent in both the data path and the telemetry about it.
Fix: the metric-stream export engine — a started stream must deliver matching PutMetricData datapoints to its Firehose destination in the configured OutputFormat (json / opentelemetry1.0), honour IncludeFilters / ExcludeFilters and StatisticsConfiguration, stop on StopMetricStreams, and publish the AWS/CloudWatch per-stream metrics MetricUpdate, TotalMetricUpdate and PublishErrorRate dimensioned by MetricStreamName.
Traces: cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors, cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace, cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream.

## Evidence
All three checks report:
  Error: Check timed out after 600.0s
with no assertion reached.

The awaited requirements, from the checks' own docstrings:
  "Two metric streams on one namespace differ only in the role's trusted principal. The trusted stream's MetricUpdate reaches the published series count and its PublishErrorRate is exactly 0.0; the untrusted stream reports PublishErrorRate > 0 and delivers nothing to S3."
  "An opentelemetry1.0 stream lands OTLP protobuf; an ExcludeFilters stream drops one namespace. The OTLP object decodes as length-delimited ExportMetricsServiceRequest messages carrying the published Summary; the excluding stream lands no record of the namespace an including twin exports."
  "A StatisticsConfiguration lifts TotalMetricUpdate above MetricUpdate and adds the extra statistics to landed records; StopMetricStreams stops the subject while the control keeps going."

Expected: a landed S3 object under each stream's prefix and non-empty AWS/CloudWatch MetricUpdate / TotalMetricUpdate / PublishErrorRate series dimensioned by MetricStreamName. Actual: neither ever appears inside a 600 s budget. Stream names, bucket names, namespaces and role ARNs are per-run values.

## Steps to Reproduce
1. Create a bucket and two Firehose delivery streams to it under distinct prefixes; wait ACTIVE.
2. PutMetricStream twice on one namespace, differing only in the IAM role's trusted principal (one assumable by streams.metrics.cloudwatch.amazonaws.com, one not); for the second scenario use OutputFormat opentelemetry1.0 on one stream and json on the other, and set ExcludeFilters on one; for the third add a StatisticsConfiguration.
3. PutMetricData into the namespace, then poll ListObjectsV2 under each prefix and GetMetricData for AWS/CloudWatch MetricUpdate, TotalMetricUpdate and PublishErrorRate dimensioned by MetricStreamName.
4. AWS lands records and publishes those series; the emulator lands nothing and publishes nothing. Call StopMetricStreams and confirm the subject quiesces while the control keeps going.
5. Re-replay the three cloudwatch__firehose__s3 checks in trace_ids to confirm the fix.
