Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__datasync__fsx.deploy-datasync-fsx-lustre-and-openzfs-locations-round-trip-and-delete cloudformation__datasync__fsx.deploy-datasync-fsx-openzfs-location-round-trip-and-delete
Issue-Key: cloudformation__datasync__fsx|CreateStack|ValidationError
Issue-Commit: 00353ccf65811f65ad7f73dbcefa75ea121211d3

# cloudformation__datasync__fsx: CreateStack refuses AWS::DataSync::LocationFSxLustre and AWS::DataSync::LocationFSxOpenZFS as unrecognized resource types, so no FSx DataSync location can be deployed

> _Note: this issue was identified against the code base at commit `00353ccf65811f65ad7f73dbcefa75ea121211d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the CloudFormation resource-type registry does not know the FSx-backed DataSync location types, so every template declaring one is refused outright.
AWS: CloudFormation registers AWS::DataSync::LocationFSxLustre and AWS::DataSync::LocationFSxOpenZFS (TemplateReference aws-resource-datasync-locationfsxlustre / aws-resource-datasync-locationfsxopenzfs). A CreateStack declaring either is accepted — Smithy: CreateStack -> CreateStackOutput.StackId — and the stack proceeds to CREATE_COMPLETE with the resource's Ref being the DataSync location ARN, readable back through DataSync DescribeLocationFsxLustre / DescribeLocationFsxOpenZfs. The "Template format error: Unrecognized resource types: [...]" ValidationError is reserved for types CloudFormation does not support: its fixed text is that sentence and its variable part is the list of offending types taken from the submitted template, so it must never name a supported type.
Emulator: CreateStack answers error code ValidationError (HTTP 400) with message "Template format error: Unrecognized resource types: [AWS::DataSync::LocationFSxLustre]" for the Lustre template and "Template format error: Unrecognized resource types: [AWS::DataSync::LocationFSxOpenZFS]" for the OpenZFS template. The wording is right; the types listed are ones AWS recognizes.
Why: a client cannot deploy an FSx-backed DataSync location by template at all — the whole stack is rejected, taking every other resource in it with it — and neither the happy-path round trip nor the stack deletion path can be exercised, so infrastructure-as-code DataSync/FSx deployments fail on their first call.
Fix: the CloudFormation resource-type registry consulted by CreateStack's template validation, plus resource providers for AWS::DataSync::LocationFSxLustre and AWS::DataSync::LocationFSxOpenZFS that provision through DataSync's own CreateLocationFsxLustre / CreateLocationFsxOpenZfs and record the returned ARN as the physical resource id (and delete it on stack delete). Open issues name the same registry gap for other services (e.g. 20261109T113428Z-cloudformation-mwaa-createstack-refuses-aws-mwaa-environment-as.md), but none carries these checks.
Traces: cloudformation__datasync__fsx.deploy-datasync-fsx-lustre-and-openzfs-locations-round-trip-and-delete, cloudformation__datasync__fsx.deploy-datasync-fsx-openzfs-location-round-trip-and-delete.

## Evidence
Expected: CreateStack returns a StackId and the stack reaches CREATE_COMPLETE; the FSx location resources' Refs are DataSync location ARNs that DescribeLocationFsxLustre / DescribeLocationFsxOpenZfs read back.
Actual (both checks fail at the very first CreateStack):
- botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DataSync::LocationFSxLustre]
- botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DataSync::LocationFSxOpenZFS]

## Steps to Reproduce
1. Build a template whose Resources declare {"Type": "AWS::DataSync::LocationFSxLustre", "Properties": {"FsxFilesystemArn": ..., "SecurityGroupArns": [...]}}.
2. Call CreateStack(StackName=<name>, TemplateBody=<template>) -> 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DataSync::LocationFSxLustre]".
3. Repeat with {"Type": "AWS::DataSync::LocationFSxOpenZFS", ...} -> the same refusal naming AWS::DataSync::LocationFSxOpenZFS.
