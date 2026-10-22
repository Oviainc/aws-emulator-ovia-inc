Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.alarm-ec2-action-requires-instance-metric
Issue-Key: cloudwatch__ec2.alarm-ec2-action-requires-instance-metric|run#7|AssertionError|raise AssertionError(
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# CloudWatch PutMetricAlarm accepts an EC2 stop action on a metric with no InstanceId dimension

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: the EC2-action precondition on PutMetricAlarm is not validated.
AWS: an alarm whose AlarmActions contains an EC2 action ARN (arn:aws:automate:<region>:ec2:stop|terminate|reboot|recover) is only accepted when the alarm's metric is dimensioned by InstanceId; otherwise PutMetricAlarm refuses the request with a 400 ValidationError (Smithy com-amazonaws-cloudwatch-graniteserviceversion20100801: PutMetricAlarm takes AlarmActions/Dimensions and declares no success body, so the refusal is the only observable; the EC2-action requirement is documented by CloudWatch, recorded behavior for the exact code).
Emulator: the alarm is created and PutMetricAlarm returns success — the check reports the alarm 'sim2real-xsvc-alarm-nodim-...' was created with action arn:aws:automate:us-west-2:ec2:stop on a metric carrying no InstanceId dimension.
Why: a client that relies on this guardrail ships an alarm that can never execute its action, and a negative test of the guardrail passes against a broken configuration.
Fix: input validation in the PutMetricAlarm handler (cross-check AlarmActions EC2 action ARNs against the request's Dimensions).
Traces: cloudwatch__ec2.alarm-ec2-action-requires-instance-metric.

## Evidence
Expected: PutMetricAlarm rejects the request. Actual: AssertionError: PutMetricAlarm must reject the EC2 stop action arn:aws:automate:us-west-2:ec2:stop on a metric with no InstanceId dimension, but the alarm sim2real-xsvc-alarm-nodim-e7c95f9380ff was created.

## Steps to Reproduce
Replay cloudwatch__ec2.alarm-ec2-action-requires-instance-metric, or call cloudwatch:PutMetricAlarm with Namespace=AWS/EC2, MetricName=CPUUtilization, Dimensions=[] and AlarmActions=['arn:aws:automate:us-west-2:ec2:stop'] and observe that it succeeds.
