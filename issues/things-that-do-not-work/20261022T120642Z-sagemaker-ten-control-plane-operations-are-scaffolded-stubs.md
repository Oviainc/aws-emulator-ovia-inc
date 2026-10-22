Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected sagemaker.cluster-node-batch-add-delete-lifecycle sagemaker.cluster-node-batch-lifecycle sagemaker.create-compute-quota-duplicate-name-conflict sagemaker.parallel-create-cluster-scheduler-config-single-winner sagemaker.create-trial-missing-experiment-rejected sagemaker.create-trial-nonexistent-experiment-rejected sagemaker.trial-create-missing-experiment-rejected sagemaker.trial-creation-missing-experiment-rejected sagemaker.create-model-card-export-job-missing-card sagemaker.create-model-card-export-job-missing-card-not-found sagemaker.create-model-card-export-job-missing-model-card-not-found-rejected sagemaker.create-model-card-export-job-not-found sagemaker.model-card-archived-update-rejected sagemaker.create-partner-app-presigned-url-app-not-found sagemaker.create-partner-app-presigned-url-missing-app sagemaker.get-scaling-configuration-recommendation-endpoint-not-found sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint sagemaker.get-scaling-configuration-recommendation-matches-endpoint sagemaker.get-scaling-configuration-recommendation-not-found sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found sagemaker.scaling-configuration-recommendation-for-live-endpoint sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint sagemaker.put-model-package-group-policy-precondition-conflict
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: sagemaker.cluster-node-batch-add-delete-lifecycle|run#4|AssertionError|assert add_code == "ResourceNotFound", f"BatchAddClusterNodes: expected ResourceNotFound, got {add_code!r}"
Issue-Key: sagemaker.cluster-node-batch-lifecycle|run#5|AssertionError|assert add_resp["Code"] == "ResourceNotFound", f"BatchAddClusterNodes: expected ResourceNotFound, got {add_resp['Code']}"
Issue-Key: sagemaker.create-compute-quota-duplicate-name-conflict|run#9|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: sagemaker.parallel-create-cluster-scheduler-config-single-winner|run#11|AssertionError|assert codes == ["ValidationException", "ValidationException"], (
Issue-Key: sagemaker.create-trial-missing-experiment-rejected|run#5|AssertionError|assert code == "ResourceNotFound", (
Issue-Key: sagemaker.create-trial-nonexistent-experiment-rejected|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.trial-create-missing-experiment-rejected|run#7|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.trial-creation-missing-experiment-rejected|run#6|AssertionError|assert code == "ResourceNotFound", (
Issue-Key: sagemaker.create-model-card-export-job-missing-card|run#6|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.create-model-card-export-job-missing-card-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.create-model-card-export-job-missing-model-card-not-found-rejected|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.create-model-card-export-job-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.model-card-archived-update-rejected|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound updating a nonexistent model card, got {code!r}"
Issue-Key: sagemaker.create-partner-app-presigned-url-app-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound for a non-existent partner app ARN, got {code!r}"
Issue-Key: sagemaker.create-partner-app-presigned-url-missing-app|run#7|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint|run#5|AssertionError|assert code == "ResourceNotFound", (
Issue-Key: sagemaker.get-scaling-configuration-recommendation-matches-endpoint|run#4|AssertionError|assert code == "ValidationException", (
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code}"
Issue-Key: sagemaker.scaling-configuration-recommendation-for-live-endpoint|run#4|AssertionError|assert shape_code == "ValidationException", (
Issue-Key: sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint|run#5|AssertionError|assert code == "ResourceNotFound", f"expected ResourceNotFound, got {code!r}"
Issue-Key: sagemaker.put-model-package-group-policy-precondition-conflict|run#7|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# SageMaker: ten control-plane operations are scaffolded stubs that answer NotImplemented instead of the modeled behavior

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachClusterNodeVolume (bundle also covers BatchAddClusterNodes, CreateComputeQuota, CreateClusterSchedulerConfig, CreateTrial, CreateModelCardExportJob, UpdateModelCard, CreatePartnerAppPresignedUrl, GetScalingConfigurationRecommendation, PutModelPackageGroupPolicy)

