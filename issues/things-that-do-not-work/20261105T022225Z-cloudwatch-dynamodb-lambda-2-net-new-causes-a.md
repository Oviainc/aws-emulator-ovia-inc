Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative cloudwatch__dynamodb__lambda.deploy-ddb-lambda-cloudwatch-alarm-stack
Issue-Key: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard|run#9|IndexError|mapping_uuid = lam.list_event_source_mappings(FunctionName=function_name)["EventSourceMappings"][0]["UUID"]
Issue-Key: cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger|run#8|IndexError|mapping_uuid = lam.list_event_source_mappings(FunctionName=function_name)["EventSourceMappings"][0]["UUID"]
Issue-Key: cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative|run#12|IndexError|mapping_uuid = lam.list_event_source_mappings(FunctionName=function_name)["EventSourceMappings"][0]["UUID"]
Issue-Key: cloudwatch__dynamodb__lambda.deploy-ddb-lambda-cloudwatch-alarm-stack|<lambda>#11|IndexError|lambda: cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]["StateValue"],
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# cloudwatch__dynamodb__lambda: 2 net-new causes — a CFN-deployed AWS::Lambda::EventSourceMapping is invisible to ListEventSourceMappings, and a CFN-deployed AWS::CloudWatch::Alarm is invisible to DescribeAlarms

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEventSourceMappings

## Description
Two distinct root causes: resources a CloudFormation stack reports as created are not present in the owning service's state. Neither cluster is marked OWNED or ALREADY FILED in the replay report.

Cause 1 of 2: a stack-declared AWS::Lambda::EventSourceMapping does not appear in lambda ListEventSourceMappings.
AWS: Smithy lambda ListEventSourceMappings (GET /2015-03-31/event-source-mappings) filtered by FunctionName returns EventSourceMappings, one entry per mapping attached to that function, each with a UUID, EventSourceArn, FunctionArn and State. A stack that declares AWS::Lambda::EventSourceMapping and reaches CREATE_COMPLETE has created the mapping, so the list is non-empty and GetEventSourceMapping on that UUID succeeds.
Emulator: after the stack settles, ListEventSourceMappings(FunctionName=<the stack's function>) returns an empty EventSourceMappings list — indexing the first element raises IndexError. The stack is reported created, but no mapping exists in Lambda's state, so the UUID that every later call needs cannot be obtained.
Why: a client that wires a DynamoDB stream (or any event source) to a function with a template cannot discover, inspect, disable or delete the mapping, and nothing is ever delivered through it. Every downstream assertion about the mapping (its state, its batch size, the metrics it emits) is unreachable.
Fix: the CloudFormation resource provider for AWS::Lambda::EventSourceMapping — it must create the mapping in the Lambda service state (and report its UUID as the physical resource id), not only mark the stack resource complete.
Traces: cloudwatch__dynamodb__lambda.deploy-ddb-stream-lambda-cloudwatch-dashboard, cloudwatch__dynamodb__lambda.deploy-ddb-table-stream-esm-cfn-lambda-trigger, cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative.

Cause 2 of 2: a stack-declared AWS::CloudWatch::Alarm does not appear in cloudwatch DescribeAlarms.
AWS: Smithy cloudwatch DescribeAlarms with AlarmNames=[name] returns MetricAlarms containing that alarm, with AlarmName, StateValue (INSUFFICIENT_DATA until data arrives, then OK/ALARM), MetricName, Namespace, Threshold and ComparisonOperator. A stack that declares AWS::CloudWatch::Alarm and reaches CREATE_COMPLETE has created the alarm.
Emulator: DescribeAlarms(AlarmNames=[<the alarm the template declared>]) returns an empty MetricAlarms list — indexing the first element raises IndexError. The alarm was never registered in CloudWatch's state despite the stack reporting it created.
Why: a client that provisions alarms declaratively cannot read their state; alarm-driven behaviour (notifications, deployment gating, auto-rollback) has nothing to key on, and DescribeAlarms gives no way to tell a missing alarm from one that simply has no data.
Fix: the CloudFormation resource provider for AWS::CloudWatch::Alarm — it must call through to PutMetricAlarm in the CloudWatch state with the template's AlarmName and metric configuration.
Traces: cloudwatch__dynamodb__lambda.deploy-ddb-lambda-cloudwatch-alarm-stack.

## Evidence
Cause 1 — expected: `lam.list_event_source_mappings(FunctionName=<fn>)["EventSourceMappings"][0]["UUID"]` yields the mapping the template declared. Actual (replay): `IndexError: list index out of range` at that expression, in all three checks.

Cause 2 — expected: `cw.describe_alarms(AlarmNames=[<alarm>])["MetricAlarms"][0]["StateValue"]` yields the alarm's state. Actual (replay): `IndexError: list index out of range` at that expression — MetricAlarms is empty.

(Function and alarm names are per-run values; the non-emptiness of the two listings for a stack-created resource is the requirement.)

## Steps to Reproduce
1. CreateStack with a template declaring a DynamoDB table with a stream, a Lambda function, an AWS::Lambda::EventSourceMapping from the stream to the function, and an AWS::CloudWatch::Alarm on a Lambda metric.
2. Wait for CREATE_COMPLETE.
3. Call lambda ListEventSourceMappings(FunctionName=<the stack's function>) — EventSourceMappings is empty (cause 1).
4. Call cloudwatch DescribeAlarms(AlarmNames=[<the template's AlarmName>]) — MetricAlarms is empty (cause 2).
