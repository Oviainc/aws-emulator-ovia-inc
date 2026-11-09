Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__groundstation.deploy-groundstation-config-invalid-name-pattern-rejected cloudformation__groundstation.deploy-groundstation-config-round-trip-and-delete cloudformation__groundstation.deploy-groundstation-dataflowendpointgroup-and-missionprofile-round-trip-and-delete cloudformation__groundstation.deploy-groundstation-dataflowendpointgroupv2-round-trip-and-delete
Issue-Key: cloudformation__groundstation|CreateStack|ValidationError
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# cloudformation__groundstation: no AWS::GroundStation::* resource type is registered — CreateStack refuses every Ground Station template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause blocks all four CloudFormation+GroundStation deploy checks: the emulator's CloudFormation resource-type registry knows no AWS::GroundStation::* type, so CreateStack rejects the template outright and nothing downstream (provisioning, Ref/GetAtt, schema validation, rollback, stack delete) is ever reached.

Cause 1 of 1: AWS::GroundStation::Config, AWS::GroundStation::DataflowEndpointGroup, AWS::GroundStation::MissionProfile and AWS::GroundStation::DataflowEndpointGroupV2 are refused as unrecognized resource types.
Failing check ids: cloudformation__groundstation.deploy-groundstation-config-invalid-name-pattern-rejected, cloudformation__groundstation.deploy-groundstation-config-round-trip-and-delete, cloudformation__groundstation.deploy-groundstation-dataflowendpointgroup-and-missionprofile-round-trip-and-delete, cloudformation__groundstation.deploy-groundstation-dataflowendpointgroupv2-round-trip-and-delete.

AWS: all four types are public CloudFormation registry types. AWS accepts CreateStack for a template declaring them, provisions the matching Ground Station resource, resolves Ref/Fn::GetAtt from it, and — when a property violates the resource schema — fails that resource and rolls the stack back with a registry-handler reason. Recorded behavior (the checks' own expectations, taken from the recording): the valid Config stack reaches CREATE_COMPLETE, Ref is the configArn and Fn::GetAtt "Id" the configId that GroundStation ListConfigs/GetConfig return under the declared Name; the DataflowEndpointGroup + MissionProfile stack and the DataflowEndpointGroupV2 stack likewise read back through GroundStation and are removed by DeleteStack; and a Config whose Name violates the resource's Name pattern ends ROLLBACK_COMPLETE with a DescribeStackEvents CREATE_FAILED ResourceStatusReason of the registry-handler form `Resource handler returned message: "<message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)` carrying message `Model validation failed (#/Name: failed validation constraint for keyword [pattern])` and code `InvalidRequest` (fixed text; the RequestToken is a per-request uuid and is not part of the requirement), with no config of that name left in ListConfigs. Smithy (services/service-com-amazonaws-cloudformation-cloudformation): CreateStack models AlreadyExistsException, InsufficientCapabilitiesException, LimitExceededException, TokenAlreadyExistsException; the ValidationError the emulator returns is the service-common template error, whose message `Template format error: Unrecognized resource types: [<types>]` has that fixed prefix plus the bracketed list of types taken from the request. That message is AWS's answer for a type the registry genuinely does not have, so returning it for these four is the defect, not the wording.

Emulator: CreateStack answers HTTP 400 ValidationError with `Template format error: Unrecognized resource types: [AWS::GroundStation::Config]` (and, per template, `[AWS::GroundStation::Config, AWS::GroundStation::DataflowEndpointGroup, AWS::GroundStation::MissionProfile]` and `[AWS::GroundStation::DataflowEndpointGroupV2]`). No stack is created, so no CREATE_COMPLETE, no Outputs, no handler validation and no rollback event exist.

Why: a client that declares Ground Station infrastructure in a template (or any CDK/SAM app that includes a Config, DataflowEndpointGroup, MissionProfile or DataflowEndpointGroupV2) cannot deploy it at all — the whole template is refused on submission, not just the Ground Station resource. Negative tests also break: callers that expect a schema-invalid Name to surface as a per-resource CREATE_FAILED/ROLLBACK_COMPLETE with an InvalidRequest handler reason instead get a synchronous ValidationError at CreateStack, a different failure mode at a different point in the workflow.

Fix: register the four AWS::GroundStation::* types in CloudFormation's resource-type registry and implement their resource handlers (create/read/update/delete against the GroundStation service state, Ref and Fn::GetAtt attribute resolution, tag propagation of stack-level Tags, and resource-schema property validation — including the Config Name pattern — reported as a registry-handler failure reason so the stack rolls back and leaves nothing behind).

Traces: the four check ids listed above are examples of this one cause.

## Evidence
Expected (AWS / recording): CreateStack accepts the template and returns a StackId; the stack settles CREATE_COMPLETE and its Ref/GetAtt outputs match what GroundStation ListConfigs/GetConfig, ListDataflowEndpointGroups/GetDataflowEndpointGroup and ListMissionProfiles/GetMissionProfile return; the bad-Name variant settles ROLLBACK_COMPLETE with CREATE_FAILED reason `Resource handler returned message: "Model validation failed (#/Name: failed validation constraint for keyword [pattern])" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)`.

Actual (emulator): botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::GroundStation::Config]
...: Template format error: Unrecognized resource types: [AWS::GroundStation::Config, AWS::GroundStation::DataflowEndpointGroup, AWS::GroundStation::MissionProfile]
...: Template format error: Unrecognized resource types: [AWS::GroundStation::DataflowEndpointGroupV2]
HTTP 400, error code ValidationError. Fixed text of that message is `Template format error: Unrecognized resource types: ` and the bracketed list is the request's types; no part of it is a per-run value.

## Steps to Reproduce
1. Boot the emulator and point a botocore client at it.
2. cloudformation.create_stack(StackName=<unique>, TemplateBody=json with Resources={"Config": {"Type": "AWS::GroundStation::Config", "Properties": {"Name": <valid name>, "ConfigData": {"TrackingConfig": {"Autotrack": "PREFERRED"}}}}} and Outputs Ref/Fn::GetAtt Id) -> observe ValidationError "Template format error: Unrecognized resource types: [AWS::GroundStation::Config]" instead of a StackId.
3. Repeat with a resource of Type AWS::GroundStation::DataflowEndpointGroup, AWS::GroundStation::MissionProfile and AWS::GroundStation::DataflowEndpointGroupV2 -> same refusal naming each type.
4. Replay the four checks in trace_ids.