## Description
The SageMaker emulator answers ten distinct control-plane operations with the scaffold error code `NotImplemented` ("operation <X> is scaffolded but not implemented in this emulator") instead of the behavior the Smithy model binds to them. Each operation below is its own root cause and its own work item; this issue bundles TEN causes and all ten must be closed before submitting. None of these operations is named by the stub issues the replay report filed for me (those name CreateAction, CreateContext, CreateArtifact, CreateExperiment, CreateAIWorkloadConfig, CreateAlgorithm, CreateTrialComponent, CreateCluster, ListClusters, CreateCodeRepository, ListEndpoints, CreateAppImageConfig, ListClusterSchedulerConfigs, ListComputeQuotas, DeleteEndpointConfig, CreateHub, ListHubContents, CreateImageVersion, CreateModelCard, CreateModelPackage, CreateNotebookInstance, CreatePipeline, ListTrainingJobs, DeleteApp, GetModelPackageGroupPolicy, DescribeCodeRepository, ListLineageGroups, DescribeLineageGroup, GetSagemakerServicecatalogPortfolioStatus, CreateInferenceComponent, CreateNotebookInstanceLifecycleConfig, ListPartnerApps, CreateMlflowApp, CreateMlflowTrackingServer, CreateTrainingJob, CreateFeatureGroup, CreateModelPackageGroup, CreateHyperParameterTuningJob, CreateProcessingJob, CreateTransformJob, CreateAutoMLJobV2, DescribeNotebookInstance).

Cause 1 of 10: AttachClusterNodeVolume.
AWS: Smithy: AttachClusterNodeVolume input requires ClusterArn (@pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:cluster/[a-z0-9]{12}$), NodeId (@pattern ^i-[a-f0-9]{8}(?:[a-f0-9]{9})?$) and VolumeId (@pattern ^vol-...$); modeled error: ResourceNotFound (HTTP 400). The recording shows a request whose NodeId/VolumeId do not exist is rejected 400 ValidationException by the precondition check that runs before the attach.
Emulator: returns the scaffold error code NotImplemented for every AttachClusterNodeVolume call.
Why: a client cannot attach or probe a HyperPod node volume at all, and error-handling code that branches on ValidationException/ResourceNotFound sees an unmodeled code it will treat as a retryable server fault.
Fix: the operation's handler services/service-com-amazonaws-sagemaker-sagemaker/src/ops/attach_cluster_node_volume.rs plus its precondition validation and cluster/node state.
Traces: sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected.

Cause 2 of 10: BatchAddClusterNodes.
AWS: Smithy: BatchAddClusterNodes requires ClusterName (name-or-ARN) and NodesToAdd (@length 1..5); modeled errors ResourceNotFound and ResourceLimitExceeded, both HTTP 400. A ClusterName that does not exist is rejected 400 ResourceNotFound.
Emulator: NotImplemented for every call.
Why: HyperPod node scale-out is unavailable and the not-found contract is unobservable.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/batch_add_cluster_nodes.rs and cluster state lookup.
Traces: sagemaker.cluster-node-batch-add-delete-lifecycle, sagemaker.cluster-node-batch-lifecycle.

Cause 3 of 10: CreateComputeQuota.
AWS: Smithy: CreateComputeQuota requires Name, ClusterArn (@pattern ^arn:...:cluster/[a-z0-9]{12}$), ComputeQuotaConfig, ComputeQuotaTarget; modeled errors ConflictException and ResourceLimitExceeded. Recorded: a well-formed but nonexistent ClusterArn is rejected 400 ValidationException (cluster existence is validated before any name-conflict arbitration), message example "Specified cluster not found".
Emulator: NotImplemented.
Why: the operation is unusable and the ordering guarantee (cluster-existence before conflict) cannot be relied on.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_compute_quota.rs plus its ClusterArn existence validation.
Traces: sagemaker.create-compute-quota-duplicate-name-conflict.

Cause 4 of 10: CreateClusterSchedulerConfig.
AWS: Smithy: CreateClusterSchedulerConfig requires Name, ClusterArn, SchedulerConfig; modeled errors ConflictException, ResourceLimitExceeded. Recorded: a nonexistent ClusterArn is rejected 400 ValidationException on every concurrent racer, never ConflictException and never a success.
Emulator: NotImplemented on every racer.
Why: scheduler configs cannot be created, and a client racing two creates sees an unmodeled code instead of the deterministic validation rejection.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_cluster_scheduler_config.rs and its cluster-existence precondition.
Traces: sagemaker.parallel-create-cluster-scheduler-config-single-winner.

