Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99 cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace cloudwatch__firehose__s3.metrics-states-stream-denied-role-delivery-metrics-stay-zero-control-matches-landed-bytes cloudwatch__firehose__s3.incoming-records-and-delivery-to-s3-join-the-put-to-the-delivered-object
Issue-Key: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing|run#31|AssertionError|assert broken_keys == [], (
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors|_poll#26|AssertionError|raise AssertionError(f"{description}: not reached within {_CEILING_S}s; last {seen!r}")
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream|_poll#23|AssertionError|raise AssertionError(f"{description}: not reached within {_CEILING_S}s; last {seen!r}")
Issue-Key: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99|run#19|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace|_poll#22|AssertionError|raise AssertionError(f"{description}: not reached within {_CEILING_S}s; last {brief!r}")
Issue-Key: cloudwatch__firehose__s3.metrics-states-stream-denied-role-delivery-metrics-stay-zero-control-matches-landed-bytes|run#25|AssertionError|assert got is not None and {k: got[k] for k in want} == want, (
Issue-Key: cloudwatch__firehose__s3.incoming-records-and-delivery-to-s3-join-the-put-to-the-delivered-object|_poll_until_stable#21|Timeout|time.sleep(interval_s)
Issue-Commit: 3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8

# cloudwatch__firehose__s3: 4 net-new causes — metric streams ignore the stream role's trust policy, no AWS/CloudWatch/MetricStreams metrics, OTel output formats deliver nothing, and no AWS/Firehose ingestion/delivery metrics

