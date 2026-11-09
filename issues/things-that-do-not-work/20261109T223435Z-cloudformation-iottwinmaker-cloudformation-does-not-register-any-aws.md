Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iottwinmaker.deploy-iottwinmaker-workspace-componenttype-entity-scene-round-trip-and-delete cloudformation__iottwinmaker.deploy-iottwinmaker-sync-job-round-trip-and-delete cloudformation__iottwinmaker.deploy-iottwinmaker-componenttype-not-stored-externally-rejected
Issue-Key: cloudformation__iottwinmaker|CreateStack|ValidationError
Issue-Commit: e85d2009e41cfda8a7867f84cab4a050d23db70a

# cloudformation__iottwinmaker: CloudFormation does not register any AWS::IoTTwinMaker::* resource type

> _Note: this issue was identified against the code base at commit `e85d2009e41cfda8a7867f84cab4a050d23db70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: AWS::IoTTwinMaker::Workspace, ComponentType, Entity, Scene and SyncJob are not recognized resource types.
AWS: all five are public CloudFormation resource types, so CreateStack accepts templates declaring them and provisions the workspace and its children.
Emulator: CreateStack answers HTTP 400 `ValidationError` with `Template format error: Unrecognized resource types: [<the bracketed list of the declared AWS::IoTTwinMaker::* types>]`. The fixed text is "Template format error: Unrecognized resource types: "; the list is taken from the request.
Why: no IoT TwinMaker estate can be deployed through CloudFormation, so the whole service is unreachable from the IaC path, including the negative case where a ComponentType that is not externally stored must be refused.
Fix: register the AWS::IoTTwinMaker::* types in the CloudFormation resource-type registry and add their resource handlers.
Traces: the three failing checks below.

## Evidence
botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoTTwinMaker::Workspace, AWS::IoTTwinMaker::ComponentType, AWS::IoTTwinMaker::Entity, AWS::IoTTwinMaker::Scene] (and the narrower lists for the SyncJob and ComponentType checks). Expected CreateStack accepted with a StackId.

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite.
2. CreateStack with a template declaring AWS::IoTTwinMaker::Workspace plus any of ComponentType / Entity / Scene / SyncJob.
3. Observe the ValidationError.
