Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle
Issue-Key: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle|phase#5|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: cc6137e930728edc173d6e704b5a7c1716e7fffd

# dynamodb__lambda__logs: the CloudFormation create of the two-stream Onyx handler stack ends ROLLBACK_COMPLETE, so the whole stream-processing lifecycle is unobservable

> _Note: this issue was identified against the code base at commit `cc6137e930728edc173d6e704b5a7c1716e7fffd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CreateStack of the Onyx stream-handler template — two AWS::DynamoDB::Table resources with NEW_AND_OLD_IMAGES streams, an AWS::IAM::Role, one AWS::Lambda::Function (inline Code.ZipFile) and two AWS::Lambda::EventSourceMapping resources (StartingPosition LATEST, BatchSize 1, MaximumRetryAttempts 3, one per table stream) — never reaches CREATE_COMPLETE. Failing check: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle.

AWS: this create provisions every resource and DescribeStacks reports StackStatus CREATE_COMPLETE (Smithy: com.amazonaws.cloudformation#StackStatus — CREATE_COMPLETE on success; ROLLBACK_COMPLETE only after a resource create fails, with DescribeStackEvents carrying the failing resource's ResourceStatusReason). The stack's Outputs then name the two tables, the function and the log group /aws/lambda/<function-name>; ListEventSourceMappings for the function returns two mappings whose EventSourceArns are the two tables' LatestStreamArns, each with StartingPosition LATEST, BatchSize 1 and MaximumRetryAttempts 3 (Smithy: lambda#CreateEventSourceMapping output members State/EventSourceArn/StartingPosition/BatchSize/MaximumRetryAttempts), and once armed each table write is delivered to the handler, whose per-record lines land in the function's log group.

Emulator: the create is accepted, then DescribeStacks reports StackStatus "ROLLBACK_COMPLETE"; the harness fails in its very first phase with "[phase 1/6 deploy] stack <name> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events", and no stack event names a failing resource. Phases 2-6 (setup, pre_invoke, solve, cleanup, destroy) never run, so the stream-record delivery, the INSERT-without-OldImage skip behavior and the diagnosis reads are all unexercised. Hypothesis: the same provisioning gap as the single-mapping case — AWS::Lambda::EventSourceMapping and/or the AWS::DynamoDB::Table StreamArn GetAtt attribute it depends on fails during create, and the stack is rolled back whole.

Why: a client that deploys a DynamoDB-streams consumer by template gets a rolled-back stack with no tables, no function and no mappings, and no reason it can read. Every stream-driven application built this way is undeployable, and the rollback is silent about which resource failed, so the client cannot even work around it.

Fix: the CloudFormation stack-provisioning path — the AWS::Lambda::EventSourceMapping resource provider (including multiple mappings in one stack and StartingPosition LATEST) and the AWS::DynamoDB::Table StreamArn GetAtt attribute — plus the create-failure/rollback path and the ResourceStatusReason it must record on the failing resource's stack event. Related open issues do not cover this: 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back.md and 20261103T171935Z-cloudformation-resource-failures-never-surface-stacks-and-resources.md are about failures reported as success; 20261104T044826Z-dynamodb-lambda-logs-dynamodb-streams-records-never-reach-the-mapped-lambda.md owns stream delivery for API-created mappings, not the template create that rolls back.

Traces: dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle (phase 1/6 deploy).

## Evidence
Expected (AWS): DescribeStacks -> StackStatus "CREATE_COMPLETE"; Outputs name the two tables (<tok>-BasaltRequests-alpha, <tok>-BasaltRegressionRequests-alpha), the function and log group /aws/lambda/<function>; ListEventSourceMappings(FunctionName=<function>) -> two mappings, {(StartingPosition, BatchSize, MaximumRetryAttempts)} == {("LATEST", 1, 3)}, EventSourceArns == the two tables' stream ARNs.

Actual (emulator): AssertionError: [phase 1/6 deploy] stack sim2real-onyx-e2e-<hex> reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE — read its stack events. The fixed text is "stack … reached 'ROLLBACK_COMPLETE', not CREATE_COMPLETE"; the stack name is the per-run variable part. No failing-resource reason is exposed via stack events.

## Steps to Reproduce
1. Boot the emulator. 2. CreateStack with the Onyx stream-handler template: two AWS::DynamoDB::Table resources with StreamSpecification NEW_AND_OLD_IMAGES, an AWS::IAM::Role for the function, one AWS::Lambda::Function with inline Code.ZipFile, and two AWS::Lambda::EventSourceMapping resources (one per table stream) with StartingPosition LATEST, BatchSize 1, MaximumRetryAttempts 3. 3. Poll DescribeStacks to a terminal status: it is ROLLBACK_COMPLETE instead of CREATE_COMPLETE. 4. Call DescribeStackEvents: no ResourceStatusReason names the resource that failed. 5. Re-replay dynamodb__lambda__logs.e2e-diagnose-onyx-lambda-processing-failure-lifecycle to confirm.
