Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-feature-group-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-feature-group-lifecycle sagemaker-training-job-hyperparameter-numeric-precision-preserved
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreReadWriteType
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreReplicas
Issue-Key: sagemaker|DescribeFeatureGroup|2xx->2xx|$.OnlineStoreTotalItemCount
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.AlgorithmSpecification.MetricDefinitions
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.DisableEFA
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.ImageMetadata
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.InputDataConfig
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.DisableModelUpload
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.OutputDataConfig.RemoveJobNameFromS3OutputPath
Issue-Key: sagemaker|DescribeTrainingJob|2xx->2xx|$.ResourceConfig.UseReservedCapacity
Issue-Commit: 717da2edf9c8dae639a46753310fbbbfd49465e2

# SageMaker describe responses omit members present in the recorded response

> _Note: this issue was identified against the code base at commit `717da2edf9c8dae639a46753310fbbbfd49465e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFeatureGroup

## Description
Two net-new wire divergences in which a SageMaker read operation answers 200 but drops members the recorded AWS response carries. Both are projection gaps in the Describe* output builder, not state gaps: the resource exists and every other member matches. The open issue 'SageMaker read responses omit empty maps and lists entirely' (20261029T192819Z-sagemaker-read-responses-omit-empty-maps-and-lists.md) covers a related class (empty collections serialized as absent) but neither of these clusters is owned by it - the replay reports them with no owner - and the scalar members below are not empty collections at all.

Cause 1 of 2: DescribeFeatureGroup omits the online-store projection members.
AWS: a feature group created with an OnlineStoreConfig is described with its online-store read/write mode, its replica list and its item count always present. The checked-in Smithy model for this service (DescribeFeatureGroupResponse) does not declare OnlineStoreReadWriteType, OnlineStoreReplicas or OnlineStoreTotalItemCount, so this is recorded behavior only: the recording shows all three present on a 200 response, with the read/write mode at its default mode for an online store, the replica list empty when no replica is configured, and the item count zero when no record has been ingested.
Emulator: all three members are absent from the 200 body.
Why: a client that reads the online-store mode to decide whether it may write, or the item count to report store occupancy, sees None instead of a value and either branches wrongly or raises a key error on a successful describe.
Fix: the DescribeFeatureGroup output projection (the handler's response builder) must populate the online-store members from the feature group's stored OnlineStoreConfig and online-store state, emitting the empty replica list and zero count explicitly rather than skipping them.
Traces: sagemaker-feature-group-lifecycle#task#5.

Cause 2 of 2: DescribeTrainingJob omits seven members, including the echoed input channels and the algorithm's metric definitions.
AWS: DescribeTrainingJob returns AlgorithmSpecification.MetricDefinitions (for a built-in algorithm image AWS populates the algorithm's own metric set, each entry a {Name, Regex} pair), InputDataConfig (the channel list submitted to CreateTrainingJob, echoed back), and the resource/output flags the recording carries - DisableEFA, ImageMetadata, OutputDataConfig.DisableModelUpload, OutputDataConfig.RemoveJobNameFromS3OutputPath and ResourceConfig.UseReservedCapacity. Smithy: DescribeTrainingJobResponse declares AlgorithmSpecification, InputDataConfig (list of Channel, length 1..20), OutputDataConfig and ResourceConfig; the checked-in model does not declare DisableEFA, ImageMetadata, OutputDataConfig.DisableModelUpload, OutputDataConfig.RemoveJobNameFromS3OutputPath or ResourceConfig.UseReservedCapacity, so for those five it is recorded behavior only.
Emulator: all seven are absent from the 200 body.
Why: InputDataConfig is the authoritative record of what the job was given - a client that re-reads a job to recover its channels (the normal clone/retry path) gets nothing back, and a client that reads MetricDefinitions to parse training metrics has no regexes to apply.
Fix: the DescribeTrainingJob output projection and the training-job state it reads from - the create request's channels and output/resource configuration must be retained on the stored job and echoed, and the metric definitions must be filled from the algorithm image's metric set.
Traces: sagemaker-training-job-hyperparameter-numeric-precision-preserved#task#4.

## Evidence
Cause 1 - fixture sagemaker-feature-group-lifecycle step task#5, DescribeFeatureGroup, status 200 expected and 200 observed: $.OnlineStoreReadWriteType missing (expected 'ReadWrite', got None); $.OnlineStoreReplicas missing (expected [], got None); $.OnlineStoreTotalItemCount missing (expected 0, got None). Divergence key service=sagemaker|DescribeFeatureGroup|status=200->200|keys=$.OnlineStoreReadWriteType,$.OnlineStoreReplicas,$.OnlineStoreTotalItemCount.

Cause 2 - fixture sagemaker-training-job-hyperparameter-numeric-precision-preserved step task#4, DescribeTrainingJob, status 200 expected and 200 observed: $.AlgorithmSpecification.MetricDefinitions missing (expected the algorithm's full {Name, Regex} metric list, e.g. entries named 'train:progress', 'validation:mae', 'test:dcg', got None); $.InputDataConfig missing; $.DisableEFA missing; $.ImageMetadata missing; $.OutputDataConfig.DisableModelUpload missing; $.OutputDataConfig.RemoveJobNameFromS3OutputPath missing; $.ResourceConfig.UseReservedCapacity missing. Divergence key service=sagemaker|DescribeTrainingJob|status=200->200|keys=$.AlgorithmSpecification.MetricDefinitions,$.DisableEFA,$.ImageMetadata,$.InputDataConfig,$.OutputDataConfig.DisableModelUpload,$.OutputDataConfig.RemoveJobNameFromS3OutputPath,$.ResourceConfig.UseReservedCapacity.

Smithy consulted: DescribeFeatureGroupResponse members (no online-store read/write, replica or item-count members declared); DescribeTrainingJobResponse members AlgorithmSpecification / InputDataConfig (Channel list, @length 1..20) / OutputDataConfig / ResourceConfig.

## Steps to Reproduce
Replay the wire corpus against the emulator. Representative: fixture sagemaker-feature-group-lifecycle, step task#5 - create a feature group with an OnlineStoreConfig, then DescribeFeatureGroup and read OnlineStoreReadWriteType / OnlineStoreReplicas / OnlineStoreTotalItemCount from the 200 body. Second cause: fixture sagemaker-training-job-hyperparameter-numeric-precision-preserved, step task#4 - CreateTrainingJob with an InputDataConfig channel list and a built-in algorithm image, then DescribeTrainingJob and read AlgorithmSpecification.MetricDefinitions, InputDataConfig, DisableEFA, ImageMetadata, OutputDataConfig.DisableModelUpload, OutputDataConfig.RemoveJobNameFromS3OutputPath and ResourceConfig.UseReservedCapacity.
