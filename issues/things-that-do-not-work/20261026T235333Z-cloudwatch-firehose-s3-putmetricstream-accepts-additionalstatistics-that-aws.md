Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic|_error#9|AssertionError|raise AssertionError(f"call was expected to be refused but succeeded: {resp!r}")
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# cloudwatch__firehose__s3: PutMetricStream accepts AdditionalStatistics that AWS refuses for the stream's OutputFormat (1 cause)

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
ONE net-new root cause in the CloudWatch metric-streams control plane.

=== Cause 1 of 1: StatisticsConfigurations[].AdditionalStatistics is never validated against the stream's OutputFormat, so unsupported statistics are accepted with 200 ===
Failing check: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic.

AWS: the rule is in the model. Smithy (model_path services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/com-amazonaws-cloudwatch-graniteserviceversion20100801.json) shape MetricStreamStatisticsConfiguration.AdditionalStatistics (@required, list MetricStreamStatisticsAdditionalStatistics) documents: "If the OutputFormat for the stream is opentelemetry1.0 or opentelemetry0.7, the only valid values are p?? percentile statistics such as p90, p99 and so on. If the OutputFormat for the stream is json, the valid values include the abbreviations for all of the statistics listed in CloudWatch statistics definitions. For example, this includes tm98, wm90, PR(:300), and so on.\" OutputFormat targets enum MetricStreamOutputFormat with values json|opentelemetry0.7|opentelemetry1.0 and is @required. PutMetricStream models InvalidParameterValueException (errorKind client, httpError 400, awsQueryError code InvalidParameterValue). Recorded behavior (the check records a measurement against real AWS, us-west-2, 2026-10-01) fills in what the model leaves open: `Average` is refused for EVERY OutputFormat, json included, because it is a base statistic a metric stream always sends and not an "additional" one; a trimmed mean such as `TM(10%:90%)` (and IQM, wm99, TC, PR) is ACCEPTED for json and REFUSED for opentelemetry1.0 — that is the format-dependent half; percentiles such as `p99` are accepted for both. Every refusal is HTTP 400, Error.Code `InvalidParameterValue`, Error.Message `Unsupported statistic <stat> for selected OutputFormat`. In that message the fixed text is `Unsupported statistic ` and ` for selected OutputFormat`; the only variable part is `<stat>`, the offending AdditionalStatistics entry echoed verbatim from the request (no quoting, no normalisation). The refusal is also stateless in both directions: a refused CREATE must leave no stream behind (a following GetMetricStream answers ResourceNotFoundException), and a refused UPSERT of an existing stream must leave that stream's OutputFormat and StatisticsConfigurations exactly as they were.

Emulator: the pairing is never evaluated, so the call succeeds. PutMetricStream with OutputFormat="opentelemetry1.0" and StatisticsConfigurations=[{IncludeMetrics:[{Namespace:<ns>, MetricName:"Latency"}], AdditionalStatistics:["Average"]}] answered HTTP 200 with {'Arn': 'arn:aws:cloudwatch:<region>:<account>:metric-stream/<name>'} — the check reports "AssertionError: call was expected to be refused but succeeded". Because AdditionalStatistics is typed as a list of free strings, a model-driven handler accepts any value for any format; no 400/InvalidParameterValue is produced for `Average` (any format) nor for `TM(10%:90%)` with opentelemetry1.0.

Why: a client cannot discover a misconfigured metric stream. On AWS the stream is never created and the caller must fix the statistic or the format; against the emulator the create succeeds, the stream goes live, and the consumer then receives an OTel payload that cannot carry the requested statistic — the configuration error surfaces downstream, in the Firehose/consumer leg, instead of synchronously at the API. It also makes the two state invariants untestable: a caller cannot rely on "a refused create leaves nothing" or "a refused upsert does not mutate" if nothing is ever refused.

Fix: input validation in the PutMetricStream handler of the CloudWatch service crate, run before any state mutation: for each StatisticsConfigurations entry, reject the whole request with InvalidParameterValueException (400, awsQueryError code InvalidParameterValue, message `Unsupported statistic <stat> for selected OutputFormat`) on the first AdditionalStatistics entry that is not admissible for the requested OutputFormat — for opentelemetry1.0/opentelemetry0.7 only p?? percentiles are admissible; for json the statistic abbreviations are admissible but the base statistics (Average, and the always-streamed MAX/MIN/SUM/SAMPLECOUNT family) are not. Validation must happen on the upsert path too, and must return before the existing stream record is touched.