> _Note: this issue was identified against the code base at commit `3b4b4e2df28997ae7cbb85154b0c83cf06a43ef8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricStream

## Description
FOUR net-new root causes in the CloudWatch metric-stream → Firehose → S3 path, all in checks the suite lists under cloudwatch__firehose__s3 and none marked ALREADY FILED or OWNED.

Relationship to open issues: 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md describes the single cause "no record ever reaches the Firehose stream and no S3 object is written". That is NOT what this replay shows: json-format streams DO export now (records with metric_stream_name/namespace/metric_name/value{max,min,sum,count} were read back out of the inc/exc prefixes, and the deliberately-untrusted stream exported two objects). 20261025T002604Z and 20261025T032458Z cover the CloudFormation-deployed pipeline only. The four causes below are what becomes observable once export works, and each needs its own fix.

=== Cause 1 of 4: a metric stream whose role cannot be assumed by CloudWatch still exports ===
Failing check: cloudwatch__firehose__s3.cw-metric-stream-delivers-through-firehose-to-s3-while-untrusted-role-delivers-nothing.
AWS: Smithy PutMetricStreamInput.RoleArn (required) is documented as "The ARN of an IAM role that this metric stream will use to access Amazon Kinesis Data Firehose resources. This IAM role must already exist and must be in the same account as the metric stream. This IAM role must include the following permissions: firehose:PutRecord, firehose:PutRecordBatch". PutMetricStream accepts a syntactically valid role, so the boundary is only visible in the data plane: CloudWatch assumes that role to call firehose:PutRecord(Batch), and a role whose trust policy names a different principal (here ec2.amazonaws.com) cannot be assumed, so NOTHING is ever published through that stream — the destination prefix stays empty — while the sibling stream whose role trusts streams.metrics.cloudwatch.amazonaws.com exports normally.
Emulator: export is performed with no identity check at all. The stream wired to the untrusted role exported 2 objects under its prefix ('<bucket>/<broken-prefix>/2026/11/09/13/<stream>-1-...-0-<uuid>' ×2). The positive leg also exports, so it is not that export is skipped — the assume-role/trust evaluation is simply never consulted.
Why: a client cannot discover a misconfigured metric-stream role. On AWS such a stream silently exports nothing (and reports PublishErrorRate); on the emulator every stream looks healthy, so trust-policy regressions, least-privilege rollouts and break-glass revocations read the opposite of production.
Fix: the metric-stream publisher component must resolve the stream's RoleArn against the IAM service's state, require that it be assumable by the CloudWatch metric-streams service principal and carry firehose:PutRecord/PutRecordBatch on the target stream, and publish nothing when it is not.
Traces: the check id above.

=== Cause 2 of 4: no AWS/CloudWatch/MetricStreams metric series is ever published (MetricUpdate / TotalMetricUpdate / PublishErrorRate) ===
Failing checks: cloudwatch__firehose__s3.obs-cloudwatch-2-metric-update-counts-good-stream-while-untrusted-role-stream-reports-publish-errors, cloudwatch__firehose__s3.obs-cloudwatch-2-statistics-configuration-lifts-total-metric-update-and-stop-quiesces-stream.
AWS: a metric stream publishes its own operational metrics in the AWS/CloudWatch/MetricStreams namespace, dimensioned by MetricStreamName: MetricUpdate (one count per metric update streamed), TotalMetricUpdate (the count including the additional statistics a StatisticsConfigurations entry adds, hence > MetricUpdate once an AdditionalStatistics entry is in force — Smithy MetricStreamStatisticsConfiguration documents that "by default, a metric stream always sends the MAX, MIN, SUM, and SAMPLECOUNT statistics" and that IncludeMetrics/AdditionalStatistics add more), and PublishErrorRate (> 0 while the stream cannot publish, e.g. its role cannot be assumed). Recorded behavior for the series names/dimension.
Emulator: every read of those series comes back empty, for the healthy stream and the broken one alike: "MetricUpdate >= series count (filtered and aggregate) and untrusted PublishErrorRate > 0: not reached within 600.0s; last {'good MetricUpdate': [], 'all MetricUpdate': [], 'broken PublishErrorRate': [], 'good PublishErrorRate': []}" and "TotalMetricUpdate > MetricUpdate on <stream> after the StatisticsConfigurations change: not reached within 600.0s; last {'mu': {}, 'tot': {}, 'after': {}}". No datapoints at all, so neither the counting contract nor StopMetricStreams quiescing can be observed.
Why: these are the only metrics that tell an operator whether a stream is healthy and how much it is streaming. With the series absent, every metric-stream dashboard and alarm is permanently INSUFFICIENT_DATA on the emulator, and a publish failure (cause 1) is invisible from both ends.
Fix: the metric-stream publisher must emit AWS/CloudWatch/MetricStreams MetricUpdate/TotalMetricUpdate per streamed metric update (TotalMetricUpdate including the additional statistics named by StatisticsConfigurations) and PublishErrorRate when a publish attempt fails, dimensioned by MetricStreamName, into the CloudWatch metric store, and stop updating them once StopMetricStreams has quiesced the stream.
Traces: the two check ids above.

=== Cause 3 of 4: OutputFormat opentelemetry0.7/opentelemetry1.0 streams deliver nothing ===
Failing checks: cloudwatch__firehose__s3.deep-cloudwatch-3-cwstreamfmt-otel-delivers-length-delimited-protobuf-with-p99, cloudwatch__firehose__s3.obs-cloudwatch-2-opentelemetry1-stream-lands-otlp-protobuf-and-exclude-filter-drops-namespace.
AWS: Smithy MetricStreamOutputFormat is an enum with values json | opentelemetry0.7 | opentelemetry1.0, and MetricStreamStatisticsConfiguration.AdditionalStatistics documents that "if the OutputFormat for the stream is opentelemetry1.0 or opentelemetry0.7, the only valid values are p?? percentile statistics such as p90, p99". An OTel-format stream therefore delivers records to its Firehose destination exactly as a json stream does, with the payload being length-delimited OTLP ExportMetricsServiceRequest protobuf instead of newline-delimited JSON, carrying the base statistics plus the named percentile.
Emulator: the OTel legs deliver NOTHING while their json siblings deliver. "the OTel stream did not deliver both 'Latency' and 'Throughput' for namespace '<ns>' to s3://<bucket>/<otel-prefix>/ within 780s; objects [], metrics decoded []", and in the second check the otel leg's keys stay [] while the inc/exc json legs return full records ('otel keys': [], 'inc excl': [ {metric_stream_name, namespace, metric_name, dimensions, timestamp, value{max,min,sum,count}, unit} ... ]). The format is accepted at PutMetricStream and then never produces a record.
Why: OTel is the format every OpenTelemetry collector/consumer pipeline ingests. A client validating an OTLP pipeline against the emulator sees an empty destination forever, cannot test its protobuf decoder, and cannot verify that percentile statistics are carried.
Fix: the metric-stream publisher must serialize records for OutputFormat opentelemetry0.7/opentelemetry1.0 as length-delimited OTLP ExportMetricsServiceRequest protobuf and publish them to the destination stream the same way the json format is published; percentile AdditionalStatistics must appear in that payload.
Traces: the two check ids above.

=== Cause 4 of 4: no AWS/Firehose ingestion/delivery metric series is published for a delivery stream ===
Failing checks: cloudwatch__firehose__s3.metrics-states-stream-denied-role-delivery-metrics-stay-zero-control-matches-landed-bytes, cloudwatch__firehose__s3.incoming-records-and-delivery-to-s3-join-the-put-to-the-delivered-object.
AWS: a Firehose delivery stream publishes the AWS/Firehose namespace dimensioned by DeliveryStreamName: IncomingRecords (Unit Count) and IncomingBytes (Unit Bytes) for what PutRecord/PutRecordBatch accepted, DeliveryToS3.Records (Count) / DeliveryToS3.Bytes (Bytes) for what was written to S3, DeliveryToS3.Success (Maximum > 0 for a delivery that happened) and DeliveryToS3.DataFreshness (Seconds). Recorded behavior for the series/unit set. So one PutRecordBatch of N records totalling B bytes that lands in the bucket yields IncomingRecords Sum = N, IncomingBytes Sum = B, DeliveryToS3.Records Sum = N, DeliveryToS3.Bytes Sum = B; a stream whose destination role is denied still counts IncomingRecords = N while its DeliveryToS3.* stay zero.
Emulator: the series are empty/zero even though the objects are in the bucket. "AWS/Firehose sums settled at {'ok IncomingRecords': 0.0, 'deny IncomingRecords': 0.0, 'ok DeliveryToS3.Records': 0.0, 'ok DeliveryToS3.Bytes': 0.0, 'ok Success max': 0.0}; expected {'ok IncomingRecords': 7.0, 'deny IncomingRecords': 7.0, 'ok DeliveryToS3.Records': 7.0, 'ok DeliveryToS3.Bytes': 315.0}" — ingestion is not counted either, so the denied-role differential is ungradeable. The second check, whose terminal read is the same metric set (IncomingRecords/IncomingBytes/DeliveryToS3.Records settling on one value, then DeliveryToS3.Success, DeliveryToS3.DataFreshness, the per-metric Unit set and the ListMetrics dimension sets), never converged and hit the harness cap: "Check timed out after 900.0s".
Why: every Firehose dashboard, backlog alarm and capacity test reads these series. With them absent/zero a client cannot tell ingestion from delivery, cannot alarm on freshness, and sees a healthy stream and a broken one as identical.
Fix: the Firehose ingestion and delivery components must publish AWS/Firehose IncomingRecords/IncomingBytes on accepted puts and DeliveryToS3.Records/Bytes/Success/DataFreshness on each flush, with the documented Units (Count/Bytes/Seconds) and the DeliveryStreamName dimension, into the CloudWatch metric store so GetMetricStatistics and ListMetrics see them.
Traces: the two check ids above.

## Evidence
Replayed suite: simparity-behavioral-aws-v1 — cloudwatch__firehose__s3 4/11 (36%), 7 failing checks, none marked ALREADY FILED and none marked OWNED by an open issue.

Cause 1 — untrusted role still exports
Expected (AWS): a metric stream whose RoleArn is not assumable by the CloudWatch metric-streams principal publishes nothing; list_objects_v2 under its prefix returns no Contents.
Actual (emulator): "AssertionError: the metric stream whose role trusts 'ec2.amazonaws.com' still exported 2 object(s) to s3://<bucket>/<broken-prefix>/: ['<broken-prefix>/2026/11/09/13/<stream>-1-2026-11-09-13-05-18-0-<uuid>', '<broken-prefix>/2026/11/09/13/<stream>-1-2026-11-09-13-05-33-0-<uuid>']. CloudWatch cannot assume that role, so nothing may be delivered through it".

Cause 2 — AWS/CloudWatch/MetricStreams series absent
Expected (AWS): MetricUpdate >= the number of streamed series for the healthy stream, TotalMetricUpdate > MetricUpdate after a StatisticsConfigurations change adds percentiles, PublishErrorRate > 0 for the stream that cannot publish.
Actual (emulator): "AssertionError: MetricUpdate >= series count (filtered and aggregate) and untrusted PublishErrorRate > 0: not reached within 600.0s; last {'good MetricUpdate': [], 'all MetricUpdate': [], 'broken PublishErrorRate': [], 'good PublishErrorRate': []}" and "AssertionError: TotalMetricUpdate > MetricUpdate on <stream> after the StatisticsConfigurations change at <ts>: not reached within 600.0s; last {'mu': {}, 'tot': {}, 'after': {}}".

Cause 3 — OTel output formats deliver nothing
Expected (AWS): an opentelemetry1.0/0.7 stream lands length-delimited OTLP protobuf records in its destination prefix, including the p99 percentile for the metric named in StatisticsConfigurations.
Actual (emulator): "AssertionError: the OTel stream did not deliver both 'Latency' and 'Throughput' for namespace '<ns>' to s3://<bucket>/<otel-prefix>/ within 780s; objects [], metrics decoded []"; and "AssertionError: the otel stream landed the published metric and the inc/exc streams delivered in a shared metric minute: not reached within 600.0s; last {'otel keys': [], 'inc excl': [{'metric_stream_name': ..., 'namespace': ..., 'metric_name': 'ExcludedMetric', 'dimensions': {'Shard': 's0'}, 'timestamp': ..., 'value': {'max': 17.0, 'min': 17.0, 'sum': 17.0, 'count': 1.0}, 'unit': 'None'}, ...], 'exc json': [...]}" — the json legs deliver, the otel leg never does.

Cause 4 — AWS/Firehose ingestion/delivery metrics absent
Expected (AWS): for one PutRecordBatch of 7 records totalling 315 bytes that lands in S3: IncomingRecords Sum 7 on both the permitted and the denied stream, DeliveryToS3.Records Sum 7 and DeliveryToS3.Bytes Sum 315 on the permitted one, DeliveryToS3.Success Maximum > 0, DeliveryToS3.DataFreshness present with Unit Seconds.
Actual (emulator): "AssertionError: AWS/Firehose sums settled at {'ok IncomingRecords': 0.0, 'deny IncomingRecords': 0.0, 'ok DeliveryToS3.Records': 0.0, 'ok DeliveryToS3.Bytes': 0.0, 'ok Success max': 0.0}; expected {'ok IncomingRecords': 7.0, 'deny IncomingRecords': 7.0, 'ok DeliveryToS3.Records': 7.0, 'ok DeliveryToS3.Bytes': 315.0}". The companion check, whose terminal read is the same metric set, reported "Check timed out after 900.0s" without reaching a verdict.

## Steps to Reproduce
Common setup: boot the emulator, point botocore at it, s3.create_bucket(Bucket=B).

Cause 1:
1. iam.create_role(R_ok) trusting streams.metrics.cloudwatch.amazonaws.com with firehose:PutRecord/PutRecordBatch; iam.create_role(R_bad) with the same inline permissions but a trust policy naming ec2.amazonaws.com.
2. firehose.create_delivery_stream for two DirectPut/ExtendedS3 streams on B with prefixes good/ and broken/ (and a delivery role that can write B).
3. cloudwatch.put_metric_stream(Name=S_ok, FirehoseArn=<good stream>, RoleArn=R_ok, OutputFormat='json', IncludeFilters=[{'Namespace': NS}]) and the same for S_bad with the broken stream and R_bad.
4. cloudwatch.put_metric_data into NS repeatedly for a few minutes; poll s3.list_objects_v2(Bucket=B, Prefix='broken/').
5. Expect no Contents under broken/; observe exported objects.

Cause 2:
1. With S_ok streaming NS as above, poll cloudwatch.get_metric_statistics(Namespace='AWS/CloudWatch/MetricStreams', MetricName='MetricUpdate', Dimensions=[{'Name':'MetricStreamName','Value':S_ok}], Statistics=['Sum']).
2. Expect datapoints whose Sum covers the streamed series; observe an empty Datapoints list.
3. put_metric_stream again with StatisticsConfigurations=[{'IncludeMetrics':[{'Namespace':NS,'MetricName':M}],'AdditionalStatistics':['p99']}] and read TotalMetricUpdate — expect TotalMetricUpdate > MetricUpdate; observe both empty. Read PublishErrorRate for S_bad — expect > 0; observe empty.

Cause 3:
1. put_metric_stream(Name=S_otel, FirehoseArn=<otel stream>, RoleArn=R_ok, OutputFormat='opentelemetry1.0', IncludeFilters=[{'Namespace':NS}], StatisticsConfigurations=[{'IncludeMetrics':[{'Namespace':NS,'MetricName':'Latency'}],'AdditionalStatistics':['p99']}]).
2. put_metric_data for Latency and Throughput in NS for several metric minutes; poll the otel prefix.
3. Expect objects whose bytes are length-delimited OTLP protobuf; observe no objects at all while a json-format sibling stream delivers.

Cause 4:
1. Create a DirectPut/ExtendedS3 stream S on B with BufferingHints {'SizeInMBs':1,'IntervalInSeconds':60}; wait ACTIVE.
2. firehose.put_record_batch(DeliveryStreamName=S, Records=[7 records totalling 315 bytes]); wait until the bytes are readable from the bucket.
3. cloudwatch.get_metric_statistics(Namespace='AWS/Firehose', MetricName='IncomingRecords'|'IncomingBytes'|'DeliveryToS3.Records'|'DeliveryToS3.Bytes'|'DeliveryToS3.Success'|'DeliveryToS3.DataFreshness', Dimensions=[{'Name':'DeliveryStreamName','Value':S}]).
4. Expect 7 / 315 / 7 / 315, Success Maximum > 0, DataFreshness present with Unit Seconds, and ListMetrics reporting the ('DeliveryStreamName',) dimension set; observe zeros / no datapoints.

Finally re-replay the six check ids in trace_ids.
