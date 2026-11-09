Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__mediapackage-vod.deploy-packaging-group-and-configuration-round-trip-and-delete cloudformation__mediapackage-vod.deploy-packaging-configuration-invalid-packaging-group-id-rejected-and-rolled-back
Issue-Key: cloudformation__mediapackage-vod|CreateStack|ValidationError
Issue-Commit: 0c7f0017b5788ebb0b2bf7aa1a6523722bad5325

# cloudformation__mediapackage-vod: AWS::MediaPackage::PackagingGroup and AWS::MediaPackage::PackagingConfiguration are not registered resource types, so CreateStack refuses every MediaPackage VOD template

> _Note: this issue was identified against the code base at commit `0c7f0017b5788ebb0b2bf7aa1a6523722bad5325`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry in the emulator does not carry the MediaPackage VOD types AWS::MediaPackage::PackagingGroup and AWS::MediaPackage::PackagingConfiguration, so template validation on CreateStack rejects the whole template before any resource is provisioned. Failing checks: cloudformation__mediapackage-vod.deploy-packaging-group-and-configuration-round-trip-and-delete, cloudformation__mediapackage-vod.deploy-packaging-configuration-invalid-packaging-group-id-rejected-and-rolled-back.

AWS: both types are real, publicly documented CloudFormation resource types (aws-resource-mediapackage-packaginggroup, aws-resource-mediapackage-packagingconfiguration). AWS accepts such a template and returns a StackId from CreateStack; the Smithy model for CreateStack declares only AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException and TokenAlreadyExistsException (400) — no validation refusal is modelled for a type AWS supports, and 'Template format error: Unrecognized resource types: [...]' is AWS's answer only for a type that genuinely is not in the registry (fixed text 'Template format error: Unrecognized resource types: [' + the comma-joined list of the unknown types from the request + ']'). With these types registered AWS then provisions them through the MediaPackage VOD control plane: the group's and configuration's Ref is the declared Id, Fn::GetAtt Arn/DomainName return the values MediaPackage VOD reports for that resource, DescribePackagingGroup/DescribePackagingConfiguration read the stack-created resources back, DeleteStack removes both (after which MediaPackage VOD answers NotFoundException 'PackagingGroup with id=<id> not found' / 'PackagingConfig with id=<id> not found'), and a PackagingConfiguration naming a PackagingGroupId that does not exist takes the stack to ROLLBACK_COMPLETE with the Config resource's ResourceStatusReason carrying the registry-handler form 'Resource handler returned message: "Resource of type '\''AWS::MediaPackage::PackagingGroup'\'' with identifier '\''<packaging-group-id>'\'' was not found." (RequestToken: <uuid>, HandlerErrorCode: NotFound)' — there the resource type name and the identifier come from the request, the RequestToken is per-run, and the rest is fixed text.

Emulator: CreateStack answers 400 ValidationError with message 'Template format error: Unrecognized resource types: [AWS::MediaPackage::PackagingGroup, AWS::MediaPackage::PackagingConfiguration]' (and 'Template format error: Unrecognized resource types: [AWS::MediaPackage::PackagingConfiguration]' for the single-resource template). No stack is created, so nothing downstream — Ref/GetAtt outputs, the MediaPackage VOD read-back, the stack delete, and the NotFound rollback path for a missing packaging group — can be observed at all.

Why: any client deploying MediaPackage VOD through infrastructure-as-code (CDK, SAM, raw templates) cannot create a stack at all. The refusal is also misleading: it tells the client the resource type does not exist in CloudFormation, which for AWS is false, so a client cannot distinguish an emulator gap from a genuine template error.

Fix: register both types in CloudFormation's resource-type registry and add the provisioning handlers that bind them to the MediaPackage VOD service state — create/read/delete for PackagingGroup and PackagingConfiguration, the Ref (declared Id) and GetAtt (Arn, DomainName) attribute resolution, and the NotFound handler-error path when PackagingGroupId names no existing group so the stack rolls back with the handler reason above. This belongs in the CloudFormation crate's resource-type registry plus its MediaPackage VOD resource handlers, not in the MediaPackage VOD operation handlers themselves (the VOD control-plane operations the handlers must call already work: 20/22 mediapackage-vod checks pass).

Traces: the two failing checks listed above, both erroring at the CreateStack call in the harness's create_stack helper.

## Evidence
Expected (AWS): cfn.create_stack(StackName=..., TemplateBody=<template declaring AWS::MediaPackage::PackagingGroup and AWS::MediaPackage::PackagingConfiguration>) returns 200 with a StackId; DescribeStacks reaches CREATE_COMPLETE; Outputs GroupId/ConfigId equal the declared Ids, GroupArn/GroupDomainName/ConfigArn equal what DescribePackagingGroup/DescribePackagingConfiguration report; DeleteStack removes both resources. For a configuration whose PackagingGroupId names no group: stack ends ROLLBACK_COMPLETE and the Config event's ResourceStatusReason is 'Resource handler returned message: "Resource of type '\''AWS::MediaPackage::PackagingGroup'\'' with identifier '\''<missing-group-id>'\'' was not found." (RequestToken: <uuid>, HandlerErrorCode: NotFound)'.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaPackage::PackagingGroup, AWS::MediaPackage::PackagingConfiguration]
and
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::MediaPackage::PackagingConfiguration]

In the diverging message, 'Template format error: Unrecognized resource types: [' and ']' are the fixed text and the bracketed type list is taken from the request; the defect is that these two types are classified as unrecognized at all.

## Steps to Reproduce
1. Boot the emulator and point a botocore cloudformation client at it.
2. CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Group":{"Type":"AWS::MediaPackage::PackagingGroup","Properties":{"Id":"<group-id>"}},"Config":{"Type":"AWS::MediaPackage::PackagingConfiguration","Properties":{"Id":"<config-id>","PackagingGroupId":{"Ref":"Group"},"HlsPackage":{"HlsManifests":[{}]}}}},"Outputs":{"GroupId":{"Value":{"Ref":"Group"}},"GroupArn":{"Value":{"Fn::GetAtt":["Group","Arn"]}},"GroupDomainName":{"Value":{"Fn::GetAtt":["Group","DomainName"]}},"ConfigId":{"Value":{"Ref":"Config"}},"ConfigArn":{"Value":{"Fn::GetAtt":["Config","Arn"]}}}}.
3. Observe 400 ValidationError 'Template format error: Unrecognized resource types: [AWS::MediaPackage::PackagingGroup, AWS::MediaPackage::PackagingConfiguration]' instead of a StackId.
4. Repeat with only the PackagingConfiguration resource, giving PackagingGroupId an id no packaging group has: observe the same ValidationError instead of a stack that reaches ROLLBACK_COMPLETE with the NotFound handler reason.
