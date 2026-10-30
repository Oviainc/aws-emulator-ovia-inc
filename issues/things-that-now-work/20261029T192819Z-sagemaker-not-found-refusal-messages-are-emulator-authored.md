Issue-Source: trace-sweep-divergence
Issue-Trace: api-experiment-trial-component-lifecycle
Issue-Step: teardown#20
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-experiment-trial-component-lifecycle api-lineage-entities-lifecycle api-model-package-group-lifecycle sagemaker-ai-benchmark-recommendation-errors sagemaker-ai-workload-config-lifecycle sagemaker-cluster-scheduler-quota-notfound
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeTrialComponent|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeArtifact|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeContext|4xx->4xx|$.Message
Issue-Key: sagemaker|GetModelPackageGroupPolicy|4xx->4xx|$.message
Issue-Key: sagemaker|CreateAIBenchmarkJob|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeAIBenchmarkJob|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeAIWorkloadConfig|4xx->4xx|$.Message
Issue-Key: sagemaker|DeleteAIWorkloadConfig|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeCluster|4xx->4xx|$.Message
Issue-Related: 20261029T192819Z-sagemaker-create-operations-skip-the-execution-role-assumability.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker not-found refusal messages are emulator-authored prose

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrialComponent

## Description
This issue bundles 10 causes with one root fault: the emulator answers the correct status (400) and error code but replaces AWS's fixed not-found wording with a generic emulator-authored template ('Could not find <thing> "<id>".'), and in several places substitutes the resource ARN for the name the caller passed. The message is a requirement: a client matching on it (or showing it to a user) sees text AWS never emits. Fix belongs in the shared not-found error constructor plus the per-operation message templates; each lookup must render AWS's text with the identifier form AWS uses for that operation. Open issue '20261029T165422Z-sagemaker-describetrialcomponent-s-resourcenotfound-message-uses-the-emulator' describes cause 1 but carries none of these replay keys.

Cause 1 of 10: DescribeTrialComponent, unknown component. AWS: "Trial component '<TrialComponentName as passed>' does not exist." Emulator: 'Could not find trial component "<id>".' Fix: DescribeTrialComponent not-found message. Traces: api-experiment-trial-component-lifecycle#teardown#20.

Cause 2 of 10: DescribeArtifact, unknown artifact. AWS: "Artifact '<ArtifactArn>' does not exist." Emulator: 'Could not find artifact "<arn>".' Fix: DescribeArtifact not-found message. Traces: api-lineage-entities-lifecycle#teardown#21.

Cause 3 of 10: DescribeContext, unknown context. AWS: "Context '<ContextName>' does not exist." - the identifier is single-quoted. Emulator: 'Context <id> does not exist.' with no quoting. Fix: DescribeContext not-found message. Traces: api-lineage-entities-lifecycle#teardown#25.

Cause 4 of 10: GetModelPackageGroupPolicy with no policy attached. AWS: 'Cannot find resource policy for: <model package group arn>' - bare ARN, no surrounding quotes and no trailing period. Emulator: 'Cannot find resource policy for: "<arn>".' Fix: GetModelPackageGroupPolicy message rendering. Traces: api-model-package-group-lifecycle#task#5.

Cause 5 of 10: CreateAIBenchmarkJob naming an unknown AI workload config. AWS: "AIWorkloadConfig '<name as passed>' not found for this account." Emulator: 'Could not find AI workload configuration "<expanded arn>".' Fix: CreateAIBenchmarkJob reference-resolution message. Traces: sagemaker-ai-benchmark-recommendation-errors#task#1.

Cause 6 of 10: DescribeAIBenchmarkJob, unknown job. AWS: "AIBenchmarkJob '<name>' not found." Emulator: 'Could not find AI benchmark job "<name>".' Fix: DescribeAIBenchmarkJob not-found message. Traces: sagemaker-ai-benchmark-recommendation-errors#task#2.

Cause 7 of 10: DescribeAIWorkloadConfig, unknown config. AWS: "AIWorkloadConfig '<name>' not found." Emulator: 'Could not find AI workload config "<expanded arn>".' Fix: DescribeAIWorkloadConfig not-found message. Traces: sagemaker-ai-workload-config-lifecycle#task#4 (also #task#7).

Cause 8 of 10: DeleteAIWorkloadConfig, unknown config. AWS: "AIWorkloadConfig '<name>' not found." (the recording additionally carries the SDK's '(Service: ..., Request ID: ...)' envelope, whose request id is per-run and is not part of the requirement). Emulator: 'Could not find AI workload config "<expanded arn>".' Fix: DeleteAIWorkloadConfig not-found message. Traces: sagemaker-ai-workload-config-lifecycle#task#5 (also #task#8).

Cause 9 of 10: DescribeCluster, unknown cluster. AWS: 'Cluster with name <ClusterName> not found' - no quoting, no trailing period. Emulator: 'Could not find cluster <name>.' Fix: DescribeCluster not-found message. Traces: sagemaker-cluster-scheduler-quota-notfound#task#3.

Cause 10 of 10: DeleteCluster, unknown cluster. AWS: 'Cluster with name <ClusterName> not found'. Emulator: 'Could not find cluster <name>.' Fix: DeleteCluster not-found message. Traces: sagemaker-cluster-scheduler-quota-notfound#task#4.

## Evidence
All ten answer the right status/code and diverge only in $.Message/$.message. Cause 1: expected "Trial component '<arn>' does not exist.", got 'Could not find trial component "<arn>".'. Cause 2: expected "Artifact '<arn>' does not exist.", got 'Could not find artifact "<arn>".'. Cause 3: expected "Context '<id>' does not exist.", got 'Context <id> does not exist.'. Cause 4: expected 'Cannot find resource policy for: <arn>', got 'Cannot find resource policy for: "<arn>".'. Cause 5: expected "AIWorkloadConfig '<name>' not found for this account.", got 'Could not find AI workload configuration "<arn>".'. Cause 6: expected "AIBenchmarkJob '<name>' not found.", got 'Could not find AI benchmark job "<name>".'. Causes 7/8: expected "AIWorkloadConfig '<name>' not found.", got 'Could not find AI workload config "<arn>".'. Causes 9/10: expected 'Cluster with name <name> not found', got 'Could not find cluster <name>.'. Fixed text is everything outside the quoted identifier; the identifier is the value from the request (name for causes 1,5,6,7,8,9,10; ARN for causes 2,4).

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-experiment-trial-component-lifecycle step teardown#20. Each other cause (fixture#step in trace_ids) reads or deletes a resource that does not exist and compares $.Message against the recorded text.
