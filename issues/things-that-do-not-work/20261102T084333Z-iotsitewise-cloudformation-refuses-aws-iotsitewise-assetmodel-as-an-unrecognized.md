Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-e2e-iotsitewise-windfarm-rollup-models-lifecycle
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|CreateStack|ValidationError
Issue-Commit: 86ceea0f39fca17f15a7090d9d9227e10094079b

# CloudFormation refuses AWS::IoTSiteWise::AssetModel as an unrecognized resource type, so no SiteWise estate can be provisioned by template

> _Note: this issue was identified against the code base at commit `86ceea0f39fca17f15a7090d9d9227e10094079b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: a CloudFormation template declaring an IoT SiteWise asset model is rejected at CreateStack as a template format error, so the whole infrastructure-as-code path into this service is unavailable.
AWS: recorded behavior only (the resource type is a CloudFormation registry fact, not part of the iotsitewise Smithy model). AWS CloudFormation recognizes AWS::IoTSiteWise::AssetModel and accepts a CreateStack whose Resources declare one: the stack reaches CREATE_COMPLETE, a Ref to that logical id resolves to the created model's assetModelId, and Outputs can publish that id and the model's name. The template in the failing check declares exactly one such resource with AssetModelName (from a stack parameter) and two AssetModelProperties - a Measurement named ActivePower and a Metric ActivePowerAvg (Expression avg(ap), Variables [{Name: ap, Value: {PropertyLogicalId: ActivePower}}], Window {Tumbling: {Interval: 5m}}) - plus two Outputs taking a Ref of the model.
Emulator: CreateStack is refused with ClientError (ValidationError): 'Template format error: Unrecognized resource types: [AWS::IoTSiteWise::AssetModel]'. The check's deploy phase therefore never runs and the whole multi-phase lifecycle (deploy, derive, mutate, destroy) is blocked at phase 1 of 4.
Why: every realistic SiteWise estate is stood up by template (CDK/CFN), not by hand-written API calls; a client whose stack is refused outright cannot provision anything, and the refusal says 'template format error', which points the user at their template rather than at a missing resource-type implementation.
Fix: the CloudFormation resource-type registry and its provisioner must carry AWS::IoTSiteWise::AssetModel - mapping the template's AssetModelName/AssetModelProperties (including Metric Expression/Variables with PropertyLogicalId references and Window.Tumbling.Interval) onto CreateAssetModel, resolving Ref to the created assetModelId, and supporting update and delete of the resource. Note: this finding is grounded in a check whose id leads with iotsitewise, so it is filed under that service, but the defect itself lives in the CloudFormation crate's resource-type support, not in the iotsitewise handlers.
Traces: iotsitewise.sitewise-e2e-iotsitewise-windfarm-rollup-models-lifecycle.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator from this checkout: 54/87 passed, all 33 failures in iotsitewise.

iotsitewise.sitewise-e2e-iotsitewise-windfarm-rollup-models-lifecycle - a four-phase end-to-end check whose phase 1 deploys the inlined template deployments/sitewise-turbine-model/template.json via cloudformation.create_stack(StackName=..., TemplateBody=..., Parameters=[{TurbineModelName}]). Result: _PhaseError: [phase 1/4 deploy] ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::IoTSiteWise::AssetModel].

The template's single resource is {"TurbineModel": {"Type": "AWS::IoTSiteWise::AssetModel", "Properties": {"AssetModelName": {"Ref": "TurbineModelName"}, "AssetModelProperties": [{"LogicalId": "ActivePower", "Name": "ActivePower", "DataType": "DOUBLE", "Type": {"TypeName": "Measurement"}}, {"LogicalId": "ActivePowerAvg", "Name": "ActivePowerAvg", "DataType": "DOUBLE", "Type": {"TypeName": "Metric", "Metric": {"Expression": "avg(ap)", "Variables": [{"Name": "ap", "Value": {"PropertyLogicalId": "ActivePower"}}], "Window": {"Tumbling": {"Interval": "5m"}}}}}]}}} with Outputs TurbineModelId = Ref TurbineModel and TurbineModelName.

Expected: CreateStack accepted, the stack polled to CREATE_COMPLETE, and DescribeStacks Outputs carrying the created model's id. The stack name and the parameter value are per-run values; the requirement is that the resource type is recognized and provisioned.

## Steps to Reproduce
1. Call CloudFormation CreateStack with a template whose Resources declare one {"Type": "AWS::IoTSiteWise::AssetModel"} with AssetModelName and AssetModelProperties (a Measurement plus a Metric with Expression/Variables/Window.Tumbling.Interval), and Outputs taking a Ref of it.
2. Observe the call refused with ValidationError 'Template format error: Unrecognized resource types: [AWS::IoTSiteWise::AssetModel]' instead of being accepted and reaching CREATE_COMPLETE.
3. Re-replay iotsitewise.sitewise-e2e-iotsitewise-windfarm-rollup-models-lifecycle to confirm (its phase 1 of 4 is the deploy).
