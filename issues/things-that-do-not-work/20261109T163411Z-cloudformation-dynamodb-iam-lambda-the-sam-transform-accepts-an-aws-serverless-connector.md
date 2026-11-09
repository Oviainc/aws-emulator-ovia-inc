Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb__iam__lambda.sam-connector-invalid-permission-rejected
Issue-Key: cloudformation__dynamodb__iam__lambda.sam-connector-invalid-permission-rejected|run#9|AssertionError|assert (bad_change_set["Status"], bad_change_set.get("StatusReason")) == (
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# The SAM transform accepts an AWS::Serverless::Connector with an unsupported Permissions value instead of failing the transform

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Cause 1 of 1: the AWS::Serverless-2016-10-31 transform does not validate an AWS::Serverless::Connector's Permissions against the source/destination pair, so an invalid connector is silently expanded and the change set succeeds.

AWS: a Connector from AWS::Lambda::Function to AWS::DynamoDB::Table supports only the Permissions values Read and Write. A connector asking for [Delete] fails the macro: the change set ends FAILED with StatusReason `Transform AWS::Serverless-2016-10-31 failed with: Invalid Serverless Application Specification document. Number of errors found: 1. Resource with id [<connector logical id>] is invalid. Unsupported 'Permissions' provided for connector from AWS::Lambda::Function to AWS::DynamoDB::Table; valid values are: Read, Write.` Fixed text is the whole sentence apart from the bracketed logical id, the two resource type names and the valid-value list, which come from the submitted template and the connector's profile table. Smithy: CreateChangeSet.TemplateBody is opaque and the transform is not modelled — recorded behavior only; the recording shows the change set in FAILED with exactly that StatusReason.

Emulator: the change set reports ('CREATE_COMPLETE', None) — Status CREATE_COMPLETE and no StatusReason, i.e. the invalid connector was accepted and expanded.

Why: a client relying on the transform to reject an unsupported connector permission gets a deployable change set, and then a connector whose generated policy either grants nothing or grants something the author did not express. The authoring error surfaces at runtime as a permission failure instead of at deploy time with a precise message.

Fix: the SAM transform's connector-profile validation inside the CloudFormation transform stage, which must fail the transform (change set FAILED + StatusReason) rather than expanding the resource.

Traces: cloudformation__dynamodb__iam__lambda.sam-connector-invalid-permission-rejected.

## Evidence
Expected: change set Status FAILED with StatusReason "Transform AWS::Serverless-2016-10-31 failed with: Invalid Serverless Application Specification document. Number of errors found: 1. Resource with id [Connector] is invalid. Unsupported 'Permissions' provided for connector from AWS::Lambda::Function to AWS::DynamoDB::Table; valid values are: Read, Write."
Actual: ('CREATE_COMPLETE', None)

## Steps to Reproduce
1. Write a SAM template (Transform: AWS::Serverless-2016-10-31) with a Lambda function, a DynamoDB table and an AWS::Serverless::Connector whose Source is the function, Destination the table, and Permissions: [Delete].
2. CreateChangeSet for that template and poll DescribeChangeSet.
3. Observe Status CREATE_COMPLETE with no StatusReason instead of a transform failure.
