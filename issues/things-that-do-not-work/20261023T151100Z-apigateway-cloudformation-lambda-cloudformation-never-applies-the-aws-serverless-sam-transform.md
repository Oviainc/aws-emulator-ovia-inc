Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https
Issue-Key: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https|run#12|AssertionError|assert len(deployments) == 1 and _DEPLOYMENT_ID_RE.match(deployments[0]), (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# CloudFormation never applies the AWS Serverless (SAM) transform: a template with Transform: AWS::Serverless-2016-10-31 produces a change set that plans no resources

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
One root cause.

Cause 1 of 1: CloudFormation accepts a template declaring `Transform: AWS::Serverless-2016-10-31` but does not run the transform, so the change set contains none of the resources the transform is defined to synthesize.
AWS: recorded behavior only (the SAM transform is a CloudFormation macro, not part of the CloudFormation Smithy model). A template whose only resources are AWS::Serverless::Function with an implicit Api event is transformed at CreateChangeSet time into concrete resources — an AWS::ApiGateway::RestApi, exactly one content-hash-suffixed AWS::ApiGateway::Deployment, an AWS::ApiGateway::Stage, the AWS::Lambda::Function, its AWS::Lambda::Permission and AWS::IAM::Role — and DescribeChangeSet/ListChangeSets report those transformed resources before execution.
Emulator: the change set is created but plans an EMPTY resource list; the replayed check found zero content-hash-suffixed implicit-API deployments planned (`the change set planned []`).
Why: every SAM-authored stack is a no-op against this emulator. A client deploys a serverless application, gets a successful change set and stack, and then finds no API, no stage, no permission and no function — and because the change set reports no changes, there is nothing in the plan for the client to detect the failure from.
Fix: the CloudFormation change-set/transform pipeline (template transform evaluation for the AWS::Serverless-2016-10-31 macro) in services/service-com-amazonaws-cloudformation-cloudformation; the resources it must synthesize belong to apigateway, lambda and iam.
Traces: apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https. The same root cause also surfaces through a different service token as apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow, filed separately because an issue carries one service token.

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1):
- apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https -> AssertionError: SAM synthesizes exactly one content-hash-suffixed implicit-API deployment; the change set planned []
Expected (AWS): the change set plans the transformed resources (RestApi, one hash-suffixed Deployment, Stage, Function, Permission, Role).
Actual (emulator): the change set plans no resources at all; CreateChangeSet/DescribeChangeSet report success with an empty change list.

## Steps to Reproduce
1. Boot the emulator; point botocore at it (us-west-2).
2. Call cloudformation.CreateChangeSet with Capabilities=['CAPABILITY_IAM','CAPABILITY_AUTO_EXPAND'] and a template that declares `Transform: AWS::Serverless-2016-10-31` and a single AWS::Serverless::Function with an implicit `Api` event.
3. Wait for the change set and call DescribeChangeSet -> observe an empty Changes list and no AWS::ApiGateway::Deployment whose logical id carries the content hash.
4. Re-replay apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https to confirm.
