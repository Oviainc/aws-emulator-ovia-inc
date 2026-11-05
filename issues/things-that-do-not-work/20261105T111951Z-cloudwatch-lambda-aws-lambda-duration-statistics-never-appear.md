Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied
Issue-Key: cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function|_warm_up#8|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=json.dumps(event).encode())
Issue-Key: cloudwatch__lambda.xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied|_warm_up#8|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=json.dumps(event).encode())
Issue-Commit: a0ca0e6d405fa8f821a988e0d013d6063af395f8

# cloudwatch__lambda: AWS/Lambda Duration statistics never appear and an alarm action never invokes its Lambda target

> _Note: this issue was identified against the code base at commit `a0ca0e6d405fa8f821a988e0d013d6063af395f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1 — the AWS/Lambda metric series for an invoked function is never populated, and a CloudWatch alarm whose AlarmAction is a Lambda function never invokes it, so all three checks poll to their deadline.
AWS: each invocation publishes AWS/Lambda Duration/Invocations datapoints under the FunctionName dimension, and GetMetricStatistics agrees with the Duration on the invocation's REPORT line. An alarm with a Lambda ARN in AlarmActions invokes that function on the OK->ALARM transition with the alarm-state-change payload, and is denied (no invocation) when the function's resource policy does not grant lambda:InvokeFunction to the alarm's principal.
Emulator: recorded behavior only — the metric series stays empty and the alarm action produces no invocation, so each check never satisfies its predicate and the harness reports "Check timed out after 60.0s".
Why: duration/latency dashboards are blank and alarm-driven remediation never runs, while the control plane reports the alarm as configured — so a client believes it has automated response that does not exist.
Fix: the AWS/Lambda metric publication in the invoke path, and the CloudWatch alarm-action dispatcher (including its invoke-permission check).
Traces: all three check ids in trace_ids.

## Evidence
Error: Check timed out after 60.0s for cloudwatch__lambda.metrics-lambda-duration-statistics-agree-with-the-report-line-of-each-invoke, .xsvc-cloudwatch-3-alarm-lambda-action-delivers-alarm-payload-to-function and .xsvc-cloudwatch-3-alarm-lambda-action-without-invoke-permission-denied. Expected: GetMetricStatistics returns Duration datapoints matching the REPORT line, and the alarm transition invokes the target (or is denied when the permission is absent).

## Steps to Reproduce
1. Invoke a function several times, then GetMetricStatistics for AWS/Lambda Duration with FunctionName=<fn>.
2. PutMetricAlarm with AlarmActions=[<function ARN>], AddPermission for the alarm principal, drive the metric past the threshold and look for the invocation; repeat without the AddPermission.
