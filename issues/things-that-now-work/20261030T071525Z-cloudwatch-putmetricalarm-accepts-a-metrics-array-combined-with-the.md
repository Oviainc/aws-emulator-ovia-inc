Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs|_error#32|AssertionError|raise AssertionError(
Issue-Commit: c06c05ee376191c16b1b27ede6b3dd133dd3f300

# PutMetricAlarm accepts a Metrics array combined with the scalar metric parameters (Namespace/MetricName/Statistic/Period) instead of refusing with ValidationError

> _Note: this issue was identified against the code base at commit `c06c05ee376191c16b1b27ede6b3dd133dd3f300`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One net-new root cause in cloudwatch, grounded in the behavioral check cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs. (Related open issues that do NOT cover this cause: 20261026T031713Z-cloudwatch-2-net-new-root-causes-putmetricalarm-refuses.md covers ANOMALY_DETECTION_BAND/ThresholdMetricId alarms being refused; 20261029T182452Z-cloudwatch-request-validation-and-error-envelope-divergences-10.md covers other request-validation families. Neither names the Metrics-vs-scalar mutual exclusion.)

Cause 1 of 1: PutMetricAlarm does not enforce the documented mutual exclusion between the `Metrics` array and the scalar metric-identity parameters.

AWS: the Smithy model for PutMetricAlarm documents, on PutMetricAlarmInput$Metrics: "For each PutMetricAlarm operation, you must specify either MetricName, a Metrics array, or an EvaluationCriteria" and "If you use the Metrics parameter, you cannot include the Namespace, MetricName, Dimensions, Period, Unit, Statistic, or ExtendedStatistic parameters of PutMetricAlarm in the same operation." A request that supplies both forms of metric identity in one call is therefore refused before any alarm is created. Recorded behavior: the refusal is an HTTP 400 with error code `ValidationError` and message `MetricName should not be set if list of Metrics is set.` In that message the fixed text is the whole sentence; its only variable part is the member name (`MetricName`) — the member of the request that was supplied alongside `Metrics` and must not have been (the same rule applies to Namespace, Dimensions, Period, Unit, Statistic and ExtendedStatistic, each named in its own refusal). No alarm with the requested AlarmName exists afterwards: a subsequent DescribeAlarms for that name returns an empty MetricAlarms list.

Emulator: the same call — AlarmName set, `Metrics` set to a three-item array (two MetricStat source queries with ReturnData=false plus one Expression `(errors / requests) * 100` with ReturnData=true), AND `Namespace`, `MetricName`, `Statistic=Maximum`, `Period=60` all set — SUCCEEDS. The replay recorded HTTP 200 with an empty JSON body (`content-type: application/x-amz-json-1.0`, `content-length: 2`) and no error at all: `AssertionError: real AWS refuses Metrics combined with Namespace/MetricName/Statistic/Period, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. So the validation step that should reject the combination is absent; the alarm is accepted and persisted with two contradictory metric identities.

Why: a client relying on AWS's behavior gets a silent misconfiguration instead of a 400. Infrastructure tooling (CDK/CloudFormation/Terraform and hand-rolled SDK callers) depends on this refusal to catch templates that mix a metric-math alarm with a scalar metric definition; against the emulator such a template deploys "successfully" and leaves an alarm whose watched series is ambiguous, so the alarm's later state is unpredictable and negative tests that assert the rejection (and assert no alarm was created) pass against AWS but fail here. The call also leaks a resource the client believes was never created.

Fix: belongs in PutMetricAlarm's request validation in the cloudwatch service — services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/src/ops/put_metric_alarm.rs (the input-validation stage that runs before the alarm is written to state). It must reject any request that carries `Metrics` together with any of Namespace, MetricName, Dimensions, Period, Unit, Statistic or ExtendedStatistic, with HTTP 400 / code `ValidationError` and the message naming the offending member, and must not create or modify any alarm on that path.

Traces: cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs (mismatch; fails at the first negative leg, after the positive metric-math alarm legs have already passed).

## Evidence
Expected (real AWS): PutMetricAlarm(AlarmName=<name>, Metrics=[{Id:errors,MetricStat:{...},ReturnData:false},{Id:requests,MetricStat:{...},ReturnData:false},{Id:rate,Expression:"(errors / requests) * 100",ReturnData:true}], Namespace=<ns>, MetricName="Errors", Statistic="Maximum", Period=60, Threshold=50.0, ComparisonOperator="GreaterThanThreshold", EvaluationPeriods=1) is refused: HTTP 400, Error.Code = "ValidationError", Error.Message = "MetricName should not be set if list of Metrics is set." and DescribeAlarms(AlarmNames=[<name>], AlarmTypes=["MetricAlarm"]) returns [].

Actual (emulator): HTTP 200, empty body, no error. Replay detail:
  File "check_cloudwatch_metric_math_alarm_evaluates_the_computed_expression_not_its_inputs.py", line 264, in run
    combo_error = _error(...)
  File "...", line 400, in _error
    raise AssertionError(
  AssertionError: real AWS refuses Metrics combined with Namespace/MetricName/Statistic/Period, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Model citation: Smithy PutMetricAlarmInput$Metrics documentation — "If you use the Metrics parameter, you cannot include the Namespace, MetricName, Dimensions, Period, Unit, Statistic, or ExtendedStatistic parameters of PutMetricAlarm in the same operation."

## Steps to Reproduce
1. Against the emulator's cloudwatch endpoint, PutMetricData into a fresh custom namespace NS with metrics `Errors` and `Requests` (Unit=Count) so the sources exist.
2. Call PutMetricAlarm with BOTH forms of metric identity in one request:
   - AlarmName="mathalarm-rejected"
   - Metrics=[{"Id":"errors","MetricStat":{"Metric":{"Namespace":NS,"MetricName":"Errors"},"Period":60,"Stat":"Maximum"},"ReturnData":false},{"Id":"requests","MetricStat":{"Metric":{"Namespace":NS,"MetricName":"Requests"},"Period":60,"Stat":"Maximum"},"ReturnData":false},{"Id":"rate","Expression":"(errors / requests) * 100","Label":"ErrorRatePercent","ReturnData":true}]
   - Namespace=NS, MetricName="Errors", Statistic="Maximum", Period=60
   - Threshold=50.0, ComparisonOperator="GreaterThanThreshold", EvaluationPeriods=1
3. Observe the response: the emulator returns HTTP 200 with an empty body instead of HTTP 400 ValidationError "MetricName should not be set if list of Metrics is set."
4. Call DescribeAlarms(AlarmNames=["mathalarm-rejected"], AlarmTypes=["MetricAlarm"]) — the emulator reports the alarm as created, where AWS reports no such alarm.
5. Replay the check to confirm: cloudwatch.metric-math-alarm-evaluates-the-computed-expression-not-its-inputs
