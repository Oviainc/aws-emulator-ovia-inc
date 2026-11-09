Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sagemaker.cfn-notebook-lifecycle-config-invalid-name-rejected cloudformation__sagemaker.cfn-notebook-lifecycle-config-round-trip cloudformation__sagemaker.cfn-sagemaker-experiment-duplicate-name-rejected-leaving-the-original cloudformation__sagemaker.deploy-cloudformation-sagemaker-ai-workload-config-round-trip-and-delete cloudformation__sagemaker.deploy-cloudformation-sagemaker-experiment-and-trial-component-round-trip-and-delete cloudformation__sagemaker.deploy-cloudformation-sagemaker-lineage-entities-round-trip-and-delete cloudformation__sagemaker.deploy-sagemaker-cfn-algorithm-read-back-then-gone cloudformation__sagemaker.deploy-sagemaker-cfn-config-resources-read-back-then-gone cloudformation__sagemaker.deploy-sagemaker-cfn-model-package-in-group-read-back-then-gone cloudformation__sagemaker.deploy-sagemaker-cfn-modelcard-invalid-status-rejected cloudformation__sagemaker.deploy-sagemaker-featuregroup-and-pipeline-round-trip-and-delete cloudformation__sagemaker.deploy-sagemaker-featuregroup-eventtimefeaturename-not-in-definitions-rejected-and-rolled-back
Issue-Key: cloudformation__sagemaker|CreateStack|ValidationError
Issue-Commit: e901ea078181297e70b2ac3a5c8be54768ac76d3

# cloudformation (SageMaker stacks): CreateStack refuses 15 AWS::SageMaker::* resource types as "Unrecognized resource types", so no SageMaker-bearing template can deploy

