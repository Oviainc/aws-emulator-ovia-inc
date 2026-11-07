Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted
Issue-Key: cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync|_deploy#1|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Key: cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted|_deploy#1|AssertionError|assert status == "CREATE_COMPLETE", (
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# cloudformation__stepfunctions: the Step Functions + role + vended-log-group stack no longer creates — it reaches ROLLBACK_COMPLETE, so every check of that estate is blocked

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, 2 failing checks. Both checks deploy the SAME template (cfn-state-machine-drift/template.json, inlined in the checks): an AWS::Logs::LogGroup (/aws/vendedlogs/states/<name>, RetentionInDays 1), an AWS::IAM::Role trusted by states.amazonaws.com carrying one inline policy with logs:CreateLogDelivery, logs:GetLogDelivery, logs:UpdateLogDelivery, logs:DeleteLogDelivery, logs:ListLogDeliveries, logs:PutResourcePolicy, logs:DescribeResourcePolicies and logs:DescribeLogGroups on Resource "*", and an AWS::StepFunctions::StateMachine (STANDARD) whose RoleArn is Fn::GetAtt of that role and whose LoggingConfiguration Level=ALL / IncludeExecutionData=true points at Fn::GetAtt <LogGroup>.Arn.

Cause 1 of 1: CreateStack on that template never reaches CREATE_COMPLETE — the create fails and the stack rolls back.
  AWS: recorded behavior only for the stack outcome (CloudFormation's Smithy model declares CreateStack/DescribeStacks and the StackStatus enum, not per-resource provisioning). The recording shows the stack reaching CREATE_COMPLETE with all three logical resources CREATE_COMPLETE, after which the checks read its Outputs (StateMachineArn, StateMachineName, StateMachineRoleName, LogGroupName). A role carrying exactly the vended-logs actions above is what Step Functions documents as sufficient for a loggingConfiguration, so CreateStateMachine succeeds.
  Emulator: the stack reaches ROLLBACK_COMPLETE, i.e. one resource's create was refused and CloudFormation rolled the stack back. Hypothesis: the failing resource is TaskOrchestrator (AWS::StepFunctions::StateMachine) and the refusal is the same one the logs__stepfunctions check sees directly against the API — CreateStateMachine with a loggingConfiguration answers AccessDeniedException "The state machine IAM Role is not authorized to access the Log Destination" even when the role carries those actions (filed separately under the logs__stepfunctions heading, check logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events). The stack events name the resource; the replay output only carries the stack status.
  Why: a client cannot deploy the standard "state machine + execution role + vended log group" stack at all — the template is the canonical IaC shape for Step Functions logging. Every downstream contract of that estate (Outputs, DescribeStackResources physical ids, DetectStackDrift) is unreachable, so this blocks both checks before their own subject.
  Note: this failure point is net-new. Open issue 20261107T034945Z-cloudformation-stepfunctions-a-stack-declared-aws-logs-loggroup.md describes the SAME two checks reaching CREATE_COMPLETE and then diverging later (log group absent, drift reported IN_SYNC); it does not cover a stack that no longer creates, and its replay keys (run#17, run#7) are not the keys here (_deploy, stack status assertion). Open issue 20261102T101202Z-cloudformation-stepfunctions-3-net-new-root-causes-role.md covers other keys in this service and not this one.
  Fix: the create path of the stack's resource providers in the cloudformation service crate — specifically the AWS::StepFunctions::StateMachine provider's CreateStateMachine call (and the AWS::Logs::LogGroup/AWS::IAM::Role providers it depends on) — plus whatever refuses that call (see the logs__stepfunctions issue). The stack must reach CREATE_COMPLETE for this template.
  Traces: cloudformation__stepfunctions.deploy-cfn-detect-drift-reports-out-of-band-logging-level-as-modified-then-in-sync, cloudformation__stepfunctions.deploy-cfn-stack-reports-complete-while-state-machine-is-deleted.

## Evidence
Both checks fail in their _deploy helper, on the same assertion:

AssertionError: stack sim2real-scn-sfnstack-c80185427f19 reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events for the resource that failed
AssertionError: stack sim2real-scn-sfnstack-dceb0527cd65 reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events for the resource that failed

Expected terminal stack status: CREATE_COMPLETE. The stack and resource names are per-run values; the requirement is that this template's create succeeds. Corroborating, in the same replay logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events gets botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the CreateStateMachine operation: The state machine IAM Role is not authorized to access the Log Destination, for a role carrying the same vended-logs actions.

## Steps to Reproduce
1. CreateStack with Capabilities=[CAPABILITY_NAMED_IAM] from a template declaring: AWS::Logs::LogGroup (LogGroupName /aws/vendedlogs/states/<name>, RetentionInDays 1); AWS::IAM::Role trusted by states.amazonaws.com with an inline policy allowing logs:CreateLogDelivery, logs:GetLogDelivery, logs:UpdateLogDelivery, logs:DeleteLogDelivery, logs:ListLogDeliveries, logs:PutResourcePolicy, logs:DescribeResourcePolicies, logs:DescribeLogGroups on "*"; AWS::StepFunctions::StateMachine (STANDARD) with RoleArn = Fn::GetAtt role.Arn and LoggingConfiguration {Level: ALL, IncludeExecutionData: true, Destinations: [{CloudWatchLogsLogGroup: {LogGroupArn: Fn::GetAtt LogGroup.Arn}}]}.
2. Poll DescribeStacks for a terminal status — the emulator reaches ROLLBACK_COMPLETE, AWS reaches CREATE_COMPLETE.
3. DescribeStackEvents on the stack to see which resource was refused (expected: the AWS::StepFunctions::StateMachine, refused AccessDeniedException on the log destination).
