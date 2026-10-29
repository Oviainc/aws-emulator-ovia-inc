Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-cluster-node-batch-ops-notfound
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-cluster-node-batch-ops-notfound sagemaker-cluster-scheduler-config-and-compute-quota-error-paths sagemaker-optimization-job-create sagemaker-mlflow-tracking-server-lifecycle
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|AttachClusterNodeVolume|4xx->4xx|$.message
Issue-Key: sagemaker|BatchDeleteClusterNodes|4xx->4xx|$.message
Issue-Key: sagemaker|BatchRebootClusterNodes|4xx->4xx|$.message
Issue-Key: sagemaker|BatchReplaceClusterNodes|4xx->4xx|$.message
Issue-Key: sagemaker|CreateComputeQuota|4xx->4xx|$.message
Issue-Key: sagemaker|CreateOptimizationJob|4xx->4xx|$.message
Issue-Key: sagemaker|CreateMlflowTrackingServer|4xx->4xx|$.message
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker ValidationException rendering diverges: ordering, list constraints, enum sets

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachClusterNodeVolume

## Description
This issue bundles 7 causes with one root fault: the shared constraint-violation renderer that builds the '<n> validation error(s) detected: ...' message does not reproduce AWS's rendering. Three things are wrong: the order in which multiple member violations are listed, the form used when the violating member is a list (AWS reports the whole list value once against the list member with the member-constraint set in brackets, not one entry per index), and the enum value set (ordering, and in one case missing values). Fix belongs in that validation-message renderer and in the enum catalogs it reads. The open issue '20261029T070612Z-sagemaker-attachclusternodevolume-s-cluster-not-found-message-is' concerns a different AttachClusterNodeVolume message and does not cover these.

Cause 1 of 7: AttachClusterNodeVolume with three invalid members. AWS: the three violations are listed clusterArn, volumeId, nodeId - i.e. in AWS's own member order, not the input's. Emulator: clusterArn, nodeId, volumeId. Everything else in the message matches. Fix: validation-renderer ordering. Traces: sagemaker-cluster-node-batch-ops-notfound#task#1.

Cause 2 of 7: BatchDeleteClusterNodes with an invalid node id. AWS: "1 validation error detected: Value '[<nodeId>]' at 'nodeIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: i-[a-f0-9]{8}(?:[a-f0-9]{9})?, Member must not be null]" - the value is the bracketed list, the path is the list member, and the constraint is the bracketed member-constraint set. Emulator: "Value '<nodeId>' at 'nodeIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." - per-index path and only the pattern. Fix: validation-renderer list-constraint form. Traces: sagemaker-cluster-node-batch-ops-notfound#task#3.

Cause 3 of 7: BatchRebootClusterNodes, same list-constraint form. Same AWS vs emulator difference as cause 2. Fix: as cause 2. Traces: sagemaker-cluster-node-batch-ops-notfound#task#4.

Cause 4 of 7: BatchReplaceClusterNodes, same list-constraint form. Fix: as cause 2. Traces: sagemaker-cluster-node-batch-ops-notfound#task#5.

Cause 5 of 7: CreateComputeQuota with a bad clusterArn and a bad resource-sharing strategy. AWS: the enum value set is rendered '[DontLend, Lend, LendAndBorrow]'. Emulator: '[Lend, DontLend, LendAndBorrow]', which is the Smithy member order (Smithy ResourceSharingStrategy values: Lend, DontLend, LendAndBorrow) - so the renderer must use AWS's recorded order, not the model's. Fix: validation-renderer enum-set ordering. Traces: sagemaker-cluster-scheduler-config-and-compute-quota-error-paths#task#3.

Cause 6 of 7: CreateOptimizationJob with an unsupported deploymentInstanceType. AWS: the enum value set is rendered in its own (non-model) order. Emulator: renders the model's declaration order. Fix: as cause 5. Traces: sagemaker-optimization-job-create#task#1.

Cause 7 of 7: CreateMlflowTrackingServer with an invalid trackingServerSize. AWS: the enum value set is '[mm1.xlarge, Small, mm1.small, mm1.medium, Medium, mm1.2xlarge, Large, mm1.large]'. Emulator: '[Small, Medium, Large]' - the on-disk Smithy TrackingServerSize enum carries only Small/Medium/Large, so the mm1.* sizes are recorded behavior only and the emulator's accepted-size catalog is incomplete as well as mis-ordered. Fix: the TrackingServerSize value catalog behind CreateMlflowTrackingServer plus the enum-set rendering. Traces: sagemaker-mlflow-tracking-server-lifecycle#task#1.

## Evidence
All seven answer 400 with the right code and differ only in $.message. Cause 1: expected order clusterArn; volumeId; nodeId, got clusterArn; nodeId; volumeId (identical per-member text). Causes 2-4: expected "Value '[<id>]' at 'nodeIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 256, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: i-[a-f0-9]{8}(?:[a-f0-9]{9})?, Member must not be null]", got "Value '<id>' at 'nodeIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: i-[a-f0-9]{8}(?:[a-f0-9]{9})?". Cause 5: expected enum set '[DontLend, Lend, LendAndBorrow]', got '[Lend, DontLend, LendAndBorrow]'. Cause 6: expected the deploymentInstanceType set in AWS's recorded order, got the model-declaration order. Cause 7: expected '[mm1.xlarge, Small, mm1.small, mm1.medium, Medium, mm1.2xlarge, Large, mm1.large]', got '[Small, Medium, Large]'. Smithy: ResourceSharingStrategy = [Lend, DontLend, LendAndBorrow]; TrackingServerSize = [Small, Medium, Large].

## Steps to Reproduce
Replay the wire corpus. Representative: fixture sagemaker-cluster-node-batch-ops-notfound step task#1. Each other cause (fixture#step in trace_ids) sends a request violating a pattern, a list-member constraint or an enum constraint and compares the rendered '<n> validation error(s) detected: ...' message with the recording.
