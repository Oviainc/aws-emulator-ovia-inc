Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive cloudwatch__lambda__logs.cloudwatch-composite-alarm-triggers-lambda-positive cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive
Issue-Key: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative|_poll_until#27|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive|_poll_until#21|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cloudwatch-composite-alarm-triggers-lambda-positive|_poll_until#26|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive|_poll_until#9|AssertionError|raise AssertionError(
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 2 net-new causes: alarm state changes never invoke their Lambda target, and a CFN-declared function's log group is never created

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Two net-new root causes. The open issue 20261105T111951Z-cloudwatch-lambda-logs-5-net-new-root-causes.md covers other checks in this service; neither cause below carries its replay key.

Cause 1 of 2 — a CloudWatch alarm state change never reaches its Lambda target.
AWS: recorded behavior only. When a metric alarm (or a composite alarm) transitions to ALARM, CloudWatch delivers the state change to its configured action — either a Lambda alarm action or an EventBridge rule on `aws.cloudwatch` `CloudWatch Alarm State Change` whose target is a function — and the function receives a payload naming the alarm. The recordings poll the target function's log group for a marker containing the alarm name. The missing-permission leg expects the invocation to be refused when the function grants the caller no lambda:InvokeFunction.
Emulator: no delivery ever happens, on any of the three legs. Replay details: `AssertionError: marker containing 'sim2real-alarm-0741db768d76' delivered to sim2real-lgcwlam-fn-... not met within 120s. Last result: []`, and the same for `'sim2real-alarm-a22b53b570fd'` and the composite alarm's `'sim2real-composite-f1e78d85cb70'`.
Why: alarm-driven automation is inert. A client wiring remediation to an alarm never runs it, and because the positive path never fires, the negative test (invocation refused without a grant) passes vacuously — a real missing-permission defect would be invisible.
Fix: the CloudWatch alarm state-evaluation and action dispatch path — delivery of a state transition to Lambda alarm actions and to EventBridge (`CloudWatch Alarm State Change`) for rule-based fan-out, including composite alarms, with the target function's resource policy evaluated on dispatch.
Traces: cloudwatch__lambda__logs.cloudwatch-alarm-missing-lambda-permission-negative, cloudwatch__lambda__logs.cloudwatch-alarm-state-change-eventbridge-invokes-lambda-positive, cloudwatch__lambda__logs.cloudwatch-composite-alarm-triggers-lambda-positive.

Cause 2 of 2 — a CloudFormation-created function's log group never exists, so its retention cannot be read.
AWS: recorded behavior only. A stack that declares a function (and an AWS::Logs::LogGroup with RetentionInDays for it) leaves `/aws/lambda/<function>` present after CREATE_COMPLETE, and DescribeLogGroups reports the declared retention.
Emulator: the group never appears. Replay detail: `AssertionError: log group /aws/lambda/sim2real-xsvc-fn-346fa553541e to exist not met within 60s. Last result: None`.
Why: a client that sets log retention through a template cannot verify or rely on it — the group it declared does not exist, so retention is unset and any subscription filter or metric filter attached to it fails to resolve.
Fix: the CloudFormation provisioning of the Lambda function / AWS::Logs::LogGroup pair — creation of the function's default log group and application of the declared RetentionInDays.
Traces: cloudwatch__lambda__logs.cfn-deploy-lambda-loggroup-retention-positive.

## Evidence
Cause 1 — expected the alarm's state change to invoke the target function (marker containing the alarm name appears in the function's log group); actual `Last result: []` after 120 s on the metric-alarm, EventBridge-rule and composite-alarm legs.
Cause 2 — expected log group /aws/lambda/<function> to exist after the stack reaches CREATE_COMPLETE with the declared RetentionInDays; actual `Last result: None` after 60 s.

## Steps to Reproduce
Cause 1: PutMetricAlarm with a Lambda alarm action (and separately an EventBridge rule on source aws.cloudwatch, detail-type "CloudWatch Alarm State Change", target = function); drive the metric into ALARM with SetAlarmState/PutMetricData; poll the function's log group — no invocation marker arrives. Repeat with PutCompositeAlarm.
Cause 2: CreateStack with AWS::Lambda::Function plus AWS::Logs::LogGroup(/aws/lambda/<fn>, RetentionInDays=N); after CREATE_COMPLETE call DescribeLogGroups — the group is absent.