Cause 5 of 10: CreateTrial.
AWS: Smithy: CreateTrial requires TrialName and ExperimentName; modeled errors ResourceNotFound and ResourceLimitExceeded (HTTP 400). A CreateTrial naming an experiment that does not exist is rejected 400 ResourceNotFound.
Emulator: NotImplemented.
Why: the experiments/trials lineage surface cannot be built, and the documented ResourceNotFound branch never fires.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_trial.rs plus experiment lookup in state.
Traces: sagemaker.create-trial-missing-experiment-rejected, sagemaker.create-trial-nonexistent-experiment-rejected, sagemaker.trial-create-missing-experiment-rejected, sagemaker.trial-creation-missing-experiment-rejected.

Cause 6 of 10: CreateModelCardExportJob.
AWS: Smithy: CreateModelCardExportJob requires ModelCardName, ModelCardExportJobName, OutputConfig; modeled errors ResourceNotFound, ConflictException, ResourceLimitExceeded (HTTP 400). A ModelCardName that does not exist is rejected 400 ResourceNotFound; recorded message example "Modelcard arn:aws:sagemaker:<region>:<account>:model-card/<name> does not exist".
Emulator: NotImplemented.
Why: export jobs cannot be created and the not-found contract is unobservable.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_model_card_export_job.rs plus model-card lookup.
Traces: sagemaker.create-model-card-export-job-missing-card, sagemaker.create-model-card-export-job-missing-card-not-found, sagemaker.create-model-card-export-job-missing-model-card-not-found-rejected, sagemaker.create-model-card-export-job-not-found.

Cause 7 of 10: UpdateModelCard.
AWS: Smithy: UpdateModelCard requires ModelCardName, optional Content/ModelCardStatus; modeled errors ResourceNotFound, ConflictException, ResourceLimitExceeded (HTTP 400). An update of a card that was never created is rejected 400 ResourceNotFound (recorded message example "Modelcard <arn> does not exist"), and no card is created as a side effect.
Emulator: NotImplemented.
Why: the model-card lifecycle (Draft->PendingReview->Approved->Archived) cannot be driven, and the not-found branch never fires.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/update_model_card.rs plus model-card state transitions.
Traces: sagemaker.model-card-archived-update-rejected.

Cause 8 of 10: CreatePartnerAppPresignedUrl.
AWS: Smithy: CreatePartnerAppPresignedUrl requires Arn (@pattern ^arn:...:partner-app/app-[A-Z0-9]{12}$), ExpiresInSeconds @range 5..300; modeled error ResourceNotFound (HTTP 400). A partner-app ARN that does not exist is rejected 400 ResourceNotFound.
Emulator: NotImplemented.
Why: no presigned partner-app URL can be minted, and the not-found branch is unreachable.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_partner_app_presigned_url.rs plus partner-app lookup.
Traces: sagemaker.create-partner-app-presigned-url-app-not-found, sagemaker.create-partner-app-presigned-url-missing-app.

Cause 9 of 10: GetScalingConfigurationRecommendation.
AWS: Smithy: GetScalingConfigurationRecommendation requires InferenceRecommendationsJobName, optional RecommendationId/EndpointName/TargetCpuUtilizationPerCore (@range 1..100); modeled error ResourceNotFound (HTTP 400). A recommendations job or endpoint that does not exist is rejected 400 ResourceNotFound; a call supplying neither EndpointName nor RecommendationId is rejected 400 ValidationException.
Emulator: NotImplemented for all of those.
Why: inference-recommender scaling consumers get an unmodeled error for both the not-found and the mutually-exclusive-parameter branch.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/get_scaling_configuration_recommendation.rs plus its input validation.
Traces: sagemaker.get-scaling-configuration-recommendation-endpoint-not-found, sagemaker.get-scaling-configuration-recommendation-for-valid-endpoint, sagemaker.get-scaling-configuration-recommendation-matches-endpoint, sagemaker.get-scaling-configuration-recommendation-not-found, sagemaker.get-scaling-configuration-recommendation-not-found-for-missing-endpoint, sagemaker.get-scaling-configuration-recommendation-unknown-endpoint-not-found, sagemaker.scaling-configuration-recommendation-for-live-endpoint, sagemaker.scaling-configuration-recommendation-not-found-for-unknown-endpoint.

Cause 10 of 10: PutModelPackageGroupPolicy.
AWS: Smithy: PutModelPackageGroupPolicy requires ModelPackageGroupName and ResourcePolicy; the only modeled error is ConflictException, yet recorded AWS rejects a group that does not exist with 400 ValidationException (group existence is validated rather than treated as a conflicting precondition).
Emulator: NotImplemented.
Why: registry resource policies cannot be set, and a client that distinguishes ValidationException from ConflictException gets neither.
Fix: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/put_model_package_group_policy.rs plus model-package-group lookup.
Traces: sagemaker.put-model-package-group-policy-precondition-conflict.

