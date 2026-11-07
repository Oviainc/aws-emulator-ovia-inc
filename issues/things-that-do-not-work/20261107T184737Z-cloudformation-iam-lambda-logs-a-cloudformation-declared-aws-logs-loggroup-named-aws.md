Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs
Issue-Key: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs|_poll_until#10|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# A CloudFormation-declared AWS::Logs::LogGroup named /aws/lambda/<function> never exists: DescribeLogGroups with that prefix stays empty after CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (AWS::Logs::LogGroup) / DescribeLogGroups

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: a log group declared in the stack template as AWS::Logs::LogGroup with LogGroupName /aws/lambda/<function> is never materialised — after the stack is provisioned, DescribeLogGroups filtered on that exact name returns no group.
Failing check ids for this cause: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs.

AWS: recorded behavior only for the end-to-end wiring; the Smithy surface is CloudWatch Logs DescribeLogGroups(logGroupNamePrefix) and FilterLogEvents. The recording shows that once the stack reaches CREATE_COMPLETE, DescribeLogGroups with the prefix /aws/lambda/<function> returns exactly that group with the declared retentionInDays (1 day here), that it is the group the deployed function logs into even though the function's execution role grants only logs:PutLogEvents on that group's ARN and no logs:CreateLogGroup, and that the invocation's marker line is then readable from the group's events.
Emulator: the group never appears. The check polls for 120 s and reports "log group /aws/lambda/<function-name> present not met within 120.0s. Last result: []" — DescribeLogGroups returns an empty list, so the declared retention, the "the declared group is the one the function uses" relationship and the log-content assertion are all unreachable. The function name is generated per run; the requirement is that a group with the template's LogGroupName exists.
Why: declaring a function's log group in the template — the standard way to control retention and to avoid granting logs:CreateLogGroup — has no effect. A client cannot read the group it declared, retention policy is silently not applied, and a least-privilege execution role that deliberately omits logs:CreateLogGroup is left with nowhere to write.
Fix: the CloudFormation resource provisioner for AWS::Logs::LogGroup — create the group under the declared LogGroupName with its RetentionInDays (and tags) as part of stack provisioning, so it is visible to DescribeLogGroups and is the group the matching Lambda writes into.
Traces: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs.

The open issues 20261107T133755Z-cloudformation-iam-lambda-logs-createstack-declaring-an-aws.md and 20261107T085614Z-cloudwatch-lambda-logs-the-cloudformation-deployed-lambda-aws-logs-loggroup-path.md cover adjacent behavior in this area; neither carries this check's replay key.

## Evidence
Expected (recorded AWS): after CREATE_COMPLETE, DescribeLogGroups(logGroupNamePrefix="/aws/lambda/<fn>") returns that group with retentionInDays 1, and the invocation's marker line is readable from it.

Actual (emulator): AssertionError: log group /aws/lambda/<fn> present not met within 120.0s. Last result: []

## Steps to Reproduce
1. CreateStack with an AWS::Logs::LogGroup whose LogGroupName is /aws/lambda/<function-name> and RetentionInDays 1, an execution role granting only logs:PutLogEvents on that group's ARN (no logs:CreateLogGroup), and the AWS::Lambda::Function of that name.
2. Wait for CREATE_COMPLETE.
3. Call DescribeLogGroups with logGroupNamePrefix=/aws/lambda/<function-name>.
4. Observe an empty list.
Or replay the check id in trace_ids.
