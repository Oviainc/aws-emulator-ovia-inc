Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-algorithm-duplicate-and-missing-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-algorithm-duplicate-and-missing-errors sagemaker-feature-group-lifecycle sagemaker-model-create-configure-verify sagemaker-model-package-create-and-verify sagemaker-model-package-metric-numeric-precision-preserved sagemaker-pipeline-lifecycle sagemaker-training-job-hyperparameter-numeric-precision-preserved sagemaker-model-package-unicode-description-round-trip
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeAlgorithm|2xx->2xx|$.CertifyForMarketplace
Issue-Key: sagemaker|DescribeAlgorithm|2xx->2xx|$.TrainingSpecification.TrainingChannels[*].IsRequired
Issue-Key: sagemaker|DescribeAlgorithm|2xx->2xx|$.TrainingSpecification.TrainingImageDigest
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreReadWriteType
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreReplicas
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreTotalItemCount
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreTotalSizeBytes
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.ThroughputConfig
Issue-Key: sagemaker|DescribeModel|2xx->2xx|$.EnableNetworkIsolation
Issue-Key: sagemaker|DescribeModel|2xx->2xx|$.PrimaryContainer.Mode
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.CertifyForMarketplace
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.CreatedBy.IamIdentity.Arn
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.CreatedBy.IamIdentity.PrincipalId
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.InferenceSpecification.Containers[*].ImageDigest
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.InferenceSpecification.Containers[*].IsCheckpoint
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.ModelPackageRegistrationType
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.ModelPackageStatusDetails.ImageScanStatuses
Issue-Key: sagemaker|DescribeModelPackage|2xx->2xx|$.ModelPackageStatus
Issue-Key: sagemaker|DescribePipeline|2xx->2xx|$.CreatedBy
Issue-Key: sagemaker|DescribePipeline|2xx->2xx|$.LastModifiedBy
Issue-Key: sagemaker|DescribePipeline|2xx->2xx|$.PipelineDisplayName
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.AlgorithmSpecification.EnableSageMakerMetricsTimeSeries
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.AlgorithmSpecification.MetricDefinitions
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.DisableEFA
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.EnableInterContainerTrafficEncryption
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.EnableManagedSpotTraining
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.EnableNetworkIsolation
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.ImageMetadata
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.InputDataConfig
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.CompressionType
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.DisableModelUpload
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.KmsKeyId
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.RemoveJobNameFromS3OutputPath
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.ProfilingStatus
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.ResourceConfig.UseReservedCapacity
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.SecondaryStatusTransitions
Issue-Key: sagemaker|BatchDescribeModelPackage|2xx->2xx|$.ModelPackageSummaries.arn
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# SageMaker Describe responses drop modeled defaults and echoed configuration

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlgorithm

## Description
This issue bundles 8 causes with one root fault: SageMaker read paths serve only the members the client literally submitted. The service-applied defaults (false booleans, enum defaults, service-computed digests/counters, status detail sub-lists) and parts of the submitted request the service echoes back are never materialized, so Describe/BatchDescribe responses are structurally thinner than AWS's. Fix belongs in each create handler's defaulting step and the matching Describe output construction.

Cause 1 of 8: DescribeAlgorithm. AWS: CertifyForMarketplace defaults to false, TrainingSpecification.TrainingChannels[].IsRequired defaults to false, and TrainingSpecification.TrainingImageDigest is resolved by the service from the training image. Emulator: all three absent. Fix: CreateAlgorithm defaulting + DescribeAlgorithm output. Traces: sagemaker-algorithm-duplicate-and-missing-errors#task#4.

Cause 2 of 8: DescribeFeatureGroup. AWS: OnlineStoreReadWriteType 'ReadWrite', OnlineStoreReplicas [], OnlineStoreTotalItemCount 0, OnlineStoreTotalSizeBytes 0 and ThroughputConfig {'ThroughputMode': 'OnDemand'} are all served for an online-store feature group that specified none of them (Smithy DescribeFeatureGroup output carries ThroughputConfig and OnlineStoreTotalSizeBytes). Emulator: all five absent. Fix: CreateFeatureGroup defaulting + DescribeFeatureGroup output. Traces: sagemaker-feature-group-lifecycle#task#5.

Cause 3 of 8: DescribeModel. AWS: EnableNetworkIsolation false and PrimaryContainer.Mode 'SingleModel' are defaulted. Emulator: both absent. Fix: CreateModel defaulting + DescribeModel output. Traces: sagemaker-model-create-configure-verify#task#2.

Cause 4 of 8: DescribeModelPackage. AWS: CertifyForMarketplace false, ModelPackageRegistrationType 'Registered', ModelPackageStatusDetails.ImageScanStatuses [], InferenceSpecification.Containers[].ImageDigest (resolved by the service) and Containers[].IsCheckpoint false, plus CreatedBy.IamIdentity.PrincipalId. Emulator: all absent. Fix: CreateModelPackage defaulting + DescribeModelPackage output. Traces: sagemaker-model-package-create-and-verify#task#3.

