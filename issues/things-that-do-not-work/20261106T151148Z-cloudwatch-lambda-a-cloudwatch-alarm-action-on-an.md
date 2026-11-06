Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied|_warm_up#8|Timeout|time.sleep(5.0)
Issue-Commit: 5678b2d659e23eeff4b2f39316abc675f4291395

# cloudwatch__lambda: a CloudWatch alarm action on an un-permitted Lambda never settles, so the denial is unobservable

> _Note: this issue was identified against the code base at commit `5678b2d659e23eeff4b2f39316abc675f4291395`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
One root cause behind 1 failing check.

Cause 1 of 1: an alarm whose action targets a Lambda that has no `lambda:InvokeFunction` resource-policy grant for cloudwatch.amazonaws.com never produces an observable outcome.
AWS: when the alarm transitions to ALARM, CloudWatch attempts the alarm action; without a resource policy admitting the `cloudwatch.amazonaws.com` (or `lambda.alarms.cloudwatch.amazonaws.com`) principal the invoke is refused and the function does not run — the function's log group stays empty and no invocation is recorded — while the twin with the grant does run. Both outcomes are reachable within seconds. Model: PutMetricAlarm with AlarmActions and Lambda's AddPermission/RemovePermission resource policy are modelled (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801, services/service-com-amazonaws-lambda-awsgirapiservice); the alarm-action authorization semantics here are recorded behavior only.
Emulator: `Error: Check timed out after 60.0s` — the alarm/action path yields no verdict at all, so neither the refusal nor the positive control can be graded.
Why: an operator cannot tell a missing permission from a broken alarm; an alarm-driven remediation function appears neither invoked nor denied, so a misconfigured production alarm stays silent and undiagnosable.
Fix: the CloudWatch alarm state-transition / alarm-action dispatcher and the Lambda resource-policy authorization it must consult. The open issue 20261105T111951Z-cloudwatch-lambda-aws-lambda-duration-statistics-never-appear.md owns the sibling alarm-payload check; it does not carry this replay key.
Traces: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied.

## Evidence
Expected (AWS): on the ALARM transition the un-granted function is not invoked (empty log group, no invocation record) while the granted twin is invoked — both observable within seconds.

Actual (emulator): cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied -> Error: Check timed out after 60.0s.

## Steps to Reproduce
1. PutMetricAlarm with an AlarmAction naming a Lambda function ARN; grant no lambda:InvokeFunction to cloudwatch.amazonaws.com on it, and create a twin function that does have the grant.
2. Drive the metric so the alarm enters ALARM.
3. Poll both functions for invocation evidence. Observed: no verdict within 60 s for either.