Traces: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic.

Relationship to open issues: 20261025T020208Z-cloudwatch-firehose-s3-getmetricstream-s-not-found-error-carries-a-message.md covers GetMetricStream's not-found MESSAGE, 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md and 20261025T032458Z-cloudwatch-firehose-s3-a-cloudformation-deployed-metric-stream.md cover the metric-stream DATA plane, and 20261026T031713Z-cloudwatch-2-net-new-root-causes-putmetricalarm-refuses.md covers PutMetricAlarm. None of them carries this check id or the AdditionalStatistics/OutputFormat validation relation, and the replay report does not mark this check OWNED by any of them.

## Evidence
Replayed suite: simparity-behavioral-ga-prod-v1. cloudwatch__firehose__s3 0/7 (0%); six of the seven failures are marked OWNED by existing issues, this one is not.

Expected (AWS, model + recorded): put_metric_stream(Name=N, FirehoseArn=F, RoleArn=R, OutputFormat='opentelemetry1.0', IncludeFilters=[{'Namespace': NS}], StatisticsConfigurations=[{'IncludeMetrics': [{'Namespace': NS, 'MetricName': 'Latency'}], 'AdditionalStatistics': ['Average']}]) -> HTTP 400, Error.Code 'InvalidParameterValue', Error.Message 'Unsupported statistic Average for selected OutputFormat'; and get_metric_stream(Name=N) afterwards -> ResourceNotFoundException. Same refusal for OutputFormat='json' + 'Average', and for OutputFormat='opentelemetry1.0' + 'TM(10%:90%)' (message 'Unsupported statistic TM(10%:90%) for selected OutputFormat').

Actual (emulator): HTTP 200 and the stream is created —
  cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic:
  "AssertionError: call was expected to be refused but succeeded: {'Arn': 'arn:aws:cloudwatch:us-west-2:000000000000:metric-stream/sim2real-cwstreamfmt-<id>', 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', ...}}}"
(the ARN's account, region and name suffix are per-run values, not part of the requirement; the requirement is the 200 vs the modeled 400.)

The check's positive controls — opentelemetry1.0 + ['p99'], then json + ['TM(10%:90%)'] against the same name, a real Firehose delivery stream and a role trusted by streams.metrics.cloudwatch.amazonaws.com — are expected to keep succeeding, so the fix must be the format/statistic relation, not a blanket refusal of AdditionalStatistics.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=B); create an IAM role trusted by firehose.amazonaws.com with s3:PutObject on B; firehose.create_delivery_stream(DeliveryStreamName=FH, DeliveryStreamType='DirectPut', S3DestinationConfiguration={'RoleARN': R_fh, 'BucketARN': 'arn:aws:s3:::B'}) and wait for ACTIVE; note its ARN F.
3. Create an IAM role R trusted by streams.metrics.cloudwatch.amazonaws.com allowing firehose:PutRecord/PutRecordBatch on F.
4. cw.put_metric_stream(Name=N, FirehoseArn=F, RoleArn=R, OutputFormat='opentelemetry1.0', IncludeFilters=[{'Namespace': NS}], StatisticsConfigurations=[{'IncludeMetrics': [{'Namespace': NS, 'MetricName': 'Latency'}], 'AdditionalStatistics': ['Average']}]).
   Expect: 400 InvalidParameterValue, message 'Unsupported statistic Average for selected OutputFormat'. Observe: 200 with an Arn.
5. cw.get_metric_stream(Name=N). Expect ResourceNotFoundException (the refused create left nothing). Observe the stream exists.
6. Repeat step 4 with OutputFormat='json' and AdditionalStatistics=['Average'] -> expect the same 400 (Average is refused for every format); and with OutputFormat='opentelemetry1.0' and AdditionalStatistics=['TM(10%:90%)'] -> expect 400 'Unsupported statistic TM(10%:90%) for selected OutputFormat'.
7. Positive controls that must still succeed: OutputFormat='opentelemetry1.0' with ['p99'], and OutputFormat='json' with ['TM(10%:90%)'] — each must read back unchanged through get_metric_stream.
8. Upsert invariance: with the json/['TM(10%:90%)'] stream live, repeat the opentelemetry1.0 + ['TM(10%:90%)'] call; expect the same 400 and get_metric_stream still reporting OutputFormat='json' with the unchanged StatisticsConfigurations.
9. Re-replay cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-rejects-average-additional-statistic.
