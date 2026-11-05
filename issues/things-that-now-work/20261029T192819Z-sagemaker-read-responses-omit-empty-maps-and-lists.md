Issue-Source: trace-sweep-divergence
Issue-Trace: api-experiment-trial-component-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-experiment-trial-component-lifecycle sagemaker-trial-component-metric-numeric-precision-preserved api-lineage-entities-lifecycle sagemaker-action-lifecycle-create-describe-delete sagemaker-action-lifecycle-transitions sagemaker-ai-workload-benchmark-and-recommendation sagemaker-ai-workload-config-lifecycle sagemaker-artifact-source-uri-binary-safe-round-trip sagemaker-model-card-create-export-verify sagemaker-model-card-lifecycle-and-export-job sagemaker-model-card-unicode-content-round-trip sagemaker-endpoint-config-variant-weight-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeContext|2xx->2xx|$.Properties
Issue-Key: sagemaker|DescribeAIWorkloadConfig|2xx->2xx|$.Tags
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->2xx|$.FailureReason
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->2xx|$.Recommendations
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->2xx|$.Tags
Issue-Key: sagemaker|DescribeArtifact|2xx->2xx|$.Properties
Issue-Key: sagemaker|DescribeModelCard|2xx->2xx|$.LastModifiedBy
Issue-Key: sagemaker|DescribeEndpointConfig|2xx->2xx|$.EnableNetworkIsolation
Issue-Key: sagemaker|DescribeEndpointConfig|2xx->2xx|$.MetricsConfig
Issue-Related: 20261029T192819Z-sagemaker-update-operations-are-accepted-but-never-mutate.md
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker read responses omit empty maps and lists entirely

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrial

## Description
This issue bundles 10 causes with one root fault: the serializer drops members whose value is an empty map, an empty list or an empty structure instead of emitting them, so a collection AWS always serializes comes back absent. Smithy models these as plain (non-required, non-sparse) maps/lists/structures — e.g. DescribeTrialComponent output has InputArtifacts/OutputArtifacts (TrialComponentArtifacts maps, length 0-60), Metrics and Sources (lists), CreatedBy/LastModifiedBy (UserContext structures) — and the recordings show AWS emitting them as {} / [] rather than omitting them. Fix belongs in the response serializer / each Describe handler's output construction.

Cause 1 of 10: DescribeTrial omits LastModifiedBy. AWS: LastModifiedBy present as an (empty) UserContext structure. Emulator: member absent. Why: SDK clients that read response.last_modified_by().iam_identity() branch differently on absent vs present-but-empty. Fix: DescribeTrial output construction. Traces: api-experiment-trial-component-lifecycle#task#7.

Cause 2 of 10: DescribeTrialComponent omits InputArtifacts, OutputArtifacts, Metrics and Sources. AWS: {} for the two artifact maps, [] for Metrics and Sources. Emulator: all four absent. Fix: DescribeTrialComponent output construction. Traces: api-experiment-trial-component-lifecycle#task#12 (also sagemaker-trial-component-metric-numeric-precision-preserved#task#2).

Cause 3 of 10: DescribeArtifact omits Source.SourceTypes. AWS: Source.SourceTypes serialized as []. Emulator: absent. Fix: DescribeArtifact output construction. Traces: api-lineage-entities-lifecycle#task#4.

Cause 4 of 10: DescribeAction omits Properties. AWS: Properties serialized as {}. Emulator: absent. Fix: DescribeAction output construction. Traces: api-lineage-entities-lifecycle#task#8 (also sagemaker-action-lifecycle-create-describe-delete#task#2, sagemaker-action-lifecycle-transitions#task#2).

Cause 5 of 10: DescribeContext omits Properties. AWS: {}. Emulator: absent. Fix: DescribeContext output construction. Traces: api-lineage-entities-lifecycle#task#12.

Cause 6 of 10: DescribeAIWorkloadConfig omits Tags. AWS: Tags serialized as []. Emulator: absent. Fix: DescribeAIWorkloadConfig output construction. Traces: sagemaker-ai-workload-benchmark-and-recommendation#task#3 (also sagemaker-ai-workload-config-lifecycle#task#2).

Cause 7 of 10: DescribeAIRecommendationJob omits Recommendations, Tags and FailureReason. AWS: Recommendations [] and Tags [] always present, and FailureReason present for a user-stopped job with the fixed text 'Job stopped by user request'. Emulator: all three absent. Fix: DescribeAIRecommendationJob output construction (and the stop path that sets FailureReason). Traces: sagemaker-ai-workload-benchmark-and-recommendation#task#7.

Cause 8 of 10: DescribeArtifact (binary-safe SourceUri fixture) omits Properties. AWS: {}. Emulator: absent. Fix: DescribeArtifact output construction. Traces: sagemaker-artifact-source-uri-binary-safe-round-trip#task#2.

Cause 9 of 10: DescribeModelCard omits LastModifiedBy. AWS: present as an empty UserContext. Emulator: absent. Fix: DescribeModelCard output construction. Traces: sagemaker-model-card-create-export-verify#task#3 (also sagemaker-model-card-lifecycle-and-export-job#task#3, sagemaker-model-card-unicode-content-round-trip#task#2).

Cause 10 of 10: DescribeEndpointConfig omits MetricsConfig and the EnableNetworkIsolation default. AWS: MetricsConfig is echoed back ({'EnableDetailedObservability': true} as submitted) and EnableNetworkIsolation is serialized as false. Emulator: both absent. Fix: DescribeEndpointConfig output construction / CreateEndpointConfig echo storage. Traces: sagemaker-endpoint-config-variant-weight-numeric-precision-preserved#task#4.

## Evidence
Cause 1: $.LastModifiedBy expected {}, got None. Cause 2: $.InputArtifacts expected {} got None; $.OutputArtifacts expected {} got None; $.Metrics expected [] got None; $.Sources expected [] got None. Cause 3: $.Source.SourceTypes expected [] got None. Cause 4/5/8: $.Properties expected {} got None. Cause 6: $.Tags expected [] got None. Cause 7: $.Recommendations expected [] got None; $.Tags expected [] got None; $.FailureReason expected 'Job stopped by user request' got None. Cause 9: $.LastModifiedBy expected {} got None. Cause 10: $.MetricsConfig expected {'EnableDetailedObservability': True} got None; $.EnableNetworkIsolation expected False got None. Smithy: DescribeTrialComponent output InputArtifacts/OutputArtifacts -> TrialComponentArtifacts (map, length 0..60), Metrics -> TrialComponentMetricSummaries (list), Sources -> TrialComponentSources (list), CreatedBy/LastModifiedBy -> UserContext.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-experiment-trial-component-lifecycle step task#7. For each other cause (fixture#step in trace_ids) create the resource with no entries in the collection and read it back; the emulator omits the member where the recording has {} or [].
