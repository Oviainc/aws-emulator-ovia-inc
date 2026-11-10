Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns
Issue-Key: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns|run#33|Timeout|time.sleep(15)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# A REST API 5xx alarm never transitions OK -> ALARM, so the SNS -> SQS notification never arrives (check exhausts its 600s budget)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: an alarm over a REST API stage's AWS/ApiGateway 5xx series never leaves OK, so no alarm action is ever executed.
AWS: a REST API stage with metrics enabled publishes 5xx datapoints under {ApiName, Stage} (and per-method series when detailed metrics are on). An alarm over that series with Threshold < the number of 500s served crosses into ALARM within a couple of evaluation periods, executes its AlarmActions (an SNS publish) and the subscribed SQS queue receives one notification message. Recorded behavior only for the emission side; the alarm transition and action execution are the recorded outcome.
Emulator: the check drives a burst of integration 500s through the stage and then polls for the OK -> ALARM transition and the SQS delivery until its 600s budget is exhausted — the transition never happens, so the run is reported as a failure with no further diagnosis than the expired budget. This is the same silent-metrics family the suite reports for every other AWS/ApiGateway series in this project.
Why: a client that pages on API 5xx rate gets no page; the entire alarm -> SNS -> SQS chain is dead for API Gateway metrics even though each component answers its own control-plane calls.
Fix: the apigateway data plane (services/service-com-amazonaws-apigateway-backplanecontrolservice) must publish the stage's 5xx/Count datapoints into the cloudwatch metric store, and the cloudwatch alarm evaluator (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) must then transition and fire the configured action.
Why this is not covered by an existing issue: the open issue 20261109T152222Z-apigateway-cloudwatch-lambda-updatestage-refuses-a-real-method.md carries the other failures of this heading but not this replay key, and 20261109T152222Z-apigateway-cloudwatch-aws-apigateway-count-metrics-are-never.md is filed under a different service heading.
Traces: apigateway__cloudwatch__lambda.obs-apigateway-1-5xx-alarm-ok-to-alarm-notifies-sqs-via-sns.

## Evidence
Expected: after the burst of integration 500s, DescribeAlarms reports StateValue 'ALARM' for the 5xx alarm and the SQS queue subscribed to the alarm's SNS topic receives the notification; actual: 'Error: Check timed out after 600.0s' — the alarm never left OK within the check's own 600s deadline and no notification was delivered.

## Steps to Reproduce
1. Create a REST API with a Lambda integration that returns 500, deploy a stage with metrics enabled.
2. PutMetricAlarm on AWS/ApiGateway 5xx, Dimensions=[{ApiName},{Stage}], Statistic Sum, Period 60, EvaluationPeriods 1, Threshold below the burst size, AlarmActions=[SNS topic ARN with an SQS subscription].
3. Issue a burst of requests that the integration answers 500.
4. Poll DescribeAlarms for StateValue=ALARM and ReceiveMessage on the queue -> neither ever happens.
