Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-cluster-scheduler-quota-notfound
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-cluster-scheduler-quota-notfound sagemaker-hub-content-reference-lifecycle sagemaker-hub-lifecycle sagemaker-model-card-and-export-job-workflow sagemaker-model-card-lifecycle-and-export-job sagemaker-model-package-create-and-verify sagemaker-partner-app-error-paths sagemaker-scaling-configuration-recommendation-endpoint-not-found sagemaker-cluster-node-batch-ops-notfound
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeClusterSchedulerConfig|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeComputeQuota|4xx->4xx|$.Message
Issue-Key: sagemaker|DeleteHub|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeHub|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeModelCard|4xx->4xx|$.Message
Issue-Key: sagemaker|DeleteModelPackage|4xx->4xx|$.message
Issue-Key: sagemaker|CreatePartnerAppPresignedUrl|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribePartnerApp|4xx->4xx|$.Message
Issue-Key: sagemaker|GetScalingConfigurationRecommendation|4xx->4xx|$.Message
Issue-Key: sagemaker|BatchAddClusterNodes|4xx->4xx|$.Message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker scheduler, hub, card and partner-app not-found texts diverge

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeClusterSchedulerConfig

## Description
This issue bundles 10 further causes of the same class as the describe/delete not-found bundle, on the scheduler, hub, model-card, model-package, partner-app, scaling-recommendation and cluster-node surfaces: the status and error code are right but the message is emulator-authored, and the identifier is rendered in the wrong form (ARN where AWS names the resource, or quoted where AWS does not quote). Fix belongs in each operation's not-found message template, over the shared not-found error constructor.

Cause 1 of 10: DescribeClusterSchedulerConfig, unknown config. AWS: 'Cluster scheduler config with arn <cluster-scheduler-config arn> could not be found' - full ARN, unquoted, no trailing period. Emulator: 'Could not find cluster policy "<id>".' (also the wrong noun). Fix: DescribeClusterSchedulerConfig not-found message. Traces: sagemaker-cluster-scheduler-quota-notfound#task#6.

Cause 2 of 10: DescribeComputeQuota, unknown quota. AWS: 'Compute Quota with arn <compute-quota arn> could not be found'. Emulator: 'Could not find compute quota "<id>".' Fix: DescribeComputeQuota not-found message. Traces: sagemaker-cluster-scheduler-quota-notfound#task#9.

Cause 3 of 10: DeleteHub, unknown hub. AWS: 'Hub with name <HubName> does not exist.' Emulator: 'Resource not found: hub "<name>".' Fix: DeleteHub not-found message. Traces: sagemaker-hub-content-reference-lifecycle#setup#1.

Cause 4 of 10: DescribeHub, unknown hub. AWS: 'Hub with name <HubName> does not exist.' Emulator: 'Resource not found: hub "<name>".' Fix: DescribeHub not-found message. Traces: sagemaker-hub-lifecycle#setup#1 (also sagemaker-hub-lifecycle#teardown#6).

Cause 5 of 10: DescribeModelCard, unknown card. AWS: 'Modelcard <model-card arn> does not exist' - the single word 'Modelcard', the full ARN, no quotes, no trailing period. Emulator: "Could not find model card '<name>'." Fix: DescribeModelCard not-found message. Traces: sagemaker-model-card-and-export-job-workflow#teardown#5 (also sagemaker-model-card-lifecycle-and-export-job#task#8).

Cause 6 of 10: DeleteModelPackage, unknown package. AWS: 'ModelPackage <model-package arn> does not exist.' Emulator: 'Could not find model package "<name>".' Fix: DeleteModelPackage not-found message. Traces: sagemaker-model-package-create-and-verify#task#4.

Cause 7 of 10: CreatePartnerAppPresignedUrl for an unknown app. AWS: the fixed message 'App entity does not exist.' with no identifier at all. Emulator: 'Partner app <arn> does not exist.' Why: the emulator leaks an ARN AWS deliberately omits and the text no client can match. Fix: CreatePartnerAppPresignedUrl not-found message. Traces: sagemaker-partner-app-error-paths#task#2.

Cause 8 of 10: DescribePartnerApp for an unknown app. AWS: 'App entity does not exist.' Emulator: 'Partner app <arn> does not exist.' Fix: DescribePartnerApp not-found message. Traces: sagemaker-partner-app-error-paths#task#3.

Cause 9 of 10: GetScalingConfigurationRecommendation for an unknown recommendations job. AWS: 'Inference recommendations job <JobName> was not found.' Emulator: 'Inference recommendations job <JobName> does not exist.' Fix: GetScalingConfigurationRecommendation not-found message. Traces: sagemaker-scaling-configuration-recommendation-endpoint-not-found#task#1.

Cause 10 of 10: BatchAddClusterNodes against an unknown cluster. AWS: 'Cluster with name <ClusterName> not found' - unquoted name, no trailing period. Emulator: 'Cluster <name> does not exist.' Fix: BatchAddClusterNodes cluster-resolution message. Traces: sagemaker-cluster-node-batch-ops-notfound#task#2.

## Evidence
Status and error code match in all ten; only $.Message diverges. 1: expected 'Cluster scheduler config with arn <arn> could not be found', got 'Could not find cluster policy "<id>".'. 2: expected 'Compute Quota with arn <arn> could not be found', got 'Could not find compute quota "<id>".'. 3/4: expected 'Hub with name <name> does not exist.', got 'Resource not found: hub "<name>".'. 5: expected 'Modelcard <arn> does not exist', got "Could not find model card '<name>'.". 6: expected 'ModelPackage <arn> does not exist.', got 'Could not find model package "<name>".'. 7/8: expected 'App entity does not exist.', got 'Partner app <arn> does not exist.'. 9: expected 'Inference recommendations job <name> was not found.', got '... does not exist.'. 10: expected 'Cluster with name <name> not found', got 'Cluster <name> does not exist.'. In every case the fixed text is the whole message minus the identifier, and the identifier form (name vs full ARN, quoted vs bare) is itself part of the requirement.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture sagemaker-cluster-scheduler-quota-notfound step task#6. Each other cause (fixture#step in trace_ids) targets a non-existent scheduler config, quota, hub, model card, model package, partner app, recommendations job or cluster and compares $.Message with the recording.
