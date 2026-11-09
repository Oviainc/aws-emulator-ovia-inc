Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__dynamodb.native-dynamodb-table-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__dynamodb.native-dynamodb-table-delete-leaves-cloudcontrol-not-found|_poll_until#8|AssertionError|raise AssertionError(
Issue-Commit: 545841f07feb57b92e392d55db412e521f655a7b

# Cloud Control GetResource never resolves a natively created DynamoDB table

> _Note: this issue was identified against the code base at commit `545841f07feb57b92e392d55db412e521f655a7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: Cloud Control API has no view of DynamoDB tables it did not itself create.

AWS: Cloud Control API is a uniform control plane over the resource providers, not a side registry. `GetResource` with TypeName `AWS::DynamoDB::Table` and an Identifier naming any existing table in the account/region returns `ResourceDescription` with that table's `Identifier` and `Properties`, regardless of whether the table was created through CreateResource or through the native `dynamodb:CreateTable` API; and once the table is deleted it answers `ResourceNotFoundException`. Smithy (cloudcontrol): GetResource -> GetResourceOutput{TypeName, ResourceDescription}, errors include ResourceNotFoundException and TypeNotFoundException. The recording drives exactly that sequence: create a table natively, read it back through Cloud Control, delete it natively, read again.

Emulator: `GetResource` never resolves the natively created table at all. The check polled for 30s and the resource never appeared; the last observation was `None`. Because the resource is never visible, the "delete leaves it NotFound" half of the behavior can never be reached either.

Why: a client that uses Cloud Control as its single generic control plane (CDK import, cfn-cli, resource-explorer style tooling, drift detection) sees an account that appears empty of DynamoDB tables unless every table was minted through Cloud Control. Generic provisioners therefore try to re-create resources that already exist, or report them as deleted.

Fix: the Cloud Control resource handler for `AWS::DynamoDB::Table` must read through to the DynamoDB service state rather than a Cloud-Control-local resource registry — belongs in the GetResource handler's type dispatch and in the AWS::DynamoDB::Table provider mapping, not in the DynamoDB service.

Traces: cloudcontrol__dynamodb.native-dynamodb-table-delete-leaves-cloudcontrol-not-found. The sibling check `cloudcontrol-success-means-the-native-table-is-already-active-and-writable` is already filed as 20261102T041105Z-cloudcontrol-dynamodb-cloudcontrol-createresource-deleteresource-reach-a-terminal-success-without.md; that issue covers the CreateResource/DeleteResource direction and does not cover the read-through of natively created resources described here.

## Evidence
Expected: GetResource(TypeName='AWS::DynamoDB::Table', Identifier=<table created with dynamodb:CreateTable>) -> 200 with ResourceDescription.Identifier == that table name; after DeleteTable the same call -> ResourceNotFoundException.
Actual: AssertionError: waited 30.0s for Cloud Control GetResource to resolve '<table-name>', a table it never created and it never happened; last observation None

## Steps to Reproduce
1. dynamodb:CreateTable a table natively and wait for ACTIVE.
2. cloudcontrol:GetResource --type-name AWS::DynamoDB::Table --identifier <table-name>.
3. Observe the resource never resolves (polling for 30s yields nothing) instead of returning the table's ResourceDescription.
4. dynamodb:DeleteTable, then repeat step 2 and observe ResourceNotFoundException can never be distinguished from step 3.