Cause 5 of 8: DescribeModelPackage (metric-precision fixture). Same omissions as cause 4, and in addition ModelPackageStatus never leaves 'Pending' where AWS reaches 'Completed' (Smithy marks ModelPackageStatus and ModelPackageStatusDetails @required). Emulator: 'Pending' plus the missing members. Fix: as cause 4, plus the model-package status machine. Traces: sagemaker-model-package-metric-numeric-precision-preserved#task#4.

Cause 6 of 8: DescribePipeline. AWS: PipelineDisplayName defaults to the PipelineName when the create omitted it, and CreatedBy/LastModifiedBy carry an IamIdentity with Arn and PrincipalId. Emulator: all three absent. Fix: CreatePipeline defaulting + DescribePipeline output. Traces: sagemaker-pipeline-lifecycle#task#4.

Cause 7 of 8: DescribeTrainingJob. AWS: the response carries the service-resolved AlgorithmSpecification.MetricDefinitions for the built-in algorithm image, AlgorithmSpecification.EnableSageMakerMetricsTimeSeries false, the echoed InputDataConfig, OutputDataConfig defaults (CompressionType, DisableModelUpload, KmsKeyId, RemoveJobNameFromS3OutputPath), EnableNetworkIsolation / EnableInterContainerTrafficEncryption / EnableManagedSpotTraining / DisableEFA false, ResourceConfig.UseReservedCapacity, ProfilingStatus, ImageMetadata and SecondaryStatusTransitions. Emulator: all absent. Fix: CreateTrainingJob defaulting/echo + DescribeTrainingJob output. Traces: sagemaker-training-job-hyperparameter-numeric-precision-preserved#task#4.

Cause 8 of 8: BatchDescribeModelPackage. AWS: ModelPackageSummaries is keyed by the requested model-package ARN and the entry carries ModelPackageArn, ModelPackageGroupName, ModelPackageVersion, ModelPackageDescription (byte-preserved, including non-ASCII), ModelPackageRegistrationType 'Registered', ModelPackageStatus 'Completed' and CreationTime. Emulator: no entry for the ARN at all. Fix: BatchDescribeModelPackage handler / summary projection. Traces: sagemaker-model-package-unicode-description-round-trip#task#3.

## Evidence
Cause 1: $.CertifyForMarketplace expected False got None; $.TrainingSpecification.TrainingChannels[0].IsRequired expected False got None; $.TrainingSpecification.TrainingImageDigest expected a sha256 digest got None. Cause 2: $.OnlineStoreReadWriteType expected 'ReadWrite', $.OnlineStoreReplicas expected [], $.OnlineStoreTotalItemCount expected 0, $.OnlineStoreTotalSizeBytes expected 0, $.ThroughputConfig expected {'ThroughputMode':'OnDemand'} - all got None. Cause 3: $.EnableNetworkIsolation expected False got None; $.PrimaryContainer.Mode expected 'SingleModel' got None. Causes 4/5: $.CertifyForMarketplace, $.ModelPackageRegistrationType ('Registered'), $.ModelPackageStatusDetails.ImageScanStatuses ([]), $.InferenceSpecification.Containers[0].ImageDigest, $.InferenceSpecification.Containers[0].IsCheckpoint (False), $.CreatedBy.IamIdentity.PrincipalId all got None; cause 5 additionally $.ModelPackageStatus expected 'Completed' got 'Pending'. Cause 6: $.PipelineDisplayName, $.CreatedBy, $.LastModifiedBy all got None. Cause 7: $.AlgorithmSpecification.MetricDefinitions, $.AlgorithmSpecification.EnableSageMakerMetricsTimeSeries (False), $.InputDataConfig, $.OutputDataConfig.CompressionType/DisableModelUpload/KmsKeyId/RemoveJobNameFromS3OutputPath, $.EnableNetworkIsolation, $.EnableInterContainerTrafficEncryption, $.EnableManagedSpotTraining, $.DisableEFA, $.ResourceConfig.UseReservedCapacity, $.ProfilingStatus, $.ImageMetadata, $.SecondaryStatusTransitions all got None. Cause 8: $.ModelPackageSummaries.<model-package arn> expected a full summary, got None. Smithy: DescribeModelPackageOutput declares ModelPackageRegistrationType, CertifyForMarketplace, ModelPackageStatusDetails (@required), CreatedBy -> UserContext.

## Steps to Reproduce
Replay the wire corpus. Representative: fixture sagemaker-algorithm-duplicate-and-missing-errors step task#4. The other seven causes are the fixture#step bindings in trace_ids: create the resource with a minimal request and read it back; the emulator's body is missing the service-defaulted and service-resolved members the recording carries.
