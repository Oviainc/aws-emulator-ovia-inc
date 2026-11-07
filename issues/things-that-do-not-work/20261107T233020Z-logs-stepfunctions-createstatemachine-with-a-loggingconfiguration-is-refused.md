Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events
Issue-Key: sfn|CreateStateMachine|AccessDeniedException
Issue-Commit: 69a812023dc7b19082ccb88a67a077b5f3204d48

# logs__stepfunctions: CreateStateMachine with a loggingConfiguration is refused AccessDeniedException "The state machine IAM Role is not authorized to access the Log Destination" even when the role carries the documented vended-logs actions

> _Note: this issue was identified against the code base at commit `69a812023dc7b19082ccb88a67a077b5f3204d48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
One net-new root cause, 1 failing check.

Cause 1 of 1: CreateStateMachine is refused for a role that IS authorized for the log destination, so no logging-configured state machine can be created.
  AWS: Smithy (services/service-com-amazonaws-sfn-awsstepfunctions): CreateStateMachine takes loggingConfiguration and models the errors ConflictException, InvalidArn, InvalidDefinition, InvalidEncryptionConfiguration, InvalidLoggingConfiguration, InvalidName, InvalidTracingConfiguration, Kms*, StateMachineAlreadyExists, StateMachineDeleting, StateMachineLimitExceeded, StateMachineTypeNotSupported, TooManyTags, ValidationException — AccessDeniedException is not among them. Recorded behavior: with a role trusted by states.amazonaws.com whose inline policy allows logs:CreateLogDelivery, logs:GetLogDelivery, logs:UpdateLogDelivery, logs:DeleteLogDelivery, logs:ListLogDeliveries, logs:PutResourcePolicy, logs:DescribeResourcePolicies and logs:DescribeLogGroups on Resource "*" (the actions Step Functions documents as required for a loggingConfiguration), CreateStateMachine with type=EXPRESS and destinations=[{cloudWatchLogsLogGroup:{logGroupArn:<group arn>:*}}] SUCCEEDS and returns stateMachineArn. AWS answers AccessDeniedException with the message "The state machine IAM Role is not authorized to access the Log Destination" only when the role does NOT carry those actions — that whole sentence is fixed text with no request-derived value.
  Emulator: the call is refused with ClientError (AccessDeniedException) "The state machine IAM Role is not authorized to access the Log Destination" for a role that does carry them. The check never gets past CreateStateMachine, so nothing of its subject (EXPRESS GetExecutionHistory refusal, vended log delivery) is exercised.
  Why: a client cannot create ANY state machine with CloudWatch Logs logging configured — the standard observability setup for EXPRESS workflows — and is told its correctly-permissioned role is unauthorized. Because the check creates the role itself, no account configuration can explain the refusal. The same refusal plausibly rolls back the CloudFormation estate (filed as cloudformation__stepfunctions: the stack reaches ROLLBACK_COMPLETE).
  Fix: the log-destination authorization evaluation on CreateStateMachine/UpdateStateMachine in the sfn service crate (services/service-com-amazonaws-sfn-awsstepfunctions, the CreateStateMachine handler's loggingConfiguration pre-check) — it must evaluate the role's attached/inline policies for the vended-log-delivery actions and allow the call when they are present, and it must not raise an error class CreateStateMachine does not model when the role is in fact authorized.
  Traces: logs__stepfunctions.deep-sfn-3-express-async-history-rejected-logs-carry-events.

## Evidence
botocore.exceptions.ClientError: An error occurred (AccessDeniedException) when calling the CreateStateMachine operation: The state machine IAM Role is not authorized to access the Log Destination

Expected: HTTP 200 with stateMachineArn. The role is created by the check (name is a per-run value) with exactly the inline policy quoted above; the message's text is entirely fixed text, so the divergence is the refusal itself, not its wording.

## Steps to Reproduce
1. logs:CreateLogGroup a group named /aws/vendedlogs/states/<name> and read its arn (append ":*").
2. iam:CreateRole trusted by states.amazonaws.com, then iam:PutRolePolicy with an inline policy allowing logs:CreateLogDelivery, logs:GetLogDelivery, logs:UpdateLogDelivery, logs:DeleteLogDelivery, logs:ListLogDeliveries, logs:PutResourcePolicy, logs:DescribeResourcePolicies, logs:DescribeLogGroups on Resource "*".
3. states:CreateStateMachine with that roleArn, type=EXPRESS, a one-state Pass definition and loggingConfiguration {level: ALL, includeExecutionData: true, destinations: [{cloudWatchLogsLogGroup: {logGroupArn: <group arn>:*}}]}. The emulator answers AccessDeniedException; AWS answers 200.