## Evidence
Replay output, one line per cause:
- sagemaker.attach-cluster-node-volume-nonexistent-node-not-found-rejected: "AssertionError: expected ValidationException, got 'NotImplemented'".
- sagemaker.cluster-node-batch-add-delete-lifecycle: "AssertionError: BatchAddClusterNodes: expected ResourceNotFound, got 'NotImplemented'"; sagemaker.cluster-node-batch-lifecycle: same.
- sagemaker.create-compute-quota-duplicate-name-conflict: "AssertionError: expected ValidationException, got NotImplemented" (expected message example "Specified cluster not found").
- sagemaker.parallel-create-cluster-scheduler-config-single-winner: "AssertionError: expected ValidationException on every racer ..., got ['NotImplemented', 'NotImplemented']".
- sagemaker.create-trial-*/trial-*-missing-experiment-rejected: "AssertionError: expected ResourceNotFound creating a trial against a nonexistent experiment, got 'NotImplemented'".
- sagemaker.create-model-card-export-job-*: "AssertionError: expected ResourceNotFound, got 'NotImplemented'".
- sagemaker.model-card-archived-update-rejected: "AssertionError: expected ResourceNotFound updating a nonexistent model card, got 'NotImplemented'".
- sagemaker.create-partner-app-presigned-url-app-not-found: "AssertionError: expected ResourceNotFound for a non-existent partner app ARN, got 'NotImplemented'".
- sagemaker.get-scaling-configuration-recommendation-*: "AssertionError: expected ResourceNotFound, got 'NotImplemented'"; sagemaker.scaling-configuration-recommendation-for-live-endpoint: "expected ValidationException when neither EndpointName nor RecommendationId is supplied, got 'NotImplemented'".
- sagemaker.put-model-package-group-policy-precondition-conflict: "AssertionError: expected ValidationException, got 'NotImplemented'".
In every case the wire response carries error code NotImplemented with message "operation <X> is scaffolded but not implemented in this emulator", instead of a 400 with the modeled client error.

## Steps to Reproduce
1. Boot the emulator and point a botocore sagemaker client at it.
2. AttachClusterNodeVolume(ClusterArn=arn:aws:sagemaker:<region>:<acct>:cluster/<12 lowercase alnum>, NodeId=i-0123456789abcdef0, VolumeId=vol-0123456789abcdef0) -> expect 400 ValidationException, observe NotImplemented.
3. BatchAddClusterNodes(ClusterName=<never-created>, NodesToAdd=[{InstanceGroupName:...,IncrementTargetCountBy:1}]) -> expect 400 ResourceNotFound.
4. CreateComputeQuota(Name=..., ClusterArn=<well-formed, nonexistent>, ComputeQuotaConfig={ComputeQuotaResources:[{InstanceType:ml.m5.large,Count:1}]}, ComputeQuotaTarget={TeamName:...}) -> expect 400 ValidationException.
5. CreateClusterSchedulerConfig(Name=..., ClusterArn=<nonexistent>, SchedulerConfig={...}) -> expect 400 ValidationException.
6. CreateTrial(TrialName=..., ExperimentName=<nonexistent>) -> expect 400 ResourceNotFound.
7. CreateModelCardExportJob(ModelCardName=<nonexistent>, ModelCardExportJobName=..., OutputConfig={S3OutputPath:...}) -> expect 400 ResourceNotFound.
8. UpdateModelCard(ModelCardName=<nonexistent>, ModelCardStatus=Draft) -> expect 400 ResourceNotFound.
9. CreatePartnerAppPresignedUrl(Arn=arn:aws:sagemaker:<region>:<acct>:partner-app/app-ABCDEF123456) -> expect 400 ResourceNotFound.
10. GetScalingConfigurationRecommendation(InferenceRecommendationsJobName=<nonexistent>) -> expect 400 ResourceNotFound; then the same call with neither EndpointName nor RecommendationId -> expect 400 ValidationException.
11. PutModelPackageGroupPolicy(ModelPackageGroupName=<nonexistent>, ResourcePolicy=<valid policy json>) -> expect 400 ValidationException.
Each call returns error code NotImplemented today.
