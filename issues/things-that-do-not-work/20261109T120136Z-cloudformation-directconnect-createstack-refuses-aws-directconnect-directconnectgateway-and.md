Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__directconnect.deploy-directconnect-gateway-and-resiliency-group-round-trip-and-delete cloudformation__directconnect.deploy-directconnect-resiliency-group-invalid-model-rejected-and-rolled-back
Issue-Key: cloudformation__directconnect|CreateStack|ValidationError
Issue-Commit: 70b7c68145c683ae49b96a2a3a7ef8edad56294d

# cloudformation__directconnect: CreateStack refuses AWS::DirectConnect::DirectConnectGateway and AWS::DirectConnect::ResiliencyGroup as unrecognized resource types, so no Direct Connect template can be deployed

> _Note: this issue was identified against the code base at commit `70b7c68145c683ae49b96a2a3a7ef8edad56294d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause behind both surviving cloudformation__directconnect failures.

Cause 1 of 1: the CloudFormation resource-type registry does not know any AWS::DirectConnect::* type, so CreateStack is refused up-front with a template-format ValidationError and no stack is ever minted.

AWS: CloudFormation supports AWS::DirectConnect::DirectConnectGateway and AWS::DirectConnect::ResiliencyGroup, and provisions them through Direct Connect's own control plane — which this emulator models. Smithy (com-amazonaws-directconnect-overtureservice): CreateDirectConnectGateway(directConnectGatewayName required, amazonSideAsn LongAsn, tags TagList) -> DirectConnectGateway; CreateResiliencyGroup(resiliencyGroupName required @length 1..255 @pattern ^[a-zA-Z0-9_\-]+$, intendedResiliencyModel required enum ResiliencyModel, tags TagList) -> ResiliencyGroup; plus DescribeDirectConnectGateways, ListResiliencyGroups, GetResiliencyGroup and DescribeTags for the read-back. ResiliencyModel values are exactly [maximum-resiliency, high-resiliency, basic-resiliency]. So a template declaring either type is ACCEPTED by CreateStack: it returns a StackId, the stack reaches CREATE_COMPLETE, Ref yields the resource ARN (arn:<partition>:directconnect::<account>:dx-gateway/<id> and :dx-resiliency-group/<id>) and Fn::GetAtt DirectConnectGatewayId / ResiliencyGroupId yield the service-minted ids, which the Direct Connect reads then confirm (recorded behavior only for the per-property mapping and for the deleted-resource tombstones: a deleted gateway/group stays describable with state 'deleted' rather than answering NotFound). For the negative check, AWS fails the RESOURCE, not the template: an IntendedResiliencyModel outside the enum makes the stack reach CREATE_FAILED/ROLLBACK_COMPLETE with the stack-level reason 'Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.' and a DescribeEvents VALIDATION_ERROR row for ResourceType AWS::DirectConnect::ResiliencyGroup carrying the PROPERTY_VALIDATION reason '<REDACTED> is not a valid enum value. Supported values: [maximum-resiliency, high-resiliency, basic-resiliency]' at /Resources/<LogicalId>/Properties/IntendedResiliencyModel — never a template-format error, because the type itself is recognized.

Emulator: CreateStack answers HTTP 400 with error code ValidationError and the message 'Template format error: Unrecognized resource types: [<types>]'. The fixed text is 'Template format error: Unrecognized resource types: ' followed by the bracketed list of the template's unknown types taken from the request; across these two checks that list is [AWS::DirectConnect::DirectConnectGateway, AWS::DirectConnect::ResiliencyGroup] and [AWS::DirectConnect::ResiliencyGroup]. No StackId is returned, so every downstream assertion (DescribeStacks status, Outputs/Ref/GetAtt, the Direct Connect round-trip reads, the tag reads, the enum-rejection event rows, the delete-time 'deleted' tombstones) is unreachable.

Why: a client cannot deploy Direct Connect infrastructure as code at all. Any CDK/CloudFormation estate containing a Direct Connect gateway or a resiliency group fails at CreateStack with a template-format error, which reads as 'my template is malformed' rather than 'this provider is missing'. The negative path is additionally mis-shaped: a template-format refusal at CreateStack is a different failure class, status path and error surface from AWS's resource-level enum validation, so a client testing invalid-input rollback observes no CREATE_FAILED/ROLLBACK_COMPLETE, no stack events and no per-property validation reason.

Fix: the CloudFormation resource-type registry and its provisioners — register AWS::DirectConnect::DirectConnectGateway and AWS::DirectConnect::ResiliencyGroup and wire each to the corresponding Direct Connect control-plane operation for create/read/update/delete, publishing the Direct Connect ARN as the physical resource id so Ref resolves to the ARN and GetAtt resolves to DirectConnectGatewayId / ResiliencyGroupId, and routing property validation through the modeled constraints (ResiliencyModel enum) so a bad value fails the resource rather than the template.

Traces: cloudformation__directconnect.deploy-directconnect-gateway-and-resiliency-group-round-trip-and-delete, cloudformation__directconnect.deploy-directconnect-resiliency-group-invalid-model-rejected-and-rolled-back.

Note: no open issue covers this. The closest open issues report the same class of registry gap for other services only (e.g. 20261109T113214Z-cloudformation-mediatailor-createstack-refuses-every-aws-mediatailor-resource.md, 20261109T113428Z-cloudformation-mwaa-createstack-refuses-aws-mwaa-environment-as.md); neither names any AWS::DirectConnect::* type.

## Evidence
From the behavioral replay, both checks [error] on their first call, CreateStack:

- deploy-directconnect-gateway-and-resiliency-group-round-trip-and-delete: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DirectConnect::DirectConnectGateway, AWS::DirectConnect::ResiliencyGroup]
- deploy-directconnect-resiliency-group-invalid-model-rejected-and-rolled-back: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DirectConnect::ResiliencyGroup]

Expected (round-trip check): CreateStack returns a StackId; the stack reaches CREATE_COMPLETE; Outputs give Ref Dgw = arn:<partition>:directconnect::<account>:dx-gateway/<GetAtt DirectConnectGatewayId> and Ref Rg = arn:<partition>:directconnect::<account>:dx-resiliency-group/<GetAtt ResiliencyGroupId>; DescribeDirectConnectGateways reports the gateway available with the declared name and amazonSideAsn; GetResiliencyGroup/ListResiliencyGroups report the group available with the declared name, matching ARN and the template tag; after DeleteStack both stay describable with state 'deleted'.
Expected (negative check): CreateStack is accepted and the stack reaches ROLLBACK_COMPLETE with zero created resources, a stack-level CREATE_FAILED reason 'Validation failed with 1 error(s). ...', a DescribeEvents VALIDATION_ERROR row for AWS::DirectConnect::ResiliencyGroup, and the PROPERTY_VALIDATION row '<REDACTED> is not a valid enum value. Supported values: [maximum-resiliency, high-resiliency, basic-resiliency]' at /Resources/Rg/Properties/IntendedResiliencyModel; the identical template with basic-resiliency still reaches CREATE_COMPLETE.
Actual: ValidationError 'Template format error: Unrecognized resource types: [...]' at CreateStack, no stack exists, nothing downstream is exercised.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the checks in trace_ids.
3. Minimal repro: cloudformation.create_stack(StackName=<name>, TemplateBody=<a template whose Resources contain a single {"Type": "AWS::DirectConnect::DirectConnectGateway", "Properties": {"DirectConnectGatewayName": <name>, "AmazonSideAsn": "64512"}}>) and observe ValidationError 'Template format error: Unrecognized resource types: [AWS::DirectConnect::DirectConnectGateway]' instead of a StackId. Repeat with {"Type": "AWS::DirectConnect::ResiliencyGroup", "Properties": {"ResiliencyGroupName": <name>, "IntendedResiliencyModel": "basic-resiliency"}}.
4. Then repeat the ResiliencyGroup template with IntendedResiliencyModel "nonexistent-resiliency-model" and observe that the refusal is the same template-format ValidationError rather than an accepted stack that rolls back with a per-property enum validation error.