> _Note: this issue was identified against the code base at commit `e901ea078181297e70b2ac3a5c8be54768ac76d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause, reached by 12 failing checks under this service heading.

Cause 1 of 1: the emulator's CloudFormation template-type registry does not know most AWS::SageMaker::* resource types, so CreateStack rejects the whole template at validation time instead of provisioning the stack.

AWS: recorded behavior only — resource-type support is a CloudFormation registry fact, not a Smithy fact; the SageMaker Smithy model (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) models the underlying control-plane operations (CreateNotebookInstanceLifecycleConfig, CreateExperiment, CreateTrialComponent, CreateAction, CreateArtifact, CreateContext, CreateAlgorithm, CreateHub, CreateModelCard, CreateAppImageConfig, CreateStudioLifecycleConfig, CreateCodeRepository, CreateModelPackage, CreateFeatureGroup) that these types front. In AWS, CreateStack for a template whose Resources declare these types returns 200 with a StackId, the stack walks CREATE_IN_PROGRESS -> CREATE_COMPLETE, each logical resource gets a PhysicalResourceId equal to the SageMaker name/ARN it created, DescribeStackResource/Outputs resolve, and DeleteStack removes the underlying SageMaker resources. "Template format error: Unrecognized resource types: [...]" is the message AWS reserves for a type that genuinely is not in the registry; these types are all in it.

Emulator: CreateStack is refused up front with ValidationError and message "Template format error: Unrecognized resource types: [<comma-separated type list>]" (the fixed text is "Template format error: Unrecognized resource types: [" ... "]"; the bracketed list is taken from the request's template). Observed type lists across the 12 checks: AWS::SageMaker::NotebookInstanceLifecycleConfig, AWS::SageMaker::Experiment, AWS::SageMaker::AIWorkloadConfig, [AWS::SageMaker::Experiment, AWS::SageMaker::TrialComponent], [AWS::SageMaker::Action, AWS::SageMaker::Artifact, AWS::SageMaker::Context], AWS::SageMaker::Algorithm, [AWS::SageMaker::Hub, AWS::SageMaker::ModelCard, AWS::SageMaker::AppImageConfig, AWS::SageMaker::StudioLifecycleConfig, AWS::SageMaker::CodeRepository], AWS::SageMaker::ModelPackage, AWS::SageMaker::ModelCard, AWS::SageMaker::FeatureGroup. Because the refusal happens in template validation, nothing downstream is exercised: the negative checks (invalid lifecycle-config name, duplicate experiment name, invalid model-card status, EventTimeFeatureName not in the feature definitions) can never observe CREATE_FAILED/ROLLBACK_COMPLETE either, and the round-trip/delete checks never get a stack id.

Why: a client that deploys SageMaker infrastructure as CloudFormation — the normal path for experiments, feature groups, model packages, lineage entities, notebook lifecycle configs and Studio configs — cannot create a stack at all. The failure arrives as a client-side template error, so the client concludes its own template is malformed rather than that the emulator lacks the type, and no retry or parameter change can succeed.

Fix: the CloudFormation template/resource-type registry and its per-type provisioners (the component that maps a template resource type to a service handler), plus the create/update/delete resource lifecycle that must call the SageMaker control-plane operations listed above and record each logical resource's PhysicalResourceId; the stack-status machine must then be able to report CREATE_COMPLETE and, for a resource whose underlying create is refused, CREATE_FAILED followed by ROLLBACK_COMPLETE.

Traces: all 12 check ids in trace_ids. The same "Unrecognized resource types: [AWS::SageMaker::NotebookInstanceLifecycleConfig]" refusal also appears under the s3__sagemaker heading (cfn-deploy-sagemaker-notebook-instance-s3-bucket-access-positive, cfn-deploy-sagemaker-notebook-instance-with-s3-role), but the replay reports those as OWNED by an open issue, so they are not claimed here.

## Evidence
Every one of the 12 checks fails in its CreateStack call with the same exception type and message template:

botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [<types>]

with <types> per check:
- cfn-notebook-lifecycle-config-invalid-name-rejected: [AWS::SageMaker::NotebookInstanceLifecycleConfig]
- cfn-notebook-lifecycle-config-round-trip: [AWS::SageMaker::NotebookInstanceLifecycleConfig]
- cfn-sagemaker-experiment-duplicate-name-rejected-leaving-the-original: [AWS::SageMaker::Experiment]
- deploy-cloudformation-sagemaker-ai-workload-config-round-trip-and-delete: [AWS::SageMaker::AIWorkloadConfig]
- deploy-cloudformation-sagemaker-experiment-and-trial-component-round-trip-and-delete: [AWS::SageMaker::Experiment, AWS::SageMaker::TrialComponent]
- deploy-cloudformation-sagemaker-lineage-entities-round-trip-and-delete: [AWS::SageMaker::Action, AWS::SageMaker::Artifact, AWS::SageMaker::Context]
- deploy-sagemaker-cfn-algorithm-read-back-then-gone: [AWS::SageMaker::Algorithm]
- deploy-sagemaker-cfn-config-resources-read-back-then-gone: [AWS::SageMaker::Hub, AWS::SageMaker::ModelCard, AWS::SageMaker::AppImageConfig, AWS::SageMaker::StudioLifecycleConfig, AWS::SageMaker::CodeRepository]
- deploy-sagemaker-cfn-model-package-in-group-read-back-then-gone: [AWS::SageMaker::ModelPackage]
- deploy-sagemaker-cfn-modelcard-invalid-status-rejected: [AWS::SageMaker::ModelCard]
- deploy-sagemaker-featuregroup-and-pipeline-round-trip-and-delete: [AWS::SageMaker::FeatureGroup]
- deploy-sagemaker-featuregroup-eventtimefeaturename-not-in-definitions-rejected-and-rolled-back: [AWS::SageMaker::FeatureGroup]

Expected in AWS: HTTP 200 from CreateStack carrying a StackId, then DescribeStacks reporting CREATE_COMPLETE (or CREATE_FAILED/ROLLBACK_COMPLETE for the four negative templates). Actual: HTTP 400 ValidationError at CreateStack, no stack created. The stack names, role ARNs and generated resource names in the recordings are per-run values; the requirement is that these resource types are accepted and provisioned.

## Steps to Reproduce
1. Build and boot the emulator, then run the behavioral suite and re-run the 12 checks in trace_ids.
2. Manually: call CreateStack with a minimal template whose Resources contain a single AWS::SageMaker::Experiment (Properties: ExperimentName=<name>), e.g.
   {"Resources":{"Experiment":{"Type":"AWS::SageMaker::Experiment","Properties":{"ExperimentName":"demo"}}}}
   AWS returns a StackId and the stack reaches CREATE_COMPLETE with DescribeExperiment(ExperimentName=demo) succeeding; the emulator returns 400 ValidationError "Template format error: Unrecognized resource types: [AWS::SageMaker::Experiment]".
3. Repeat for each type listed in the evidence (NotebookInstanceLifecycleConfig, AIWorkloadConfig, TrialComponent, Action, Artifact, Context, Algorithm, Hub, ModelCard, AppImageConfig, StudioLifecycleConfig, CodeRepository, ModelPackage, FeatureGroup): each is refused the same way.
4. For the negative templates (invalid NotebookInstanceLifecycleConfig name, duplicate ExperimentName, invalid ModelCard status, FeatureGroup EventTimeFeatureName absent from FeatureDefinitions), confirm that once the types are recognized the stack reaches CREATE_FAILED and then ROLLBACK_COMPLETE rather than being refused at template validation.
