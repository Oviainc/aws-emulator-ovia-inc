Issue-Source: trace-sweep-divergence
Issue-Trace: api-experiment-trial-component-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-experiment-trial-component-lifecycle api-lineage-entities-lifecycle api-notebook-instance-lifecycle-config api-studio-and-app-image-config
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeExperiment|2xx->2xx|$.Description
Issue-Key: sagemaker|DescribeExperiment|2xx->2xx|$.DisplayName
Issue-Key: sagemaker|DescribeTrial|2xx->2xx|$.DisplayName
Issue-Key: sagemaker|DescribeTrial|2xx->2xx|$.LastModifiedBy
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.DisplayName
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.InputArtifacts
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.Metrics
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.OutputArtifacts
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.Sources
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.Status.Message
Issue-Key: sagemaker|DescribeTrialComponent|2xx->2xx|$.Status.PrimaryStatus
Issue-Key: sagemaker|ListTrialComponents|2xx->2xx|$.TrialComponentSummaries[*]
Issue-Key: sagemaker|DescribeArtifact|2xx->2xx|$.ArtifactName
Issue-Key: sagemaker|DescribeArtifact|2xx->2xx|$.Properties.rows
Issue-Key: sagemaker|DescribeArtifact|2xx->2xx|$.Source.SourceTypes
Issue-Key: sagemaker|ListArtifacts|2xx->2xx|$.ArtifactSummaries[*]
Issue-Key: sagemaker|DescribeAction|2xx->2xx|$.Description
Issue-Key: sagemaker|DescribeAction|2xx->2xx|$.Properties
Issue-Key: sagemaker|DescribeAction|2xx->2xx|$.Status
Issue-Key: sagemaker|ListAssociations|2xx->2xx|$.AssociationSummaries[*]
Issue-Key: sagemaker|DescribeNotebookInstanceLifecycleConfig|2xx->2xx|$.OnStart[*]
Issue-Key: sagemaker|DescribeAppImageConfig|2xx->2xx|$.KernelGatewayImageConfig.FileSystemConfig.DefaultGid
Issue-Key: sagemaker|DescribeAppImageConfig|2xx->2xx|$.KernelGatewayImageConfig.FileSystemConfig.DefaultUid
Issue-Key: sagemaker|DescribeAppImageConfig|2xx->2xx|$.KernelGatewayImageConfig.KernelSpecs[*].DisplayName
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker Update operations are accepted but never mutate stored state

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExperiment

## Description
This issue bundles 10 causes with one root fault: SageMaker Update* operations answer 200 with the resource ARN but do not apply the submitted members to stored state, so every read-back after an update serves the pre-update values. Smithy models these updates as real mutations (e.g. UpdateExperiment takes DisplayName and Description and returns only ExperimentArn, with ConflictException/ResourceNotFound as its only errors), so the read-back is the only place the change is observable. Fix belongs in each Update* handler's write to the state store (and in the list projections that read it).

Cause 1 of 10: DescribeExperiment after UpdateExperiment. AWS: DisplayName and Description reflect the updated values. Emulator: both still hold the create-time values. Fix: UpdateExperiment state write. Traces: api-experiment-trial-component-lifecycle#task#4.

Cause 2 of 10: DescribeTrial after UpdateTrial. AWS: DisplayName is the updated value. Emulator: unchanged. Fix: UpdateTrial state write. Traces: api-experiment-trial-component-lifecycle#task#9.

Cause 3 of 10: DescribeTrialComponent after UpdateTrialComponent. AWS: DisplayName and Status (Status.PrimaryStatus 'Completed', Status.Message as submitted) reflect the update. Emulator: DisplayName and both Status members still carry the create-time values. Fix: UpdateTrialComponent state write. Traces: api-experiment-trial-component-lifecycle#task#14.

Cause 4 of 10: ListTrialComponents after that update. AWS: the summary carries the updated DisplayName and Status. Emulator: the stale ones. Fix: as cause 3, plus the ListTrialComponents summary projection. Traces: api-experiment-trial-component-lifecycle#task#16.

Cause 5 of 10: DescribeArtifact after UpdateArtifact. AWS: ArtifactName and Properties reflect the update (Properties entries replaced/added as submitted). Emulator: both stale. Fix: UpdateArtifact state write. Traces: api-lineage-entities-lifecycle#task#6.

Cause 6 of 10: ListArtifacts after that update. AWS: the summary carries the updated ArtifactName. Emulator: the stale name. Fix: as cause 5, plus the ListArtifacts projection. Traces: api-lineage-entities-lifecycle#task#7.

Cause 7 of 10: DescribeAction after UpdateAction. AWS: Description, Properties and Status reflect the update. Emulator: Description stale, Properties and Status absent. Fix: UpdateAction state write. Traces: api-lineage-entities-lifecycle#task#10.

Cause 8 of 10: ListAssociations after a source rename. AWS: SourceName in the association summary tracks the renamed artifact. Emulator: the pre-rename name. Fix: as cause 5, plus the association summary projection. Traces: api-lineage-entities-lifecycle#task#17.

Cause 9 of 10: DescribeNotebookInstanceLifecycleConfig after UpdateNotebookInstanceLifecycleConfig. AWS: OnStart[0].Content is the newly submitted base64 script. Emulator: the original script. Fix: UpdateNotebookInstanceLifecycleConfig state write. Traces: api-notebook-instance-lifecycle-config#task#4.

Cause 10 of 10: DescribeAppImageConfig after UpdateAppImageConfig. AWS: KernelGatewayImageConfig.FileSystemConfig.DefaultUid/DefaultGid and KernelSpecs[0].DisplayName reflect the submitted values (including an explicit 0, which must not be treated as absent). Emulator: the create-time values. Fix: UpdateAppImageConfig state write. Traces: api-studio-and-app-image-config#task#7.

## Evidence
Cause 1: $.Description expected 'updated description', got 'initial description'; $.DisplayName expected '<name>-renamed', got '<name>'. Cause 2: $.DisplayName expected '<trial>-renamed', got '<trial>'. Cause 3: $.DisplayName expected '<component>-done', got '<component>'; $.Status.PrimaryStatus expected 'Completed', got 'InProgress'; $.Status.Message expected the submitted message, got the create-time one. Cause 4: TrialComponentSummaries[0] expected the updated DisplayName/Status, got the stale pair. Cause 5: $.ArtifactName expected '<artifact>-renamed', got '<artifact>'; $.Properties.<key> expected the updated value, got the original. Cause 6: ArtifactSummaries[0].ArtifactName stale. Cause 7: $.Description expected 'updated action', got 'initial action'; $.Properties expected the submitted map, got None; $.Status expected 'Completed', got None. Cause 8: AssociationSummaries[0].SourceName stale. Cause 9: OnStart[0].Content expected the updated base64 payload, got the original. Cause 10: DefaultGid expected 0 got 100, DefaultUid expected 0 got 1000, KernelSpecs[0].DisplayName expected 'Python 3 Updated' got 'Python 3'. Smithy: UpdateExperiment input members DisplayName/Description, output ExperimentArn only.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-experiment-trial-component-lifecycle step task#4. Each other cause (fixture#step in trace_ids) issues an Update* call that answers 200 and then reads the resource back; the read serves pre-update values.
