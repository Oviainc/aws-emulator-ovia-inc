Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__devicefarm.deploy-devicefarm-devicepool-invalid-rule-attribute-rejected cloudformation__devicefarm.deploy-devicefarm-devicepool-networkprofile-round-trip cloudformation__devicefarm.deploy-devicefarm-project-instanceprofile-testgridproject-round-trip cloudformation__devicefarm.deploy-devicefarm-upload-round-trip
Issue-Key: cloudformation__devicefarm|CreateStack|ValidationError
Issue-Commit: eaa123f65a60b02b9ee1461dec055a780335fce9

# cloudformation__devicefarm: no AWS::DeviceFarm::* resource type is registered — CreateStack refuses every Device Farm template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `eaa123f65a60b02b9ee1461dec055a780335fce9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry contains no AWS::DeviceFarm::* types, so CreateStack refuses any template that declares one and no Device Farm estate can ever be deployed, updated, rolled back or deleted through CloudFormation. The six types exercised by the failing checks are AWS::DeviceFarm::Project, ::DevicePool, ::NetworkProfile, ::InstanceProfile, ::TestGridProject and ::Upload.

AWS: AWS::DeviceFarm::Project, AWS::DeviceFarm::DevicePool, AWS::DeviceFarm::NetworkProfile, AWS::DeviceFarm::InstanceProfile, AWS::DeviceFarm::TestGridProject and AWS::DeviceFarm::Upload are supported CloudFormation resource types in the us-west-2 registry. CreateStack on a well-formed template declaring them is accepted (200, a StackId is returned) and the stack reaches CREATE_COMPLETE, having created the corresponding Device Farm resources: Ref returns each resource's ARN, Fn::GetAtt <logical>.Arn returns the same ARN, the stack's own Tags propagate to the taggable Device Farm resources alongside the template's own Tags, the resources read back through devicefarm (ListProjects/ListDevicePools/GetDevicePool/ListNetworkProfiles/GetNetworkProfile), an update that drops a child resource deletes it (its Get* then answers NotFoundException) while the parent project still answers, and DeleteStack removes the project. AWS reserves the refusal the emulator gives — 400 ValidationError, "Template format error: Unrecognized resource types: [<type>, ...]" — for types that are genuinely not in the registry; its fixed text is "Template format error: Unrecognized resource types: [" ... "]" and its variable part is the list of unknown type names taken from the request's TemplateBody. These six are not unknown types, so that refusal must not be reached at all for them. Smithy (cloudformation): CreateStack -> CreateStackOutput{StackId}; the model declares no error for an unknown resource type, so this refusal is the service-common ValidationError; recorded behavior only for the per-resource provisioning semantics. AWS also validates these resources' properties early: a DevicePool Rules entry with an unsupported Attribute fails early validation before any resource is created, the stack ends ROLLBACK_COMPLETE, and DescribeStackEvents/validation reporting carries a PROPERTY_VALIDATION row for the pool with FAILED/FAIL, reason "<value> is not a valid enum value. Supported values: [ARN, PLATFORM, FORM_FACTOR, MANUFACTURER, REMOTE_ACCESS_ENABLED, REMOTE_DEBUG_ENABLED, APPIUM_VERSION, INSTANCE_ARN, INSTANCE_LABELS, FLEET_TYPE, OS_VERSION, MODEL, AVAILABILITY]" and path "/Resources/<logical id>/Properties/Rules/0/Attribute" — the fixed text is the "... is not a valid enum value. Supported values: [...]" sentence and the supported-value list; the leading value and the logical id come from the request. None of that is observable while the type itself is unrecognized.

Emulator: every CreateStack carrying any of these types is refused with 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::DevicePool]" (and the analogous lists naming ::NetworkProfile, ::InstanceProfile, ::TestGridProject, ::Upload). No stack is created, so the checks never reach their Device Farm read-backs, their early-validation rejection assertions, or their delete assertions.

Why: a client deploying Device Farm through CloudFormation — directly, through the CLI, or through CDK — cannot create a stack at all: the call fails at submission with a template-format error, so the whole estate is undeployable and there is no stack to inspect, update or delete. Clients also lose the early property-validation rejection they rely on to catch a bad DevicePool rule before any resource is created.

Fix: register the AWS::DeviceFarm::* resource types in the CloudFormation service crate's resource-type registry/template validator and implement their provisioners (create/update/delete, Ref and GetAtt Arn, stack-tag propagation) against the devicefarm service state, including the pre-create property validation of DevicePool Rules Attribute.

Traces: cloudformation__devicefarm.deploy-devicefarm-devicepool-invalid-rule-attribute-rejected, cloudformation__devicefarm.deploy-devicefarm-devicepool-networkprofile-round-trip, cloudformation__devicefarm.deploy-devicefarm-project-instanceprofile-testgridproject-round-trip, cloudformation__devicefarm.deploy-devicefarm-upload-round-trip.

## Evidence
Expected: CreateStack accepts a template declaring AWS::DeviceFarm::Project/DevicePool/NetworkProfile/InstanceProfile/TestGridProject/Upload, returns a StackId, and the stack reaches CREATE_COMPLETE with each resource existing in Device Farm (Ref == GetAtt Arn == the resource ARN); a DevicePool rule with an unsupported Attribute instead ends the stack ROLLBACK_COMPLETE with a PROPERTY_VALIDATION FAILED row at /Resources/<logical>/Properties/Rules/0/Attribute.

Actual (all four checks, at the first create_stack call):
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::DevicePool]
  ... Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::NetworkProfile, AWS::DeviceFarm::DevicePool]
  ... Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::InstanceProfile, AWS::DeviceFarm::TestGridProject]
  ... Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::Upload]
The refusal's status (400), code (ValidationError) and wording are AWS's wording for a genuinely unknown type; the divergence is that these registered AWS types are treated as unknown, so no stack is ever created.

## Steps to Reproduce
1. Boot the emulator and point a boto3 cloudformation client at it.
2. Call CreateStack with TemplateBody = {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"Proj":{"Type":"AWS::DeviceFarm::Project","Properties":{"Name":"<unique>"}},"Pool":{"Type":"AWS::DeviceFarm::DevicePool","Properties":{"Name":"<unique>","ProjectArn":{"Fn::GetAtt":["Proj","Arn"]},"Rules":[{"Attribute":"PLATFORM","Operator":"EQUALS","Value":"\"ANDROID\""}]}}},"Outputs":{"ProjRef":{"Value":{"Ref":"Proj"}},"PoolRef":{"Value":{"Ref":"Pool"}}}}.
3. Observe the call is refused with 400 ValidationError "Template format error: Unrecognized resource types: [AWS::DeviceFarm::Project, AWS::DeviceFarm::DevicePool]" instead of returning a StackId.
4. Repeat substituting AWS::DeviceFarm::NetworkProfile, ::InstanceProfile, ::TestGridProject and ::Upload — each is likewise named as unrecognized.
5. Replay the four behavioral checks listed in trace_ids.
