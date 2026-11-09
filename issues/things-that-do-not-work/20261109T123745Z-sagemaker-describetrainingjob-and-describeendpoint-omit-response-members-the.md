Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-train-host-and-invoke-xgboost-model
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-train-host-and-invoke-xgboost-model
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.FinalMetricDataList[*].Timestamp
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.InputDataConfig[*].EnableFFM
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.InputDataConfig[*].RecordWrapperType
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.TrainingEndTime
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.TrainingJobOutput
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.TrainingStartTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.ProductionVariants[*].CurrentInstanceCount
Issue-Key: sagemaker|DescribeEndpoint|2xx->2xx|$.ProductionVariants[*].DeployedImages
Issue-Related: 20261102T105242Z-sagemaker-describe-responses-omit-members-present-in-the.md
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# SageMaker DescribeTrainingJob and DescribeEndpoint omit response members the recording populates

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTrainingJob

## Description
2 causes: two Describe responses drop members AWS always populates for the resource being described, so a client reading those members sees absent where AWS reports a value.

Cause 1 of 2 - DescribeTrainingJob omits AlgorithmSpecification.MetricDefinitions and the other per-job detail members.
AWS: Smithy: DescribeTrainingJob output member AlgorithmSpecification -> AlgorithmSpecification, whose MetricDefinitions list carries the metric Name/Regex pairs the algorithm publishes; InputDataConfig -> InputDataConfig (a list of Channel, with RecordWrapperType); OutputDataConfig -> OutputDataConfig; ResourceConfig -> ResourceConfig; FinalMetricDataList -> FinalMetricDataList (each entry has a Timestamp); TrainingStartTime / TrainingEndTime / LastModifiedTime -> Timestamp. For a job launched from a built-in/first-party algorithm image, AWS resolves the algorithm's full MetricDefinitions set onto the response even when the request omitted it - the recorded response carries the whole xgboost metric set (train:mae, train:merror, validation:logloss, validation:auc and the rest, each with its '.*\[[0-9]+\].*#011<metric>:(...)' regex). The remaining members the replay flags - ImageMetadata, TrainingJobOutput, DisableEFA, OutputDataConfig.DisableModelUpload, OutputDataConfig.RemoveJobNameFromS3OutputPath, ResourceConfig.UseReservedCapacity, InputDataConfig[].EnableFFM - are not in this checkout's on-disk model, so for those it is recorded behavior only: the recording shows them present on every DescribeTrainingJob response.
Emulator: returns the response with AlgorithmSpecification.MetricDefinitions and each of those members absent (null) rather than populated; the status is 200 either way, so nothing signals the omission.
Why: a client that reads AlgorithmSpecification.MetricDefinitions to learn which metric names a job emits - the SageMaker Python SDK's analytics/tuning paths do exactly this - gets an empty set and concludes the job publishes no metrics; a client that reads RecordWrapperType or the OutputDataConfig flags to reconstruct the job's configuration (a round-trip of describe-then-recreate) silently loses those settings.
Fix: the DescribeTrainingJob handler's response projection and the training-job state it reads from (the algorithm's MetricDefinitions must be resolved onto the stored job at create time and echoed back; the per-channel and output/resource flags must be stored and echoed).
Traces: sagemaker-train-host-and-invoke-xgboost-model#task#6.

Cause 2 of 2 - DescribeEndpoint omits ProductionVariants[].DeployedImages and CurrentInstanceCount.
AWS: Smithy: DescribeEndpoint output member ProductionVariants -> ProductionVariantSummaryList of ProductionVariantSummary, which models DeployedImages ("An array of DeployedImage objects that specify the Amazon EC2 Container Registry paths of the inference images deployed on instances of this ProductionVariant") and CurrentInstanceCount -> TaskCount ("The number of instances associated with the variant", @range min 0). AWS populates both for a deployed endpoint: DeployedImages carries one entry per container with SpecifiedImage (the image URI as given in the endpoint config), ResolvedImage (that URI with the digest it resolved to) and ResolutionTime; CurrentInstanceCount carries the variant's instance count, which is 0 while no instances are yet attributed to the variant.
Emulator: returns each ProductionVariants entry with DeployedImages and CurrentInstanceCount absent (null).
Why: a client that verifies WHICH image a variant is actually serving - the standard way to confirm a deployment rolled out, and the only place the resolved digest is exposed - cannot do so, and a client that reads CurrentInstanceCount to decide whether the variant has capacity cannot distinguish "zero instances" from "not reported". Autoscaling and deployment-verification logic keyed on either member breaks.
Fix: the DescribeEndpoint handler's ProductionVariantSummary projection and the endpoint state behind it (record the endpoint config's container images as DeployedImages when the endpoint is created, and report the variant's instance count).
Note on the same clusters' timestamp keys: the divergence also lists $.CreationTime / $.LastModifiedTime (DescribeEndpoint) and the training job's CreationTime/LastModifiedTime/TrainingStartTime/TrainingEndTime and FinalMetricDataList[0].Timestamp against '<shaped-not-asserted>'. Those are per-run clock values, not requirements, and are NOT part of this issue; only the absent members above are.

