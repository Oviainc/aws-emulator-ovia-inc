Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies cloudwatch.metric-stream-server-side-guarantees-delivery-ordering-per-metric
Issue-Key: cloudwatch|PutMetricAlarm|ValidationError
Issue-Key: cloudwatch.metric-stream-server-side-guarantees-delivery-ordering-per-metric|_prime_pipe#15|Timeout|time.sleep(interval_s)
Issue-Commit: ebfe4f059696b431f00c6abb397dd11b94b7073a

# cloudwatch: 2 net-new root causes — PutMetricAlarm refuses every anomaly-detection-band alarm (ThresholdMetricId -> ANOMALY_DETECTION_BAND) as "more than one time series", and a managed metric stream never delivers its datapoints to the Firehose->S3 destination

> _Note: this issue was identified against the code base at commit `ebfe4f059696b431f00c6abb397dd11b94b7073a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Two distinct root causes in the CloudWatch emulator, both grounded in failing behavioral checks that no open issue carries the replay key for. A fixer must close BOTH before submitting.

Cause 1 of 2: PutMetricAlarm rejects an anomaly-detection alarm whose ThresholdMetricId names an ANOMALY_DETECTION_BAND query.
  AWS: an anomaly-detection alarm is the modelled shape where the static Threshold is replaced by ThresholdMetricId, which points at a query in the Metrics array whose Expression is ANOMALY_DETECTION_BAND(...). Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801 model): PutMetricAlarmInput.ThresholdMetricId -> MetricId, @length min 1 max 255, documented as "If this is an alarm based on an anomaly detection model, make this value match the ID of the ANOMALY_DETECTION_BAND function." PutMetricAlarmInput also carries Metrics -> MetricDataQueries and Threshold -> double, and the only modelled error is LimitExceededFault (awsQueryError LimitExceeded, HTTP 400); nothing in the model declares a band expression invalid. The band query legitimately returns TWO series (the Low and High bands) — that is the whole point of the function, and the single-time-series rule AWS enforces applies to the query the alarm WATCHES (the ReturnData metric/expression), not to the query named by ThresholdMetricId. AWS accepts the call with HTTP 200 and an empty body, and DescribeAlarms then round-trips ThresholdMetricId, omits Threshold, and returns both Metrics entries verbatim.
  Emulator: the call is refused. HTTP 400, error code `ValidationError`, message `the Expression of query ad1 returns more than one time series, and the expression an alarm watches must return a single time series`. Fixed text of that message is "the Expression of query " + ", returns more than one time series, and the expression an alarm watches must return a single time series"; the only variable part is the query id, taken from the request's MetricDataQueries[].Id (here `ad1`). The emulator applies the single-series rule to EVERY query in the Metrics array instead of only to the alarm's watched query, so the ThresholdMetricId query can never be a band.
  Why: anomaly-detection alarms are unusable end to end. A client cannot create one at all, so it cannot observe band evaluation, cannot see ThresholdMetricId/Metrics round-trip in DescribeAlarms, and never receives the math-form notification payload (Trigger.Metrics rather than Trigger.Threshold) that downstream consumers parse. Every detector-plus-alarm workflow — including the documented "delete the detector and the live alarm re-creates it" behavior — aborts on the first write.
  Fix: PutMetricAlarm's validation in the cloudwatch service crate — the single-time-series check must be scoped to the watched query and must exempt the query referenced by ThresholdMetricId (and ANOMALY_DETECTION_BAND expressions generally); the alarm store and DescribeAlarms serializer must keep ThresholdMetricId and the full Metrics array and leave Threshold absent.
  Traces: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band (error, PutMetricAlarm); cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies (error, identical refusal on the same shape — same cause, reached from the alerting path).

Cause 2 of 2: a running managed metric stream delivers no datapoints to its Firehose->S3 destination, so per-metric delivery ordering can never be observed.
  AWS: recorded behavior only (the Smithy model describes PutMetricStream/StartMetricStreams configuration, not delivery). The recording shows: after PutMetricStream(OutputFormat="json", IncludeFilters=[{Namespace: ns}]) plus StartMetricStreams, GetMetricStream reports State "running", and each subsequently published datapoint is exported to the Firehose destination and lands in S3 as one JSON record per metric/period, with records for a single metric arriving in the same chronological order they were published (record timestamps non-decreasing, value.sum following the publish order).
  Emulator: nothing is ever observed in the destination bucket. The check never reaches its ordering assertion and the replay reports `Check timed out after 60.0s`. Note for the fixer: this check's own scenario mandates >=130s of wall clock (two 65s waits so each datapoint lands in its own 60s period) plus up to 300s of S3 polling, which is already longer than the suite's 60s per-check budget — so the timeout alone does not localise the failure. Hypothesis: the check blocks in its priming loop, which republishes a throwaway metric until the first record is demonstrably present in S3, because the emulator's metric-stream export path never writes any record (the same silence already filed for a different service token as 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md, which does not carry this check's replay key).
  Why: a client that builds on metric streams gets a stream that reports State "running" and then exports nothing, so no consumer downstream of Firehose ever sees a metric, and ordering/latency guarantees cannot be relied on at all.
  Fix: the metric-stream delivery path in the cloudwatch service crate (the export side that PutMetricStream/StartMetricStreams arms, and the per-metric ordering of the records it emits to the configured Firehose ARN).
  Traces: cloudwatch.metric-stream-server-side-guarantees-delivery-ordering-per-metric (mismatch).

## Evidence
Cause 1 of 2 — expected vs actual.
  Expected (AWS): PutMetricAlarm(AlarmName=<name>, ComparisonOperator=LessThanLowerOrGreaterThanUpperThreshold | GreaterThanUpperThreshold, EvaluationPeriods=1, ThresholdMetricId="ad1", Metrics=[{Id:"m1", MetricStat:{Metric:{Namespace:<ns>, MetricName:<metric>}, Period:60, Stat:"Average"}, ReturnData:true}, {Id:"ad1", Expression:"ANOMALY_DETECTION_BAND(m1, 2)", ReturnData:true}]) -> HTTP 200, empty body. DescribeAlarms then reports ThresholdMetricId=="ad1", no Threshold member, and Metrics ids {m1, ad1} with ad1.Expression=="ANOMALY_DETECTION_BAND(m1, 2)" verbatim.
  Actual (emulator): HTTP 400 `ValidationError` — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the PutMetricAlarm operation: the Expression of query ad1 returns more than one time series, and the expression an alarm watches must return a single time series. Reproduced identically from two independent checks (one driving the in-band/out-of-band evaluator path, one driving the SNS->SQS action path), both aborting on this first PutMetricAlarm call.

Cause 2 of 2 — expected vs actual.
  Expected (AWS, recorded): GetMetricStream(Name=<stream>) -> State "running"; then for publishes of 100.0, 200.0, 300.0 in three consecutive 60s periods, the Firehose->S3 destination contains three records for that metric whose value.sum sequence equals the send order and whose timestamps are chronologically sorted.
  Actual (emulator): no record for the metric (nor for the priming metric) is ever observed in the destination bucket; the check does not complete and is reported as `Check timed out after 60.0s`, so neither the delivery assertion nor the ordering assertion is ever reached.

## Steps to Reproduce
Cause 1 of 2:
  1. cloudwatch.put_metric_data for namespace NS, metric "X", enough history to train (the checks publish 90 points at 2-minute spacing).
  2. cloudwatch.put_anomaly_detector(SingleMetricAnomalyDetector={Namespace: NS, MetricName: "X", Stat: "Average"}) and confirm it via describe_anomaly_detectors.
  3. cloudwatch.put_metric_alarm(AlarmName=A, ComparisonOperator="GreaterThanUpperThreshold" (or "LessThanLowerOrGreaterThanUpperThreshold"), EvaluationPeriods=1, ThresholdMetricId="ad1", Metrics=[{Id:"m1", MetricStat:{Metric:{Namespace:NS, MetricName:"X"}, Period:60, Stat:"Average"}, ReturnData:true}, {Id:"ad1", Expression:"ANOMALY_DETECTION_BAND(m1, 2)", ReturnData:true}]).
  4. Observe HTTP 400 ValidationError "the Expression of query ad1 returns more than one time series, ..." instead of HTTP 200; AWS accepts, and describe_alarms round-trips ThresholdMetricId with no Threshold.
  Replay: cloudwatch.alarm-math-and-anomaly-band-alarm-survives-detector-delete-and-fires-out-of-band and cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies.

Cause 2 of 2:
  1. Create an S3 bucket, a Firehose delivery stream writing to it, and the metric-stream role.
  2. cloudwatch.put_metric_stream(Name=S, FirehoseArn=..., RoleArn=..., OutputFormat="json", IncludeFilters=[{Namespace: NS}]) then cloudwatch.start_metric_streams(Names=[S]); confirm get_metric_stream reports State "running".
  3. put_metric_data one priming datapoint in NS and poll S3 until a record appears — on AWS it appears; on the emulator nothing ever does.
  4. Publish 100.0, 200.0, 300.0 for one metric, >60s apart, then read every delivered record for that metric from S3 and compare value.sum order and timestamp order to the send order.
  Replay: cloudwatch.metric-stream-server-side-guarantees-delivery-ordering-per-metric (allow the check its full declared budget; its own scenario needs >=130s of wall clock, more than the suite's 60s per-check cap).
