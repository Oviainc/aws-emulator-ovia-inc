Issue-Source: trace-sweep-divergence
Issue-Trace: api-studio-and-app-image-config
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-studio-and-app-image-config sagemaker-feature-group-lifecycle sagemaker-endpoint-model-and-config-ops-reject-invalid-input sagemaker-algorithm-lifecycle sagemaker-ai-recommendation-job-workflow
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeAppImageConfig|4xx->2xx|$.Message
Issue-Key: sagemaker|DescribeAppImageConfig|4xx->2xx|$.__type
Issue-Key: sagemaker|DescribeAppImageConfig|4xx->2xx|status
Issue-Key: sagemaker|DescribeFeatureGroup|4xx->2xx|$
Issue-Key: sagemaker|DescribeFeatureGroup|4xx->2xx|$.Message
Issue-Key: sagemaker|DescribeFeatureGroup|4xx->2xx|$.__type
Issue-Key: sagemaker|DescribeFeatureGroup|4xx->2xx|status
Issue-Key: sagemaker|DeleteEndpointConfig|4xx->2xx|$
Issue-Key: sagemaker|DeleteEndpointConfig|4xx->2xx|status
Issue-Key: sagemaker|DeleteEndpoint|4xx->2xx|$
Issue-Key: sagemaker|DeleteEndpoint|4xx->2xx|status
Issue-Key: sagemaker|DeleteAlgorithm|4xx->2xx|$
Issue-Key: sagemaker|DeleteAlgorithm|4xx->2xx|status
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.AlgorithmArn
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.AlgorithmName
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.AlgorithmStatus
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.AlgorithmStatusDetails
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.CertifyForMarketplace
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.CreationTime
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|$.TrainingSpecification
Issue-Key: sagemaker|DescribeAlgorithm|2xx->4xx|status
Issue-Key: sagemaker|CreateAlgorithm|4xx->2xx|$.__type
Issue-Key: sagemaker|CreateAlgorithm|4xx->2xx|$.message
Issue-Key: sagemaker|CreateAlgorithm|4xx->2xx|status
Issue-Key: sagemaker|DeleteAIRecommendationJob|4xx->2xx|$.__type
Issue-Key: sagemaker|DeleteAIRecommendationJob|4xx->2xx|$.message
Issue-Key: sagemaker|DeleteAIRecommendationJob|4xx->2xx|status
Issue-Key: sagemaker|StopAIRecommendationJob|2xx->4xx|$.AIRecommendationJobArn
Issue-Key: sagemaker|StopAIRecommendationJob|2xx->4xx|status
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->4xx|$
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->4xx|$.AIRecommendationJobStatus
Issue-Key: sagemaker|DescribeAIRecommendationJob|2xx->4xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker delete and stop paths lack not-found and state gates

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAppImageConfig

## Description
This issue bundles 10 causes with one root fault: SageMaker delete/stop handlers neither reject an absent target nor honour the resource's non-terminal-state gate, and a successful delete does not remove the resource from the read path. Both directions of the gate are wrong, so deleted resources stay readable while live resources are destroyed. Fix belongs in the delete/stop handlers' precondition checks and in the state store's removal step.

Cause 1 of 10: DescribeAppImageConfig after DeleteAppImageConfig. AWS: 400 with __type 'ResourceNotFound' and message 'AppImageConfig [<app image config arn>] does not exist' (fixed text, ARN from the request). Emulator: 200 still describing the deleted config. Why: a client that polls for deletion to complete never observes it. Fix: DeleteAppImageConfig state removal / DescribeAppImageConfig lookup. Traces: api-studio-and-app-image-config#teardown#12.

Cause 2 of 10: DescribeFeatureGroup after DeleteFeatureGroup. AWS: 400 ResourceNotFound "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <FeatureGroupName>" (Smithy DescribeFeatureGroup models ResourceNotFound, httpError 400). Emulator: keeps answering 200 with FeatureGroupStatus 'Created' for the full 61-attempt poll. Fix: DeleteFeatureGroup state removal. Traces: sagemaker-feature-group-lifecycle#task#7.

