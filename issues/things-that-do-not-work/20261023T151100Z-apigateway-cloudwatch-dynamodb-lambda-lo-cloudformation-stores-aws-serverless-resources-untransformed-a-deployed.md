Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow|_assert_transform_expanded#17|AssertionError|assert not missing, (
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# CloudFormation stores AWS::Serverless::* resources untransformed: a deployed SAM stack's resources are the Serverless types themselves, not the ApiGateway/Lambda/IAM/DynamoDB resources the transform must synthesize

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

Cause 1 of 1: a stack created from a template with `Transform: AWS::Serverless-2016-10-31` reaches a terminal success state while its stack resources are the AWS::Serverless::* macro types, i.e. the transform was never applied.
AWS: recorded behavior only (the SAM transform is a CloudFormation macro, not part of the CloudFormation Smithy model). After CreateStack of a SAM template, DescribeStackResources/ListStackResources report the SYNTHESIZED types — AWS::ApiGateway::RestApi, AWS::ApiGateway::Deployment, AWS::ApiGateway::Stage, AWS::Lambda::Function, AWS::Lambda::Permission, AWS::IAM::Role, AWS::DynamoDB::Table — and no AWS::Serverless::* type appears in the deployed stack.
Emulator: the deployed stack's resource types are ['AWS::CloudWatch::Alarm', 'AWS::Logs::LogGroup', 'AWS::Logs::MetricFilter', 'AWS::Serverless::Api', 'AWS::Serverless::Function', 'AWS::Serverless::SimpleTable'] — the Serverless types are stored verbatim and none of the concrete API Gateway / Lambda / IAM / DynamoDB resources exist.
Why: the stack reports success, so a client believes its serverless application is deployed, but no function, API, stage or table was created; every subsequent call against the expected physical resources fails with a not-found, and nothing in the stack's status or resource list signals that the transform was skipped.
Fix: the CloudFormation template-transform stage (AWS::Serverless-2016-10-31 macro expansion before resource provisioning) in services/service-com-amazonaws-cloudformation-cloudformation.
Traces: apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow. The same root cause surfaces under another service token as apigateway__cloudformation__lambda.sam-implicit-api-synthesis-serves-real-https (filed separately, since an issue carries one service token).

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1):
- apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow -> AssertionError: the SAM transform did not synthesize ['AWS::ApiGateway::RestApi', 'AWS::ApiGateway::Deployment', 'AWS::ApiGateway::Stage', 'AWS::Lambda::Function', 'AWS::Lambda::Permission', 'AWS::IAM::Role', 'AWS::DynamoDB::Table']; the deployed stack holds ['AWS::CloudWatch::Alarm', 'AWS::Logs::LogGroup', 'AWS::Logs::MetricFilter', 'AWS::Serverless::Api', 'AWS::Serverless::Function', 'AWS::Serverless::SimpleTable']
Expected (AWS): only transformed, concrete resource types in the deployed stack.
Actual (emulator): AWS::Serverless::Api / ::Function / ::SimpleTable stored as-is; no concrete API Gateway, Lambda, IAM or DynamoDB resources.

## Steps to Reproduce
1. Boot the emulator; point botocore at it (us-west-2).
2. cloudformation.CreateStack with Capabilities=['CAPABILITY_IAM','CAPABILITY_AUTO_EXPAND'] and a SAM template (`Transform: AWS::Serverless-2016-10-31`) declaring AWS::Serverless::Api, AWS::Serverless::Function and AWS::Serverless::SimpleTable.
3. Wait for the terminal status, then ListStackResources -> observe the AWS::Serverless::* types still present and none of the synthesized types.
4. Re-replay apigateway__cloudwatch__dynamodb__lambda__logs.sam-serverless-api-transform-workflow to confirm.
