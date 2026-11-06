Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm
Issue-Key: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm|_poll_until#11|Timeout|return last
Issue-Commit: 216f714d250c6d780581584a9f6355ede0acb526

# cloudwatch__dynamodb__lambda: an alarm on AWS/Lambda Errors never leaves its initial state after failing invocations, so the Errors-rate alarm lifecycle never settles

> _Note: this issue was identified against the code base at commit `216f714d250c6d780581584a9f6355ede0acb526`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: a metric alarm created on the AWS/Lambda `Errors` series of a function, dimensioned FunctionName, never transitions to ALARM after that function's invocations fail, and no corresponding StateUpdate entry ever appears in its alarm history. The check polls DescribeAlarms for the alarm's StateValue and is still polling when the harness cap (60s) expires, so the scenario never reaches a verdict. Failing check: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm.

AWS: Smithy (cloudwatch, com.amazonaws.cloudwatch#GraniteServiceVersion20100801) PutMetricAlarm takes Namespace (@length 1..255, @pattern ^[^:]), MetricName, Dimensions (@length 0..30), Statistic, Period (@range min 1), EvaluationPeriods (@range min 1), Threshold, ComparisonOperator and TreatMissingData, and answers an empty output (Unit) with only LimitExceededFault modeled; DescribeAlarms -> DescribeAlarmsOutput{MetricAlarms, CompositeAlarms, LogAlarms, NextToken} and DescribeAlarmHistory -> DescribeAlarmHistoryOutput{AlarmHistoryItems, NextToken}. The model describes only those shapes; that the AWS/Lambda namespace is populated with a per-function `Errors` datum per failed invocation, and that CloudWatch then evaluates the alarm over it, is recorded behavior only. Recorded behavior: after three Invoke calls that each answer FunctionError='Unhandled', the AWS/Lambda Errors series for FunctionName=<function> carries datapoints whose Sum over the alarm Period is >= the Threshold, so an alarm with Statistic=Sum, Period=60, EvaluationPeriods=1, Threshold=1, ComparisonOperator=GreaterThanOrEqualToThreshold and TreatMissingData=notBreaching reaches StateValue='ALARM', and DescribeAlarmHistory(HistoryItemType='StateUpdate') then contains an item whose HistoryData JSON has newState.stateValue == 'ALARM'. Every concrete value here — the alarm name, the function name, the datapoint timestamps and the Sum — is filled in from the request and the emulator's own invocation history; the requirement is the transition and the history entry, not any one run's numbers.

Emulator: PutMetricAlarm is accepted and the three Invokes do report FunctionError='Unhandled', but DescribeAlarms keeps answering the alarm in its pre-transition state for as long as it is polled; the replay ends with 'Check timed out after 60.0s' while still waiting for StateValue=='ALARM', so the subsequent DescribeAlarmHistory assertion (an item with newState.stateValue=='ALARM') is never even reached. Nothing in the envelope is wrong — the alarm is stored and read back correctly; there is simply no AWS/Lambda Errors metric data behind it and/or no server-side evaluation that would move it.

Why: a client that wires alarms to Lambda failure signals — error-rate alarms, deployment canaries, retry/rollback automation, dashboards, any 'wait until the Errors alarm fires' loop — sees a function that failed three times as a function with no errors and an alarm that is never breaching. Against AWS the alarm flips within an evaluation period or two; against the emulator the client spins until its own timeout and then either fails or wrongly concludes the invocations succeeded. Alarm actions keyed on AWS/Lambda Errors can therefore never fire at all.

Fix: the metric-publication side of the Lambda invoke path plus CloudWatch's alarm-evaluation and alarm-history state. The invoke handler must publish the per-invocation AWS/Lambda datums (at minimum `Errors`, dimensioned FunctionName, Unit Count) into the same metric store the alarm evaluator and GetMetricStatistics/GetMetricData read, and the alarm evaluator must re-evaluate stored metric alarms against that series, write the resulting StateValue through DescribeAlarms and append a StateUpdate item whose HistoryData carries newState.stateValue.

Related open issue, not a duplicate: 20261106T132053Z-monitoring-lambda-invocation-errors-metric-never-lands-in-cloudwatch.md reports the same missing AWS/Lambda Errors data from the wire corpus as seen through GetMetricStatistics (service token `monitoring`, key monitoring|GetMetricStatistics|2xx->2xx). The replay did not mark this check as owned by it, and this check additionally covers the alarm-evaluation and DescribeAlarmHistory half of the behavior, which that issue does not assert.

Traces: failing check cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm (verdict mismatch, 'Check timed out after 60.0s').

## Evidence
Failing behavioral check: cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm — verdict [mismatch], detail: `Check timed out after 60.0s`.

The check (GROUP 'alarm-state', SERVICES ['cloudwatch','dynamodb','lambda']) does:
  1. create an execution role + a python function whose handler raises RuntimeError('boom');
  2. cw.put_metric_alarm(AlarmName=<generated>, Namespace='AWS/Lambda', MetricName='Errors', Dimensions=[{'Name':'FunctionName','Value':<function>}], Statistic='Sum', Period=60, EvaluationPeriods=1, Threshold=1, ComparisonOperator='GreaterThanOrEqualToThreshold', TreatMissingData='notBreaching');
  3. three lam.invoke() calls, each asserted to report FunctionError='Unhandled' (this assertion passes against the emulator);
  4. poll cw.describe_alarms(AlarmNames=[alarm])['MetricAlarms'][0]['StateValue'] until it equals 'ALARM';
  5. assert cw.describe_alarm_history(AlarmName=alarm, HistoryItemType='StateUpdate') contains an item whose json.loads(HistoryData)['newState']['stateValue'] == 'ALARM'.

Expected (AWS): step 4 converges on StateValue='ALARM' and step 5 finds the matching StateUpdate item.
Actual (emulator): step 4 never converges — DescribeAlarms keeps returning the alarm un-transitioned and the run is killed at the 60s harness cap; step 5 is never reached. The check's own in-script message for this situation is 'alarm never transitioned to ALARM within 6 minutes'.

Status codes and response envelopes are correct throughout (PutMetricAlarm, Invoke and DescribeAlarms all answer 2xx with well-formed bodies); the divergence is missing AWS/Lambda Errors metric data and missing alarm evaluation over it, not a protocol or serializer defect. Alarm name, function name, timestamps and datapoint sums are per-run values and are not part of the requirement.

## Steps to Reproduce
1. Replay the behavioral suite and watch check `cloudwatch__dynamodb__lambda.cloudwatch-alarm-on-lambda-error-rate-transitions-alarm`.
2. Equivalently, by hand against the emulator: create an IAM role trusted by lambda.amazonaws.com with AWSLambdaBasicExecutionRole, then create a python function whose handler raises RuntimeError('boom').
3. PutMetricAlarm with Namespace='AWS/Lambda', MetricName='Errors', Dimensions=[{Name:'FunctionName', Value:<function>}], Statistic='Sum', Period=60, EvaluationPeriods=1, Threshold=1, ComparisonOperator='GreaterThanOrEqualToThreshold', TreatMissingData='notBreaching'.
4. Invoke the function three times; confirm each response carries FunctionError='Unhandled'.
5. Poll DescribeAlarms(AlarmNames=[<alarm>]) and read MetricAlarms[0].StateValue. Observe it never becomes 'ALARM' however long it is polled.
6. Call DescribeAlarmHistory(AlarmName=<alarm>, HistoryItemType='StateUpdate') and observe no item whose HistoryData JSON has newState.stateValue == 'ALARM'.
7. On AWS the alarm reaches ALARM once the failed-invocation datapoints land, and the StateUpdate history item is present.
