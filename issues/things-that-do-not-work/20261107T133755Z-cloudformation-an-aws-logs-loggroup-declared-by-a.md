Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cdk-function-default-props-grant-real-log-writes
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cdk-function-default-props-grant-real-log-writes|_one_log_group#38|AssertionError|assert len(matches) == 1, (
Issue-Commit: d9e6cac456a6ddf38a11f2ddf4f109e13159427a

# cloudformation: an AWS::Logs::LogGroup declared by a deployed stack does not exist in CloudWatch Logs — DescribeLogGroups on the name the stack published matches 0 groups

> _Note: this issue was identified against the code base at commit `d9e6cac456a6ddf38a11f2ddf4f109e13159427a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a stack that declares an AWS::Logs::LogGroup reaches CREATE_COMPLETE and publishes the group's name in its Outputs, but the group was never materialised in CloudWatch Logs: DescribeLogGroups with logGroupNamePrefix set to exactly that published name returns an empty logGroups list, and the declared RetentionInDays is therefore nowhere to be read. The same stack's implicitly created /aws/lambda/<function> group DOES exist and carries the function's log events (the check asserts that first and it passes), so this is specific to the group CloudFormation itself owns, not to CloudWatch Logs being inert.

AWS: provisioning AWS::Logs::LogGroup creates the group in CloudWatch Logs before the stack reports CREATE_COMPLETE, so after the stack settles the group is readable by name and Ref on the resource resolves to that group name. Smithy (service-com-amazonaws-cloudwatchlogs-logs-20140328): CreateLogGroup{logGroupName @required, length 1..512, pattern ^[\.\-_/#A-Za-z0-9]+$}; PutRetentionPolicy{logGroupName @required, retentionInDays @required}; DescribeLogGroups{logGroupNamePrefix} -> logGroups[] where each entry carries logGroupName, arn, creationTime and retentionInDays only when a retention was set. A group created with RetentionDays.ONE_DAY reports retentionInDays == 1; a group created with no retention has the key absent (never expire). CloudFormation resource reference: https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-resource-logs-loggroup.html. Beyond that the model says nothing about the CloudFormation side — recorded behavior only: the recording shows the stack's Outputs carrying the group name and DescribeLogGroups then resolving exactly one group with that name and retentionInDays 1.
Emulator: after the deploy reports success and the stack Outputs contain RetainedLogGroupName, DescribeLogGroups(logGroupNamePrefix=<that exact name>) matches 0 groups (replay detail: "AssertionError: DescribeLogGroups for prefix '<stack-generated group name>' matched 0 groups with that exact name, expected exactly 1"; the name itself is a per-run CDK-generated value and is not part of the requirement). The declared retention (1 day) consequently cannot be observed at all, and the function configured to log into that explicit group has no group to write to. Hypothesis: the stack's LogGroup resource is reported as provisioned without the CloudWatch Logs state ever being written; the alternative reading — that the group was created under a name other than the one Ref/Outputs published — is the same defect seen from the other end, since a client can only find the group by the name the stack gives it.
Why: a client that declares its log group in the template (the documented way to control retention, tags, KMS and deletion with the stack) is told the stack succeeded and handed a group name that does not resolve. Every downstream read — DescribeLogGroups, PutRetentionPolicy, FilterLogEvents, a LoggingConfig.LogGroup pointing at it, a stack delete that should remove it — fails or silently reads nothing, and the stack's own view and CloudWatch Logs' view disagree.
Fix: the CloudFormation resource provisioner for AWS::Logs::LogGroup — it must create the group (and apply RetentionInDays) in the CloudWatch Logs service state under the same name it publishes through Ref/Outputs, before the stack is allowed to reach CREATE_COMPLETE. Related but NOT covering this: the open issue 20261107T034945Z-lambda-stepfunctions-a-log-group-declared-by-a.md records the same shape of defect under the lambda__stepfunctions heading, and 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md records CREATE_COMPLETE for logs resources that were never created; neither carries this check's replay key.
Traces: cloudformation.cdk-function-default-props-grant-real-log-writes (mismatch, failed at the DescribeLogGroups read of the stack-declared group, after the implicit /aws/lambda group and its log events had already been verified present).

## Evidence
Expected (AWS): the deployed stack declares a LogGroup with RetentionDays.ONE_DAY and outputs its name; DescribeLogGroups(logGroupNamePrefix=<that name>) returns exactly one logGroups entry whose logGroupName equals the name and whose retentionInDays == 1, and the function wired to it (LoggingConfig.LogGroup) writes its printed marker into that group.
Actual (emulator): the stack reports deployed and its Outputs include the group name, but DescribeLogGroups for that exact name returns zero groups — replay detail: "AssertionError: DescribeLogGroups for prefix '<stack-generated group name>' matched 0 groups with that exact name, expected exactly 1" (check helper _one_log_group, called from the step that asserts retentionInDays == 1).
Supporting, from the same run: the sibling default-props function's implicit group /aws/lambda/<function> DID exist, reported no retentionInDays (correct never-expire), and the function's printed marker was readable from it — so CloudWatch Logs reads and writes work in this run and the gap is the stack-declared group.

## Steps to Reproduce
1. Deploy a stack (CDK app or a plain template) containing: a Lambda function with default props, a second function whose logging is pointed at an explicitly declared AWS::Logs::LogGroup with RetentionInDays 1, and Outputs publishing the function names and {"Ref": <LogGroup logical id>} as the group name.
2. Wait for the stack to reach CREATE_COMPLETE and read its Outputs.
3. Call logs DescribeLogGroups with logGroupNamePrefix set to the published group name.
   Expected: exactly one logGroups entry with that logGroupName and retentionInDays == 1.
   Actual: logGroups == [] — no group with that name exists.
4. (Secondary) Invoke the function wired to the declared group and poll FilterLogEvents on that group name: the group is absent, so no events can ever be read from it.
