Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies
Issue-Key: cloudwatch__sns__sqs|PutAnomalyDetector|NotImplemented
Issue-Commit: e0255819ca3e503154d6ac924edd987da058b9b5

# cloudwatch__sns__sqs: an ANOMALY_DETECTION_BAND alarm cannot exist — PutAnomalyDetector answers NotImplemented, so the math-form alarm never round-trips and its SNS notification is never delivered (1 cause)

> _Note: this issue was identified against the code base at commit `e0255819ca3e503154d6ac924edd987da058b9b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAnomalyDetector

## Description
Cause 1 of 1: the anomaly-detection alarm shape is unreachable because its detector cannot be created. Failing check: cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies. This check was NOT marked ALREADY FILED/OWNED by the replay report: the open stub issue 20261025T020208Z-cloudwatch-putanomalydetector-stub.md is scoped to service token `cloudwatch` and carries neither this check's replay key nor the alarm/notification semantics below, so it does not cover this cause.

AWS: Smithy: PutAnomalyDetector takes SingleMetricAnomalyDetector (or MetricMathAnomalyDetector), returns AnomalyDetectorId, and models InvalidParameterCombination / InvalidParameterValue / MissingParameter / LimitExceededException / InternalServiceError — a well-formed SingleMetricAnomalyDetector {Namespace, MetricName, Stat} is a success, and DescribeAnomalyDetectors then lists it for that Namespace+MetricName. PutMetricAlarm models `ThresholdMetricId` (MetricId, length 1-255) and a `Metrics` list of MetricDataQuery alongside the flat Namespace/MetricName/Statistic form: an anomaly-detection alarm is thresholded by ThresholdMetricId pointing at an `ANOMALY_DETECTION_BAND(m1, 2)` expression in `Metrics`, with ComparisonOperator LessThanLowerOrGreaterThanUpperThreshold and NO `Threshold`. Recorded behavior: that alarm is accepted; DescribeAlarms round-trips it in math form — `ThresholdMetricId` == 'ad1' and `Threshold` absent, not coerced to a static-threshold alarm; and when the alarm transitions (driven here by SetAlarmState) its AlarmActions fire to SNS and the delivered notification carries a `Trigger.Metrics` array instead of a static `Trigger.Threshold`.
Emulator: the very first call fails — ClientError (NotImplemented) on PutAnomalyDetector: "operation PutAnomalyDetector is scaffolded but not implemented in this emulator". Nothing after it runs, so the alarm shape, its round-trip and the notification representation are all unverifiable.
Why: a client that monitors with anomaly detection gets no detector, and every downstream consumer that parses the alarm notification is affected in a way no static-threshold check can reveal: consumers of an anomaly alarm must read `Trigger.Metrics`, and an emulator that accepts the alarm but emits the flat static-threshold Trigger (or silently flattens ThresholdMetricId into a Threshold) breaks them while appearing to work.
Fix: the PutAnomalyDetector handler and anomaly-detector state (plus DescribeAnomalyDetectors / DeleteAnomalyDetector over that state) in the cloudwatch service crate; PutMetricAlarm/DescribeAlarms must preserve `ThresholdMetricId` and the `Metrics` array without synthesising a `Threshold`; and the alarm action path's SNS notification serializer must emit `Trigger.Metrics` for a math/anomaly alarm. Note the alarm-action delivery path itself is separately open as 20261025T020208Z-cloudwatch-sns-sqs-cloudwatch-alarm-actions-never-fire-to-sns-so.md, which does not carry this check's replay key and covers only static-threshold alarms.

Traces: cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies.

## Evidence
Expected (AWS): cw.put_anomaly_detector(SingleMetricAnomalyDetector={'Namespace': <ns>, 'MetricName': 'AnomalyMetric', 'Stat': 'Average'}) -> 200; cw.describe_anomaly_detectors(Namespace=<ns>, MetricName='AnomalyMetric')['AnomalyDetectors'] non-empty; cw.put_metric_alarm(ComparisonOperator='LessThanLowerOrGreaterThanUpperThreshold', EvaluationPeriods=1, ThresholdMetricId='ad1', ActionsEnabled=True, AlarmActions=[<topic arn>], Metrics=[{'Id':'m1','MetricStat':{...},'ReturnData':True},{'Id':'ad1','Expression':'ANOMALY_DETECTION_BAND(m1, 2)','ReturnData':True}]) -> 200; cw.describe_alarms(AlarmNames=[alarm])['MetricAlarms'][0] has ThresholdMetricId == 'ad1' and Threshold is None; on the SetAlarmState transition the SNS->SQS notification body carries Trigger.Metrics and no static Trigger.Threshold.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the PutAnomalyDetector operation: operation PutAnomalyDetector is scaffolded but not implemented in this emulator. The check aborts on this first call, so no alarm is created and no notification is delivered.

## Steps to Reproduce
1. Create an SNS topic subscribed to an SQS queue (so the alarm notification is observable).
2. cw.put_anomaly_detector(SingleMetricAnomalyDetector={'Namespace': <fresh ns>, 'MetricName': 'AnomalyMetric', 'Stat': 'Average'}) -> emulator answers NotImplemented; AWS answers 200.
3. cw.describe_anomaly_detectors(Namespace=<ns>, MetricName='AnomalyMetric') must list the detector.
4. cw.put_metric_alarm with ComparisonOperator='LessThanLowerOrGreaterThanUpperThreshold', EvaluationPeriods=1, ThresholdMetricId='ad1', ActionsEnabled=True, AlarmActions=[topic arn], and Metrics=[{'Id':'m1','MetricStat':{'Metric':{'Namespace':<ns>,'MetricName':'AnomalyMetric'},'Period':60,'Stat':'Average'},'ReturnData':True},{'Id':'ad1','Expression':'ANOMALY_DETECTION_BAND(m1, 2)','ReturnData':True}].
5. cw.describe_alarms(AlarmNames=[alarm]) -> ThresholdMetricId must be 'ad1' and Threshold must be absent.
6. cw.set_alarm_state to drive the transition, then read the SQS queue -> the notification must carry Trigger.Metrics (the math form), not a static Trigger.Threshold.
7. Tear down LIFO (delete the alarm before the detector; CloudWatch refuses to delete a detector still referenced by an alarm).
8. Re-replay: cloudwatch__sns__sqs.anomaly-detection-band-alarm-action-notifies.
