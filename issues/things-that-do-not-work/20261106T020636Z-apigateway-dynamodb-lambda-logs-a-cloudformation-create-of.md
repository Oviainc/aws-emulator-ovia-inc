Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Key: apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created|_deploy_cfn#5|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# apigateway__dynamodb__lambda__logs: a CloudFormation create of a DynamoDB-stream + Lambda event-source-mapping stack ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CreateStack of a template whose resources are AWS::DynamoDB::Table (StreamSpecification NEW_AND_OLD_IMAGES, PAY_PER_REQUEST), AWS::IAM::Role (lambda.amazonaws.com trust + AWSLambdaBasicExecutionRole + a stream-read inline policy whose Resource is Fn::Sub "${Table.Arn}/stream/*"), AWS::Lambda::Function (python3.12, inline Code.ZipFile) and AWS::Lambda::EventSourceMapping (EventSourceArn = Fn::GetAtt [Table, StreamArn], StartingPosition TRIM_HORIZON, BatchSize 1) never reaches CREATE_COMPLETE. Failing checks: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly, apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created.

AWS: a create of this template provisions all four resources and DescribeStacks reports StackStatus CREATE_COMPLETE (Smithy: com.amazonaws.cloudformation#StackStatus enum contains CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_IN_PROGRESS, ROLLBACK_COMPLETE; a stack only reaches ROLLBACK_COMPLETE after a resource create actually fails, and DescribeStackEvents then carries a ResourceStatusReason naming the resource and the reason). The completed stack exposes its declared Outputs — Ref on the table, Fn::GetAtt Table.StreamArn (equal to the live DescribeTable LatestStreamArn), and Ref on the mapping (its UUID) — and Lambda GetEventSourceMapping on that UUID settles in State "Enabled" with EventSourceArn equal to that same stream ARN and FunctionArn ending ":function:<function-name>" (Smithy: lambda#CreateEventSourceMapping, 202, output members UUID/State/EventSourceArn/FunctionArn). A write to the table is then delivered to the function, whose stdout line reaches /aws/lambda/<function-name>.

Emulator: for the same create, DescribeStacks reports StackStatus "ROLLBACK_COMPLETE" and the stack carries no Outputs, so the harness fails at the deploy step with "Stack <name> ended in ROLLBACK_COMPLETE". Nothing downstream can be observed: the exported StreamArn, the mapping's State/EventSourceArn/FunctionArn, and the stream-triggered log line are all unreachable. The replay shows no refusal on CreateStack itself (it is accepted) and no reason surfaced to the caller for the rollback. Hypothesis: one of the declared resources (most likely AWS::Lambda::EventSourceMapping, or the AWS::DynamoDB::Table StreamArn GetAtt attribute that its EventSourceArn depends on) fails or resolves to nothing during provisioning, and the stack is rolled back whole.

Why: every client that deploys a DynamoDB-stream-driven Lambda by template — CloudFormation, SAM or CDK — gets a stack that rolls back, with no Outputs and no usable function or mapping. Code that reads the stack's StreamArn output or the mapping UUID fails at deploy time, so the entire stream-processing estate cannot be stood up from a template even though every one of its resources is individually modeled.

Fix: the CloudFormation stack-provisioning path — the resource providers for AWS::Lambda::EventSourceMapping and the AWS::DynamoDB::Table StreamArn GetAtt attribute it consumes (and the IAM role's Fn::Sub over ${Table.Arn}) — plus the create-failure path that currently rolls the whole stack back; the per-resource failure must also be reported through DescribeStackEvents' ResourceStatusReason so the cause is diagnosable. Related but distinct open issues: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back.md and 20261103T171935Z-cloudformation-resource-failures-never-surface-stacks-and-resources.md describe the OPPOSITE direction (failures reported as success) and do not cover this template; 20261028T172904Z-ecs-events-iam-s3-a-cloudformation-create-of.md is a rollback of a different (ECS/EventBridge) estate.

Traces: apigateway__dynamodb__lambda__logs.deploy-dynamodb-table-stream-arn-exported-correctly (AssertionError: Stack sim2real-xsvc-stk-… ended in ROLLBACK_COMPLETE, raised from the deploy helper before any DynamoDB/Lambda assertion), apigateway__dynamodb__lambda__logs.deploy-lambda-dynamodb-event-source-mapping-created (same assertion at the same deploy step).

## Evidence
Expected (AWS): DescribeStacks -> Stacks[0].StackStatus == "CREATE_COMPLETE"; Outputs contain TableName, StreamArn (== DescribeTable Table.LatestStreamArn) and EventSourceMappingId; GetEventSourceMapping(UUID=<that id>) -> State "Enabled", EventSourceArn == that StreamArn, FunctionArn ending ":function:<function-name>"; a PutItem on the table produces a log event in /aws/lambda/<function-name> containing the handler's "STREAM_RECORD_ID <pk>" line.

Actual (emulator): CreateStack is accepted, then DescribeStacks -> Stacks[0].StackStatus == "ROLLBACK_COMPLETE" with no Outputs. Harness error, both checks: AssertionError: Stack <stack-name> ended in ROLLBACK_COMPLETE (raised in _deploy_cfn after polling to a terminal status). No ResourceStatusReason is surfaced to the caller naming which resource failed. Fixed text of the harness message is "Stack … ended in …"; the stack name (sim2real-xsvc-stk-<hex>) and the status are the variable parts.

## Steps to Reproduce
1. Build and boot the emulator, then against it: cloudformation CreateStack with Capabilities [CAPABILITY_IAM, CAPABILITY_NAMED_IAM] and a template containing (a) AWS::DynamoDB::Table with KeySchema pk HASH, BillingMode PAY_PER_REQUEST and StreamSpecification {StreamViewType: NEW_AND_OLD_IMAGES}; (b) AWS::IAM::Role trusting lambda.amazonaws.com with ManagedPolicyArns [arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole] and an inline policy whose Resource is {"Fn::Sub": "${Table.Arn}/stream/*"}; (c) AWS::Lambda::Function runtime python3.12, Code.ZipFile inline, Role Fn::GetAtt ExecRole.Arn; (d) AWS::Lambda::EventSourceMapping with EventSourceArn Fn::GetAtt [Table, StreamArn], FunctionName Ref Function, StartingPosition TRIM_HORIZON, BatchSize 1; Outputs TableName, StreamArn, EventSourceMappingId.
2. Poll DescribeStacks until StackStatus is terminal.
3. Observe ROLLBACK_COMPLETE with no Outputs instead of CREATE_COMPLETE; DescribeStackEvents names no failing resource reason.
4. Replay the two checks listed in trace_ids to confirm.
