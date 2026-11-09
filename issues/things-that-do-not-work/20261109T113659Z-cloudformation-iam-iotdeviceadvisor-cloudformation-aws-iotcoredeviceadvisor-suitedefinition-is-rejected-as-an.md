Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__iotdeviceadvisor.deploy-iotcoredeviceadvisor-suitedefinition-empty-root-group-rejected cloudformation__iam__iotdeviceadvisor.deploy-iotcoredeviceadvisor-suitedefinition-round-trip-and-delete
Issue-Key: cloudformation__iam__iotdeviceadvisor|CreateStack|ValidationError
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# cloudformation: AWS::IoTCoreDeviceAdvisor::SuiteDefinition is rejected as an unrecognized resource type, so no Device Advisor suite can be deployed from a template

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template-validation stage does not know the resource type `AWS::IoTCoreDeviceAdvisor::SuiteDefinition`, so every CreateStack whose template declares an IoT Core Device Advisor suite definition is refused at submit time. Both Device Advisor deploy checks of this suite die on the very first call and never reach the behavior they were written to measure.

AWS: `AWS::IoTCoreDeviceAdvisor::SuiteDefinition` is a supported CloudFormation resource type. CreateStack accepts a template declaring it (no `Template format error`), provisions it through the Device Advisor control plane (Smithy, service iotdeviceadvisor/IotSenateService: `CreateSuiteDefinition` POST /suiteDefinitions, input member `suiteDefinitionConfiguration` -> shape `SuiteDefinitionConfiguration` (required), `tags` -> TagMap; errors ValidationException (400) and InternalServerException (500)), and exposes the created suite as `Ref` -> suiteDefinitionId plus `Fn::GetAtt` SuiteDefinitionArn / SuiteDefinitionId / SuiteDefinitionVersion, whose values must equal what `GetSuiteDefinition` (Smithy: GET /suiteDefinitions/{suiteDefinitionId}, output members `suiteDefinitionArn`, `suiteDefinitionVersion`, `suiteDefinitionConfiguration`) reports for the same id. A configuration AWS rejects (e.g. a `rootGroup` with no test groups) is refused by the resource's create, not by template format validation: the stack rolls back to ROLLBACK_COMPLETE, the stack-level CREATE_FAILED event carries the rollback reason, and DeleteStack of the stack removes the suite so GetSuiteDefinition then answers `ResourceNotFoundException` with message `Unable to find resource with suiteDefinitionId: <suiteDefinitionId>` (fixed text `Unable to find resource with suiteDefinitionId: ` plus the id from the request). Only the type-recognition behavior and the Smithy facts above are modeled; the per-resource GetAtt attribute names are recorded/documented behavior.

Emulator: CreateStack is refused immediately with HTTP 400, error code `ValidationError`, message `Template format error: Unrecognized resource types: [AWS::IoTCoreDeviceAdvisor::SuiteDefinition]`. Fixed text of that message is `Template format error: Unrecognized resource types: [` + the type names + `]`; the type name is taken from the submitted template, so the only request-derived part is the type list — the refusal itself is unconditional for this type. No stack is created, no stack events exist, and nothing of the Device Advisor path (Ref/GetAtt resolution, the rollback-on-invalid-rootGroup path, the delete-cascades-to-suite path) is reachable.

Why: a client deploying Device Advisor suites as infrastructure-as-code cannot deploy at all — CreateStack fails at submit, so there is no stack, no rollback, no outputs and no suite. Worse, the failure is reported as a *template format* error, which tells the client its template is malformed rather than that the emulator lacks the type, so IaC tooling (CDK/CFN deploy pipelines) treats the template as broken. Both the negative path (an invalid rootGroup must fail the resource and roll the stack back with a reason) and the positive path (suite created, Ref/GetAtt agreeing with Device Advisor's own read, delete removing the suite) are unobservable.

Fix: the CloudFormation template resource-type registry / template validation stage must recognize `AWS::IoTCoreDeviceAdvisor::SuiteDefinition`, and a provisioning handler for it must be wired to the iotdeviceadvisor control plane: create via CreateSuiteDefinition, Ref -> suiteDefinitionId, GetAtt SuiteDefinitionArn/SuiteDefinitionId/SuiteDefinitionVersion from the suite record, resource-create refusals surfaced as CREATE_FAILED with the Device Advisor reason (driving ROLLBACK_COMPLETE, with no resource record for a create refused before provisioning), and stack delete calling DeleteSuiteDefinition so GetSuiteDefinition subsequently answers ResourceNotFoundException.

Traces: cloudformation__iam__iotdeviceadvisor.deploy-iotcoredeviceadvisor-suitedefinition-empty-root-group-rejected and cloudformation__iam__iotdeviceadvisor.deploy-iotcoredeviceadvisor-suitedefinition-round-trip-and-delete — both fail on their first CreateStack with the identical refusal.

No existing open issue covers this type; the nearest one, 20261103T041241Z-ec2-ecs-elasticache-cloudformation-rejects-aws-ec2-transitgatewayroutetableassociation-and-aws-ec2.md, is the same class of defect for two unrelated AWS::EC2::* types and does not cover AWS::IoTCoreDeviceAdvisor::SuiteDefinition.

## Evidence
Expected (AWS): CreateStack with a template containing a resource of type AWS::IoTCoreDeviceAdvisor::SuiteDefinition is accepted (200, a StackId is returned) and the stack reaches CREATE_COMPLETE for a valid suiteDefinitionConfiguration, or ROLLBACK_COMPLETE with a stack-level CREATE_FAILED reason when the declared rootGroup has no test groups. After DeleteStack, iotdeviceadvisor GetSuiteDefinition(suiteDefinitionId=<id>) answers 404 ResourceNotFoundException, message "Unable to find resource with suiteDefinitionId: <suiteDefinitionId>".

Actual (emulator): CreateStack is refused — botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoTCoreDeviceAdvisor::SuiteDefinition]. Identical refusal in both checks; nothing after the first CreateStack executes, so suite id, GetAtt outputs, stack events and the delete cascade produce no observations at all.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cf = client('cloudformation'); submit CreateStack with a TemplateBody whose Resources contain one resource of type AWS::IoTCoreDeviceAdvisor::SuiteDefinition (Properties: SuiteDefinitionConfiguration with SuiteDefinitionName, DevicePermissionRoleArn and a RootGroup JSON string; Outputs using Ref and Fn::GetAtt SuiteDefinitionArn / SuiteDefinitionVersion).
3. Observe the call raises ClientError ValidationError "Template format error: Unrecognized resource types: [AWS::IoTCoreDeviceAdvisor::SuiteDefinition]" instead of returning a StackId.
4. Repeat with a RootGroup whose "tests" list is empty: AWS fails the resource and the stack ends ROLLBACK_COMPLETE with a CREATE_FAILED reason; the emulator again refuses at CreateStack with the same template-format error.
