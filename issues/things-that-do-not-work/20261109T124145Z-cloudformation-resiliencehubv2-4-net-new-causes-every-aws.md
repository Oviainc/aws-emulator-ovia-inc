Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__resiliencehubv2.deploy-resiliencehubv2-policy-round-trip-then-stack-delete cloudformation__resiliencehubv2.deploy-resiliencehubv2-policy-availabilityslo-target-over-maximum-rejected cloudformation__resiliencehubv2.deploy-resiliencehubv2-service-and-servicefunction-round-trip-then-child-and-stack-delete cloudformation__resiliencehubv2.deploy-resiliencehubv2-system-and-userjourney-round-trip-then-child-and-stack-delete
Issue-Key: cloudformation__resiliencehubv2|CreateStack|ValidationError
Issue-Commit: ab58585e96332c13ed2b8d30023e29e5d03b1c39

# cloudformation__resiliencehubv2: 4 net-new causes — every AWS::ResilienceHubV2::* resource type is refused as "Unrecognized resource types", so no ResilienceHubV2 estate can be deployed, updated, validated or rolled back from a template

> _Note: this issue was identified against the code base at commit `ab58585e96332c13ed2b8d30023e29e5d03b1c39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
CloudFormation's template resource-type registry in this emulator knows none of the AWS::ResilienceHubV2::* types. Every CreateStack whose template declares one is refused at template-format validation, so all four deploy checks of this suite never get past their first call and none of the downstream provisioning, Ref/GetAtt, in-place update, early property validation or stack-delete behaviour is reachable. The four causes below are distinct missing capabilities, not one check repeated: each needs the type registered AND a provisioner that drives the matching ResilienceHubV2 control-plane operations.

Cause 1 of 4 — AWS::ResilienceHubV2::Policy is not a registered resource type (happy-path round trip).
AWS: CreateStack accepts a template whose resource Type is AWS::ResilienceHubV2::Policy (Properties Name, Description, AvailabilitySlo.Target) and answers 200 with a StackId; the stack reaches CREATE_COMPLETE, the resource's Ref resolves to the policy ARN, GetPolicy on that ARN returns the declared name/description and availabilitySlo {target: <declared target>}, ListPolicies lists it, the stack's propagated tags appear in ListTagsForResource, an UpdateStack that changes only Description updates the policy in place (createdAt unchanged, availabilitySlo untouched) and reaches UPDATE_COMPLETE, and DeleteStack removes the policy so GetPolicy answers ResourceNotFoundException "Policy not found: <policyId>" (fixed text "Policy not found: " plus the policy id taken from the request's ARN) and ListPolicies no longer lists it. Smithy (resiliencehubv2): AvailabilitySlo.target targets Double with @range min 0 max 100.
Emulator: CreateStack is refused 400 ValidationError with message "Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::Policy]" (fixed text "Template format error: Unrecognized resource types: [" + the unknown types from the template + "]").
Why: a client cannot provision a resilience policy declaratively at all; any CDK/CFN stack containing one fails at submit time, and the whole policy lifecycle (Ref output, in-place update, delete cascade to ResilienceHubV2) is unverifiable.
Fix: CloudFormation's template resource-type registry plus a new AWS::ResilienceHubV2::Policy resource provisioner (create/update/delete mapped onto CreatePolicy/UpdatePolicy/DeletePolicy, Ref -> policyArn) in the cloudformation service crate.
Traces: cloudformation__resiliencehubv2.deploy-resiliencehubv2-policy-round-trip-then-stack-delete.

Cause 2 of 4 — no early property validation for AWS::ResilienceHubV2::Policy, because the type is unrecognized (rejection path).
AWS: a template declaring AWS::ResilienceHubV2::Policy with AvailabilitySlo.Target above the modeled maximum is ACCEPTED by CreateStack (200 + StackId) and then fails CloudFormation's resource-schema property validation: the stack ends ROLLBACK_COMPLETE, DescribeEvents reports one VALIDATION_ERROR OperationEvent for the resource's logical id with ValidationName PROPERTY_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationStatusReason of the form "#/<property path>: <supplied value> is not less or equal to <maximum>" and ValidationPath "/Resources/<LogicalId>/Properties/AvailabilitySlo/Target"; no policy is created (ListPolicies never lists the name the rolled-back policy would have had). The fixed text is the schema-validation wording; the property path, supplied value and limit come from the request and the resource schema (max 100 per Smithy AvailabilitySlo.target @range max 100). The same template with an in-range Target reaches CREATE_COMPLETE.
Emulator: both the control and the rejection stack are refused at submit with 400 ValidationError "Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::Policy]", so there is no ROLLBACK_COMPLETE, no VALIDATION_ERROR OperationEvent, and no distinction between a valid and an invalid property value.
Why: a client that relies on CloudFormation catching an out-of-range property before any resource is touched gets a submit-time template error instead of a validated rollback, and gets no per-property diagnosis (name/path/reason) to act on.
Fix: the AWS::ResilienceHubV2::Policy resource schema + CloudFormation's property-validation stage and its VALIDATION_ERROR operation-event emission.
Traces: cloudformation__resiliencehubv2.deploy-resiliencehubv2-policy-availabilityslo-target-over-maximum-rejected.

Cause 3 of 4 — AWS::ResilienceHubV2::Service and AWS::ResilienceHubV2::ServiceFunction are not registered resource types.
AWS: CreateStack accepts a template declaring AWS::ResilienceHubV2::Service (Name, Regions, DependencyDiscovery, PermissionModel.InvokerRoleName referencing an AWS::IAM::Role in the same stack) and a child AWS::ResilienceHubV2::ServiceFunction (ServiceArn, Name, Criticality), reaching CREATE_COMPLETE; the Service's Ref is its service ARN and the ServiceFunction's Ref is the composite "<serviceArn>|<serviceFunctionId>"; GetService returns the declared name, regions, dependencyDiscovery.status and permissionModel.invokerRoleName, the stack tags are visible through ListTagsForResource, ListServices lists it, and ListServiceFunctions(serviceArn) returns exactly the declared function with its name and criticality. Dropping the ServiceFunction from the template updates in place: UPDATE_COMPLETE, ListServiceFunctions empty, Service createdAt unchanged. DeleteStack removes the Service, so GetService answers ResourceNotFoundException "Service not found: <serviceId>" (fixed text plus the id from the request ARN) and ListServices no longer lists it.
Emulator: CreateStack is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::Service, AWS::ResilienceHubV2::ServiceFunction]".
Why: no resilience Service or ServiceFunction can be created by a stack, so the child-before-parent update ordering and the stack-delete cascade into ResilienceHubV2 are both unavailable to clients.
Fix: register both types and add their provisioners (CreateService/UpdateService/DeleteService and CreateServiceFunction/DeleteServiceFunction), including the composite Ref form for the child, in the cloudformation service crate.
Traces: cloudformation__resiliencehubv2.deploy-resiliencehubv2-service-and-servicefunction-round-trip-then-child-and-stack-delete.

Cause 4 of 4 — AWS::ResilienceHubV2::System and AWS::ResilienceHubV2::UserJourney are not registered resource types.
AWS: CreateStack accepts a template declaring AWS::ResilienceHubV2::System (Name, Description) and a child AWS::ResilienceHubV2::UserJourney (SystemIdentifier, Name, Description) and reaches CREATE_COMPLETE; the System's Ref is its system ARN and Fn::GetAtt "SystemId" is the system id returned by GetSystem; the UserJourney's Ref is "<systemArn>|<userJourneyId>"; GetSystem/ListSystems and GetUserJourney/ListUserJourneys return the declared names and descriptions, and the stack tags propagate. Dropping the UserJourney updates in place (UPDATE_COMPLETE, GetUserJourney answers ResourceNotFoundException "User journey not found: <userJourneyId>", System createdAt unchanged), and DeleteStack removes the System so GetSystem answers ResourceNotFoundException "System not found: <systemId>". In each message the text before the colon is fixed and the identifier after it comes from the request.
Emulator: CreateStack is refused 400 ValidationError "Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::System, AWS::ResilienceHubV2::UserJourney]". Note the underlying CreateSystem control-plane operation is itself an unimplemented scaffold stub (separately filed as 20261029T191749Z-resiliencehubv2-createsystem-stub.md); this cause is the CloudFormation-side gap and needs the registry entry, the GetAtt SystemId attribute and the provisioner in addition to that stub.
Why: no System/UserJourney estate can be deployed declaratively, so the GetAtt attribute contract and the child-removal/stack-delete lifecycle are unreachable.
Fix: register both types, add their provisioners (CreateSystem/UpdateSystem/DeleteSystem, CreateUserJourney/DeleteUserJourney), the SystemId GetAtt attribute and the composite child Ref, in the cloudformation service crate.
Traces: cloudformation__resiliencehubv2.deploy-resiliencehubv2-system-and-userjourney-round-trip-then-child-and-stack-delete.

Related but not covering this: several open issues record the same registry gap for other families (e.g. 20261109T113556Z-cloudformation-omics-createstack-refuses-every-aws-omics-resource.md, 20261109T113214Z-cloudformation-mediatailor-createstack-refuses-every-aws-mediatailor-resource.md). None of them registers AWS::ResilienceHubV2::*.

## Evidence
All four checks fail on their first CreateStack call.

Expected (AWS): HTTP 200, CreateStackOutput with a StackId; the stack then settles CREATE_COMPLETE (causes 1, 3, 4) or ROLLBACK_COMPLETE with a PROPERTY_VALIDATION VALIDATION_ERROR operation event (cause 2).

Actual (emulator):
- botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::Policy]  — causes 1 and 2
- ... Unrecognized resource types: [AWS::ResilienceHubV2::Service, AWS::ResilienceHubV2::ServiceFunction]  — cause 3
- ... Unrecognized resource types: [AWS::ResilienceHubV2::System, AWS::ResilienceHubV2::UserJourney]  — cause 4

Message anatomy: "Template format error: Unrecognized resource types: [" + comma-separated unknown types from the submitted template + "]" is the emulator's refusal; AWS emits no such refusal for these types at all. Smithy (resiliencehubv2) AvailabilitySlo.target: Double, @range min 0 max 100 — the bound cause 2's expected validation reason cites ("150 is not less or equal to 100": the fixed form is "#/<path>: <value> is not less or equal to <max>", with value from the request and max from the schema).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cloudformation.create_stack(StackName=<name>, TemplateBody=json.dumps({"AWSTemplateFormatVersion": "2010-09-09", "Resources": {"Pol": {"Type": "AWS::ResilienceHubV2::Policy", "Properties": {"Name": <policy name>, "AvailabilitySlo": {"Target": 99.9}}}}, "Outputs": {"PolArn": {"Value": {"Ref": "Pol"}}}}))
   -> observe ValidationError "Template format error: Unrecognized resource types: [AWS::ResilienceHubV2::Policy]" instead of a StackId.
3. Repeat with Target above the schema maximum (e.g. 150): AWS accepts the submit and ends ROLLBACK_COMPLETE with a PROPERTY_VALIDATION VALIDATION_ERROR event; the emulator gives the same submit-time refusal.
4. Repeat with Resources containing AWS::ResilienceHubV2::Service + AWS::ResilienceHubV2::ServiceFunction (Service referencing an in-stack AWS::IAM::Role via PermissionModel.InvokerRoleName), and with AWS::ResilienceHubV2::System + AWS::ResilienceHubV2::UserJourney (Outputs using Ref and Fn::GetAtt ["Sys", "SystemId"]) -> the same refusal naming those types.