Related but not covering: 20261102T105242Z-sagemaker-describe-responses-omit-members-present-in-the.md owns the DescribeTrainingJob cluster from fixture sagemaker-training-job-hyperparameter-numeric-precision-preserved and the DescribeFeatureGroup cluster. The two clusters here are reported unowned by the replay, so they are filed with their own replay keys.

## Evidence
Cause 1 - sagemaker-train-host-and-invoke-xgboost-model#task#6 DescribeTrainingJob, status 200 -> 200: "$.AlgorithmSpecification.MetricDefinitions: missing (expected [{'Name': 'train:mae', 'Regex': '.*\\[[0-9]+\\].*#011train-mae:([-+]?[0-9]*\\.?[0-9]+(?:[eE][-+]?[0-9]+)?).*'}, {'Name': 'train:merror', ...}, ...], got None)". divergence_key service=sagemaker|DescribeTrainingJob|status=200->200|keys=$.AlgorithmSpecification.MetricDefinitions,$.CreationTime,$.DisableEFA,$.FinalMetricDataList[0].Timestamp,$.ImageMetadata,$.InputDataConfig[0].EnableFFM,$.InputDataConfig[0].RecordWrapperType,$.LastModifiedTime,$.OutputDataConfig.DisableModelUpload,$.OutputDataConfig.RemoveJobNameFromS3OutputPath,$.ResourceConfig.UseReservedCapacity,$.TrainingEndTime,$.TrainingJobOutput,$.TrainingStartTime. The metric NAMES and their regex templates are the requirement (they are the algorithm's fixed metric set); the number of entries follows from the algorithm, not from this run.

Cause 2 - sagemaker-train-host-and-invoke-xgboost-model#task#11 DescribeEndpoint, status 200 -> 200:
  $.ProductionVariants[0].CurrentInstanceCount: missing (expected 0, got None)
  $.ProductionVariants[0].DeployedImages: missing (expected [{'ResolutionTime': '<shaped-not-asserted>', 'ResolvedImage': '<account>.dkr.ecr.<region>.amazonaws.com/xgboost@sha256:<digest>', 'SpecifiedImage': '<account>.dkr.ecr.<region>.amazonaws.com/xgboost:1'}], got None)
  (divergence_key service=sagemaker|DescribeEndpoint|status=200->200|keys=$.CreationTime,$.LastModifiedTime,$.ProductionVariants[0].CurrentInstanceCount,$.ProductionVariants[0].DeployedImages)
The requirement is that DeployedImages is present with one entry per container carrying SpecifiedImage (the config's image URI), ResolvedImage (that URI at a digest) and ResolutionTime, and that CurrentInstanceCount is present; the specific registry account, repository and digest in the recording are that run's values.

## Steps to Reproduce
1. Replay fixture sagemaker-train-host-and-invoke-xgboost-model.
2. At task#6, DescribeTrainingJob for the xgboost training job: read $.AlgorithmSpecification.MetricDefinitions - absent, where the recording lists the algorithm's metric name/regex set. Read $.InputDataConfig[0].RecordWrapperType, $.OutputDataConfig.DisableModelUpload, $.OutputDataConfig.RemoveJobNameFromS3OutputPath, $.ResourceConfig.UseReservedCapacity, $.ImageMetadata, $.TrainingJobOutput and $.DisableEFA - all absent.
3. At task#11, DescribeEndpoint for the endpoint created from that model: read $.ProductionVariants[0].DeployedImages and $.ProductionVariants[0].CurrentInstanceCount - both absent, where the recording has a one-entry DeployedImages list and CurrentInstanceCount 0.