Cause 3 of 10: DeleteEndpointConfig for a config that does not exist. AWS: 400 ValidationException 'Could not find endpoint configuration "<EndpointConfigName>".' Emulator: 200 with an empty body. Why: delete-if-exists semantics hide a client's naming error. Fix: DeleteEndpointConfig not-found check. Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#5.

Cause 4 of 10: DeleteEndpoint for an endpoint that does not exist. AWS: 400 ValidationException 'Could not find endpoint "<EndpointName>".' (Smithy DeleteEndpoint models no errors, so this wording is recorded behavior only.) Emulator: 200 empty body. Fix: DeleteEndpoint not-found check. Traces: sagemaker-endpoint-model-and-config-ops-reject-invalid-input#task#6.

Cause 5 of 10: DeleteAlgorithm while the algorithm is still Pending. AWS: 400 ValidationException with the fixed message 'Cannot delete Algorithm in Pending status.' Emulator: 200 and the algorithm is destroyed. Fix: DeleteAlgorithm state gate. Traces: sagemaker-algorithm-lifecycle#task#2.

Cause 6 of 10: DescribeAlgorithm after that ungated delete. AWS: 200 with the algorithm reaching AlgorithmStatus 'Completed'. Emulator: 400 ValidationException 'Algorithm <arn> does not exist.' for the entire poll window. Fix: as cause 5 (the resource must survive the refused delete and settle to Completed). Traces: sagemaker-algorithm-lifecycle#task#3.

Cause 7 of 10: CreateAlgorithm re-issued for the same name. AWS: 400 ValidationException 'Algorithm already exists: <algorithm arn>' because the algorithm still exists. Emulator: 200, because its ungated delete removed it. Fix: as cause 5. Traces: sagemaker-algorithm-lifecycle#task#4.

Cause 8 of 10: DeleteAIRecommendationJob while the job is Pending. AWS: 400 ValidationException "Cannot delete job in 'Pending' state. Stop the job first." (fixed text; the state name comes from the job's current status). Emulator: 200 and the job is destroyed. Fix: DeleteAIRecommendationJob state gate. Traces: sagemaker-ai-recommendation-job-workflow#task#4.

Cause 9 of 10: StopAIRecommendationJob on that job. AWS: 200 returning AIRecommendationJobArn. Emulator: 400, the job having been destroyed by cause 8. Fix: as cause 8. Traces: sagemaker-ai-recommendation-job-workflow#teardown#5.

Cause 10 of 10: DescribeAIRecommendationJob polling for a terminal status. AWS: 200 with AIRecommendationJobStatus reaching Stopped/Failed/Completed. Emulator: 400 ResourceNotFound "Job '<AIRecommendationJobName>' not found." for 61 attempts. Fix: as cause 8. Traces: sagemaker-ai-recommendation-job-workflow#teardown#6.

## Evidence
Cause 1: expected 400 {'__type':'ResourceNotFound','Message':'AppImageConfig [<arn>] does not exist'}; actual 200 with the config body. Cause 2: expected 400 ResourceNotFound "Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name <name>"; actual 200, FeatureGroupStatus 'Created' (poll capped at 60s). Cause 3: expected 400 ValidationException 'Could not find endpoint configuration "<name>".'; actual 200 {}. Cause 4: expected 400 ValidationException 'Could not find endpoint "<name>".'; actual 200 {}. Cause 5: expected 400 ValidationException 'Cannot delete Algorithm in Pending status.'; actual 200 {}. Cause 6: expected 200 with AlgorithmStatus 'Completed'; actual 400 ValidationException 'Algorithm <arn> does not exist.'. Cause 7: expected 400 ValidationException 'Algorithm already exists: <arn>'; actual 200. Cause 8: expected 400 ValidationException "Cannot delete job in 'Pending' state. Stop the job first."; actual 200. Cause 9: expected 200 with AIRecommendationJobArn; actual 400. Cause 10: expected 200 with a terminal AIRecommendationJobStatus; actual 400 ResourceNotFound "Job '<name>' not found.".

## Steps to Reproduce
Replay the wire corpus. Representative: fixture api-studio-and-app-image-config step teardown#12. The remaining nine causes are the fixture#step bindings in trace_ids: delete/stop a resource that is absent or in a non-terminal state, then read it back.
