Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__schemas.deploy-eventschemas-discoverer-round-trip-and-delete cloudformation__events__schemas.deploy-eventschemas-discoverer-invalid-source-arn-rejected-and-rolled-back
Issue-Key: cloudformation__events__schemas|CreateStack|ValidationError
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# cloudformation__events__schemas: CreateStack refuses AWS::EventSchemas::Discoverer as an unrecognized resource type, so no EventBridge schema discoverer can be provisioned from a template (2 checks)

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause behind both surviving cloudformation__events__schemas failures.

Cause 1 of 1: the CloudFormation resource-type registry does not know the type AWS::EventSchemas::Discoverer, so CreateStack is refused up-front with a template-format ValidationError and no stack is ever created. Failing checks: cloudformation__events__schemas.deploy-eventschemas-discoverer-round-trip-and-delete, cloudformation__events__schemas.deploy-eventschemas-discoverer-invalid-source-arn-rejected-and-rolled-back.

AWS: AWS::EventSchemas::Discoverer is a CloudFormation-supported resource type whose provider drives the Schemas control plane, which this emulator models. Smithy (com-amazonaws-schemas-schemas): CreateDiscoverer POST /v1/discoverers with required SourceArn (@length min 20, max 1600), optional Description (@length 0..256) and CrossAccount (boolean), returning DiscovererArn, DiscovererId, SourceArn and State (enum DiscovererState); DescribeDiscoverer, ListDiscoverers, UpdateDiscoverer and DeleteDiscoverer (DELETE /v1/discoverers/id/{DiscovererId}) are all bound operations too. So AWS accepts a template declaring that type: CreateStack returns a StackId, the stack reaches CREATE_COMPLETE, Ref resolves to the discoverer's ARN and Fn::GetAtt resolves DiscovererArn, DiscovererId and State, and ListDiscoverers(SourceArnPrefix=<bus arn>) then reports the discoverer. For the rejection check AWS likewise ACCEPTS the template — the type is recognized — and fails the RESOURCE: the stack goes CREATE_FAILED then ROLLBACK_COMPLETE, DescribeStackEvents carries no template-validation row, and the Discoverer resource's ResourceStatusReason is the provider wrapper 'Resource handler returned message: "<service message>" (RequestToken: <token>, HandlerErrorCode: <code>)' whose inner message is Schemas' own refusal — recorded behavior: the fixed text 'Source ARN must be a valid ARN. (Service: Schemas, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)' with HandlerErrorCode InvalidRequest (the request id and attempt count are per-run values, not part of the requirement). Rollback then removes both the discoverer and the stack's AWS::Events::EventBus, so DescribeEventBus answers ResourceNotFoundException 'Event bus <name> does not exist.' with <name> taken from the request.

Emulator: CreateStack answers HTTP 400 with error code ValidationError and the message 'Template format error: Unrecognized resource types: [AWS::EventSchemas::Discoverer]'. The fixed text is 'Template format error: Unrecognized resource types: ' followed by the bracketed list of the template's unknown types, taken from the request; here that list is the single type AWS::EventSchemas::Discoverer. No StackId is minted, so DescribeStacks, DescribeStackEvents, the Outputs (DiscovererRef/DiscovererArn/DiscovererId/DiscovererState), the ListDiscoverers round-trip, the rollback assertions and the delete are all unreachable.

Why: a client cannot deploy an EventBridge schema discoverer as infrastructure as code at all — any CDK/CloudFormation estate containing one fails at CreateStack with a message that reads as 'my template is malformed' rather than 'this provider is missing'. The rejection path is additionally mis-shaped: a template-format refusal at CreateStack is a different failure class, at a different time, from a resource-level provider failure, so a client that tests invalid-input rollback never sees CREATE_FAILED/ROLLBACK_COMPLETE, never sees a per-resource ResourceStatusReason, and cannot distinguish a bad property value from an unparsable template.

Fix: the CloudFormation resource-type registry and its provisioners — register AWS::EventSchemas::Discoverer and wire its create/read/update/delete to the Schemas control plane (CreateDiscoverer / DescribeDiscoverer / UpdateDiscoverer / DeleteDiscoverer), publishing the discoverer ARN as the physical resource id so Ref resolves to it and GetAtt exposes DiscovererArn, DiscovererId and State, and surfacing the Schemas 400 refusal as the resource's handler failure reason instead of pre-validating the type away.

Traces: cloudformation__events__schemas.deploy-eventschemas-discoverer-round-trip-and-delete, cloudformation__events__schemas.deploy-eventschemas-discoverer-invalid-source-arn-rejected-and-rolled-back.

Note: no open issue covers this type. The nearest open issues report the same CLASS of gap for other providers (e.g. 20261109T113214Z-cloudformation-mediatailor-createstack-refuses-every-aws-mediatailor-resource.md, 20261109T113428Z-cloudformation-mwaa-createstack-refuses-aws-mwaa-environment-as.md) and name no AWS::EventSchemas::* type.

## Evidence
From the behavioral replay, both checks [error] on their first CloudFormation call, CreateStack:

- cloudformation__events__schemas.deploy-eventschemas-discoverer-round-trip-and-delete: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EventSchemas::Discoverer]
- cloudformation__events__schemas.deploy-eventschemas-discoverer-invalid-source-arn-rejected-and-rolled-back: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::EventSchemas::Discoverer]

Expected (round-trip check): CreateStack returns a StackId; the stack reaches CREATE_COMPLETE; DescribeEventBus(Name=<bus>)["Arn"] equals the stack output BusArn; ListDiscoverers(SourceArnPrefix=<BusArn>) reports exactly one discoverer whose (DiscovererArn, DiscovererId) equal the stack outputs DiscovererRef and DiscovererId.
Expected (rejection check): CreateStack returns a StackId; the stack reaches ROLLBACK_COMPLETE; DescribeStackEvents carries no template-validation row; the Discoverer event's ResourceStatusReason is 'Resource handler returned message: "Source ARN must be a valid ARN. (Service: Schemas, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'; afterwards DescribeEventBus(Name=<bad bus>) is refused ResourceNotFoundException 'Event bus <bad bus> does not exist.' and ListDiscoverers lists no discoverer for the bad SourceArn.
Actual: ValidationError 'Template format error: Unrecognized resource types: [AWS::EventSchemas::Discoverer]' at CreateStack; no stack exists and nothing downstream is exercised.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the two checks listed in trace_ids.
3. Minimal repro: cloudformation.create_stack(StackName='t', TemplateBody=json.dumps({"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Bus": {"Type": "AWS::Events::EventBus", "Properties": {"Name": "<bus>"}}, "Discoverer": {"Type": "AWS::EventSchemas::Discoverer", "Properties": {"SourceArn": {"Fn::GetAtt": ["Bus", "Arn"]}, "Description": "d", "CrossAccount": False}}}, "Outputs": {"DiscovererRef": {"Value": {"Ref": "Discoverer"}}}})) and observe ValidationError 'Template format error: Unrecognized resource types: [AWS::EventSchemas::Discoverer]' instead of a StackId.
4. Repeat with SourceArn set to a non-ARN string (e.g. 'not-an-event-bus-arn-xxxxxxxxxxxxxxxxxxxx') and observe the same template-format refusal instead of a stack that reaches ROLLBACK_COMPLETE with the Schemas handler reason on the Discoverer resource.
