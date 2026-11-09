Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__codestar-connections.deploy-cfn-codestarconnections-connection-resource-lifecycle cloudformation__codestar-connections.deploy-cfn-codestarconnections-connection-invalid-provider-type-rejected
Issue-Key: cloudformation__codestar-connections|CreateStack|ValidationError
Issue-Commit: 4cb124a92bc472448f3b420460157b630c09e49d

# cloudformation__codestar-connections: AWS::CodeStarConnections::Connection is not a registered resource type — CreateStack refuses every Connection template with ValidationError "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `4cb124a92bc472448f3b420460157b630c09e49d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry in the emulator does not know AWS::CodeStarConnections::Connection, so a template declaring one is rejected at CreateStack — before any stack, event or connection is ever created. Both behavioral checks in this suite (the happy-path lifecycle one and the invalid-ProviderType rejection one) die on the very first CreateStack call, so nothing downstream of template validation (stack status, Ref/Fn::GetAtt resolution, the CodeStarConnections handler, deletion propagation) is reachable at all.

AWS: AWS::CodeStarConnections::Connection is a long-standing public CloudFormation resource type (Ref -> the connection ARN; Fn::GetAtt ConnectionArn, ConnectionStatus, OwnerAccountId). Real CloudFormation accepts a template declaring it, returns a StackId from CreateStack, drives the resource to CREATE_COMPLETE, and mints a real connection reachable through codestar-connections GetConnection/ListConnections (ConnectionStatus PENDING until a handshake completes, OwnerAccountId = the creating account). Deleting the stack deletes the connection, after which GetConnection answers ResourceNotFoundException. On an invalid ProviderType, CloudFormation does NOT refuse the template: the stack is created, the Conn resource goes CREATE_FAILED, and the resource's ResourceStatusReason is CodeStarConnections' own validation error wrapped by the resource handler as: Resource handler returned message: "1 validation error detected: Value '<providerType>' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitLabSelfManaged, GitHubEnterpriseServer, Bitbucket] (Service: CodeStarConnections, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException) — the bracketed provider list and the fixed prose are the requirement; the quoted value, request id, request token and attempt count are per-request values. Smithy: codestar-connections CreateConnection.ProviderType targets the ProviderType enum whose values are Bitbucket, GitHub, GitHubEnterpriseServer, GitLab, GitLabSelfManaged; GetConnection error ResourceNotFoundException. The CreateStack-level refusal here is recorded behavior only in the sense that AWS never produces it for this type.

Emulator: CreateStack fails immediately with HTTP 400, error code ValidationError and message "Template format error: Unrecognized resource types: [AWS::CodeStarConnections::Connection]". The fixed text is "Template format error: Unrecognized resource types: [...]" and the bracketed list is the set of types from the request the registry did not resolve — here the single type the template declares. No StackId is returned, so the stack does not exist and no stack events, outputs or resources are produced.

Why: a client that deploys a CodeStar Connections connection through CloudFormation (CDK, SAM, or a raw template — the normal way these connections are provisioned for CodePipeline source stages) cannot deploy at all. The failure arrives as a template-format ValidationError, which a client reads as "my template is malformed" rather than "this resource type is unsupported here", so it misdirects debugging. It also hides the negative path entirely: a template with a bogus ProviderType must create the stack and fail only the resource with CodeStarConnections' validation message, so any test or rollback logic keyed on CREATE_FAILED plus that handler reason never runs.

Fix: register AWS::CodeStarConnections::Connection in the CloudFormation emulator's resource-type registry/template-validation step and give it a resource handler that calls through to the codestar-connections state: create -> CreateConnection (PhysicalResourceId and Ref = the connection ARN, GetAtt ConnectionArn/ConnectionStatus/OwnerAccountId served from the created connection, ConnectionStatus PENDING, OwnerAccountId the creating account), delete -> DeleteConnection so GetConnection afterwards answers ResourceNotFoundException, and propagate the service's ProviderType validation failure as a CREATE_FAILED resource event whose ResourceStatusReason carries the handler-wrapped CodeStarConnections message with HandlerErrorCode GeneralServiceException (leaving no PhysicalResourceId and no connection in ListConnections).

Traces: both failing checks below refuse at the first CreateStack with the same ValidationError, so they are examples of this single registry gap.

## Evidence
Expected (AWS):
- cfn.create_stack(TemplateBody=<template declaring Resources.Conn of Type AWS::CodeStarConnections::Connection>) returns a StackId; DescribeStacks reaches CREATE_COMPLETE; DescribeStackResource(LogicalResourceId="Conn").PhysicalResourceId is the connection ARN; Outputs Ref/GetAtt ConnectionArn both equal that ARN; GetConnection on it returns ConnectionName/ProviderType=GitHub/ConnectionStatus=PENDING/OwnerAccountId=<account>; after DeleteStack, GetConnection -> ResourceNotFoundException "No connection exists with the ARN".
- With ProviderType="NotARealProvider": CreateStack still succeeds; stack status CREATE_FAILED; the Conn CREATE_FAILED event reason is Resource handler returned message: "1 validation error detected: Value 'NotARealProvider' at 'providerType' failed to satisfy constraint: Member must be one of: [GitLab, GitHub, GitLabSelfManaged, GitHubEnterpriseServer, Bitbucket] (Service: CodeStarConnections, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <uuid>, HandlerErrorCode: GeneralServiceException); DescribeStackResource returns no PhysicalResourceId and ListConnections does not list the name.

Actual (emulator), identical for both checks:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::CodeStarConnections::Connection]
HTTP 400, Code=ValidationError. No StackId is returned, so every later assertion in both checks is unreachable.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cf = client("cloudformation"); template = {"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Conn": {"Type": "AWS::CodeStarConnections::Connection", "Properties": {"ConnectionName": "<unique-name>", "ProviderType": "GitHub"}}}}
3. cf.create_stack(StackName="<unique-stack>", TemplateBody=json.dumps(template)) -> ClientError ValidationError "Template format error: Unrecognized resource types: [AWS::CodeStarConnections::Connection]" instead of a StackId.
4. Repeat step 3 with Properties.ProviderType = "NotARealProvider": the same CreateStack ValidationError is raised, instead of the stack being created and the Conn resource reaching CREATE_FAILED with CodeStarConnections' providerType validation message wrapped by the resource handler.
