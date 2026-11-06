Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive|_invoke_and_parse#4|Timeout|return payload
Issue-Key: bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive|_invoke_and_parse#4|Timeout|return payload
Issue-Commit: 2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d

# bedrock-agent-runtime__lambda__s3vectors: CloudFormation create and update of the Lambda + Bedrock-agent + S3 Vectors stack never settle

> _Note: this issue was identified against the code base at commit `2d5907cd7e831b17c8c4fd60b3b325267ed3fe4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation stack that wires a Lambda function to a Bedrock agent runtime and an S3 Vectors index never reaches a terminal state, so both the create and the stack-update scenarios hang until the harness kills them at 60s.

AWS: CreateStack followed by DescribeStacks reaches a terminal status (CREATE_COMPLETE, or CREATE_FAILED/ROLLBACK_COMPLETE) for every accepted template, and UpdateStack that adds a bedrock:InvokeAgent permission to the function's role reaches UPDATE_COMPLETE; a client polling DescribeStacks therefore always terminates. Recorded behavior only for the per-resource provisioning of AWS::Lambda::Function / Bedrock agent / S3 Vectors resources in this template: the recording shows the stack settling and the deployed function then answering an invoke.
Emulator: neither check ever observes a terminal stack status or the deployed function's reply — both fail with "Check timed out after 60.0s" while polling. Nothing is refused: the emulator accepts the request and then never converges, which is worse than an error because the client has no failure to handle.
Why: a caller that deploys this stack and polls for completion (CLI `cloudformation deploy`, CDK, or a waiter) blocks forever instead of getting CREATE_COMPLETE or a failure reason, so every downstream step of a deployment pipeline stalls.
Fix: the stack provisioning loop for these resource types in the cloudformation service's resource providers, and whichever of the Lambda / Bedrock-agent-runtime / S3 Vectors resource handlers never signals completion back to it; the stack state machine must always reach a terminal status.
Traces: bedrock-agent-runtime__lambda__s3vectors.cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive, bedrock-agent-runtime__lambda__s3vectors.cfn-stack-update-add-bedrock-agent-permission-positive.

## Evidence
Both checks are replayed failures under the suite heading `### bedrock-agent-runtime__lambda__s3vectors`, neither marked ALREADY FILED nor OWNED:
- cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive — "Error: Check timed out after 60.0s" (group deployment-order, SERVICES = [bedrock-agent-runtime, lambda, s3vectors]).
- cfn-stack-update-add-bedrock-agent-permission-positive — "Error: Check timed out after 60.0s" (same group and services).
Expected: DescribeStacks reports a terminal status (CREATE_COMPLETE / UPDATE_COMPLETE) within the poll window and the deployed function answers. Actual: no terminal status is ever reported; the checks are killed at the 60s limit. The sibling checks in this group that do reach the emulator fail with other, already-filed causes, so the hang is specific to the stack lifecycle here.

## Steps to Reproduce
Boot the emulator, replay the behavioral suite and re-run the two checks under `bedrock-agent-runtime__lambda__s3vectors`: cfn-deploy-lambda-bedrock-s3vectors-full-stack-positive and cfn-stack-update-add-bedrock-agent-permission-positive.

By hand: CreateStack with a template containing the Lambda function (plus its execution role), the Bedrock agent runtime wiring and the S3 Vectors bucket/index used by that scenario, then poll DescribeStacks — the status never becomes terminal. Repeat with UpdateStack adding a bedrock:InvokeAgent statement to the role and poll again.
