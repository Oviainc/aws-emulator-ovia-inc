Issue-Source: trace-sweep-divergence
Issue-Trace: api-tracking-error-conditions
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-tracking-error-conditions sagemaker-ai-workload-config-lifecycle sagemaker-algorithm-duplicate-and-missing-errors sagemaker-feature-group-lifecycle sagemaker-hub-lifecycle sagemaker-image-and-image-version-lifecycle sagemaker-model-package-and-pipeline sagemaker-inference-recommendations-job-workflow sagemaker-inference-component-against-missing-endpoint-error sagemaker-inference-component-error-paths sagemaker-inference-component-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|CreateExperiment|4xx->4xx|$.message
Issue-Key: sagemaker|CreateAIWorkloadConfig|4xx->4xx|$.Message
Issue-Key: sagemaker|CreateAlgorithm|4xx->4xx|$.message
Issue-Key: sagemaker|CreateFeatureGroup|4xx->4xx|$.Message
Issue-Key: sagemaker|CreateHub|4xx->4xx|$.Message
Issue-Key: sagemaker|CreateImage|4xx->4xx|$.Message
Issue-Key: sagemaker|CreatePipeline|4xx->4xx|$.message
Issue-Key: sagemaker|CreateInferenceRecommendationsJob|4xx->4xx|$.message
Issue-Key: sagemaker|CreateInferenceComponent|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker duplicate-create and request-refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateExperiment

## Description
This issue bundles 9 causes with one root fault: when a create is refused because the name is already taken (or the request is otherwise invalid), the emulator emits its own generic conflict text instead of AWS's per-resource wording - in two cases a single catch-all string ('The ID or Name specified is already in use.') that identifies neither the resource nor the name. Fix belongs in each create handler's conflict/validation message template.

Cause 1 of 9: CreateExperiment, duplicate name. AWS: 'Experiment names must be unique within an AWS account and region. Experiment with name (<ExperimentName>) already exists.' - fixed text with the name in parentheses. Emulator: 'Experiment with name "<name>" already exists.' Fix: CreateExperiment conflict message. Traces: api-tracking-error-conditions#task#2.

Cause 2 of 9: CreateAIWorkloadConfig, duplicate name. AWS: "AIWorkloadConfig with name '<Name>' already exists for this account." Emulator: 'The ID or Name specified is already in use.' Fix: CreateAIWorkloadConfig conflict message. Traces: sagemaker-ai-workload-config-lifecycle#task#3.

Cause 3 of 9: CreateAlgorithm, duplicate name. AWS: 'Algorithm already exists: <algorithm arn>' - colon, full ARN, no quotes, no trailing period. Emulator: 'Cannot create already existing algorithm "<arn>".' Fix: CreateAlgorithm conflict message. Traces: sagemaker-algorithm-duplicate-and-missing-errors#task#2.

Cause 4 of 9: CreateFeatureGroup, duplicate name. AWS: 'Resource Already Exists: FeatureGroup with name <FeatureGroupName> already exists. Choose a different name.' Emulator: 'The ID or Name specified is already in use.' Fix: CreateFeatureGroup conflict message. Traces: sagemaker-feature-group-lifecycle#task#4.

Cause 5 of 9: CreateHub, duplicate name. AWS: 'A hub with name <HubName> already exists. Please use a different name and try again.' Emulator: 'Cannot create already existing hub "<hub arn>".' Fix: CreateHub conflict message. Traces: sagemaker-hub-lifecycle#task#4.

Cause 6 of 9: CreateImage, duplicate name. AWS: 'The Image with ImageName <ImageName> already exists. Please use a different ImageName.' Emulator: 'Cannot create already existing image "<image arn>".' Fix: CreateImage conflict message. Traces: sagemaker-image-and-image-version-lifecycle#task#3.

Cause 7 of 9: CreatePipeline with a definition holding no steps. AWS: 'Pipeline definition: At least 1 step must be provided' (fixed text, no variable part). Emulator: 'PipelineDefinition declares no steps: Steps must hold at least one step'. Fix: CreatePipeline definition-validation message. Traces: sagemaker-model-package-and-pipeline#task#2.

Cause 8 of 9: CreateInferenceRecommendationsJob with a Default JobType carrying a ResourceLimit. AWS: refuses on that rule with the fixed message 'Default JobType will not accept ResourceLimit'. Emulator: refuses for a different reason entirely - 'Could not find model "<model arn>" named by InputConfig.ModelName.' - i.e. the wrong validation runs first and the real rule is absent. Fix: CreateInferenceRecommendationsJob validation order plus the missing JobType/ResourceLimit rule. Traces: sagemaker-inference-recommendations-job-workflow#task#1.

Cause 9 of 9: CreateInferenceComponent against a missing endpoint. AWS: 'Could not find endpoint "<EndpointName>".' - the name as submitted. Emulator: expands it to the endpoint ARN inside the same template. Fix: CreateInferenceComponent reference-resolution message (use the request's EndpointName). Traces: sagemaker-inference-component-against-missing-endpoint-error#task#1 (also sagemaker-inference-component-error-paths#task#1, sagemaker-inference-component-lifecycle#task#1).

## Evidence
1: expected 'Experiment names must be unique within an AWS account and region. Experiment with name (<name>) already exists.', got 'Experiment with name "<name>" already exists.'. 2: expected "AIWorkloadConfig with name '<name>' already exists for this account.", got 'The ID or Name specified is already in use.'. 3: expected 'Algorithm already exists: <arn>', got 'Cannot create already existing algorithm "<arn>".'. 4: expected 'Resource Already Exists: FeatureGroup with name <name> already exists. Choose a different name.', got 'The ID or Name specified is already in use.'. 5: expected 'A hub with name <name> already exists. Please use a different name and try again.', got 'Cannot create already existing hub "<arn>".'. 6: expected 'The Image with ImageName <name> already exists. Please use a different ImageName.', got 'Cannot create already existing image "<arn>".'. 7: expected 'Pipeline definition: At least 1 step must be provided', got 'PipelineDefinition declares no steps: Steps must hold at least one step'. 8: expected 'Default JobType will not accept ResourceLimit', got 'Could not find model "<arn>" named by InputConfig.ModelName.'. 9: expected 'Could not find endpoint "<EndpointName>".', got the same template with the endpoint ARN substituted. All ten keep the recorded status (400) and code.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-tracking-error-conditions step task#2. Each other cause (fixture#step in trace_ids) re-creates an existing resource, or sends the invalid create described, and compares the refusal message with the recording.
