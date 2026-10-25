Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason s3__sagemaker.sagemaker-model-creation-references-s3-model-data-positive
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive|run#15|AssertionError|assert train_status == "Completed", train_status
Issue-Key: s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row|run#18|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason|run#16|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Key: s3__sagemaker.sagemaker-model-creation-references-s3-model-data-positive|run#14|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Commit: a348f7a60ca5edb956d11f71c3f040d86e771b20

# Cross-service S3+SageMaker flows blocked: an accepted training job ends Failed with a no-container-runtime reason and writes no model.tar.gz to the declared S3 output path

> _Note: this issue was identified against the code base at commit `a348f7a60ca5edb956d11f71c3f040d86e771b20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
This issue bundles ONE root cause; it must be closed before submitting. It is the cross-service (S3 + SageMaker) face of the same execution gap, filed under the check group's own service token s3__sagemaker.

Cause 1 of 1: a well-formed CreateTrainingJob whose InputDataConfig reads from S3 and whose OutputDataConfig names an S3 output path is accepted, but never executed: DescribeTrainingJob reports TrainingJobStatus 'Failed' and nothing is written under OutputDataConfig.S3OutputPath, so the S3-to-SageMaker integration (artifact produced in S3, then consumed by CreateModel / Endpoint / batch transform) is never exercised.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker): CreateTrainingJob requires TrainingJobName (@length 1-63), RoleArn and OutputDataConfig, accepts InputDataConfig (@length 1-20) with S3DataSource.S3Uri, returns only TrainingJobArn, and models ResourceInUse / ResourceLimitExceeded / ResourceNotFound (400). DescribeTrainingJob models ModelArtifacts, TrainingJobStatus and SecondaryStatus as REQUIRED output members. Recorded behavior: AWS reads the channel objects from the input S3 prefix, drives the job InProgress -> 'Completed', writes the compressed model artifact under the caller's OutputDataConfig.S3OutputPath (so the object is visible to S3 HeadObject/GetObject), and returns ModelArtifacts.S3ModelArtifacts as an s3:// URI inside that same bucket/prefix. Deleting that S3 object afterwards makes a subsequent endpoint deploy fail with an S3-origin FailureReason.

Emulator: Create succeeds, then TrainingJobStatus goes to 'Failed' with FailureReason (example text) "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"; the declared S3 output prefix stays empty, so ModelArtifacts.S3ModelArtifacts names nothing that exists.

Why: every client that uses S3 as the artifact handoff between training and serving breaks. The artifact-location assertion (artifact URI must start with s3://<the caller's buc>/) cannot hold, CreateModel/CreateEndpoint have no ModelDataUrl to consume, batch transform and the real-time endpoint have no model to agree on row-by-row, and the negative scenario that deletes the artifact to prove an S3-origin endpoint FailureReason has nothing to delete — a genuine S3 failure is indistinguishable from a job that never ran. CloudFormation-deployed stacks that provision the bucket and role succeed, then the flow dies at the first training step.

Fix: the training-job execution path in SageMaker state, including its S3 side effect — the CreateTrainingJob handler and the job state read back by DescribeTrainingJob must advance the job to Completed (SecondaryStatus/SecondaryStatusTransitions/TrainingStartTime/TrainingEndTime) and materialize a model artifact object into S3 at the declared OutputDataConfig.S3OutputPath, with ModelArtifacts.S3ModelArtifacts pointing at that real object so S3 HeadObject/GetObject and downstream CreateModel resolve it. Files: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs, .../src/ops/describe_training_job.rs, .../src/state.rs. Related but NOT the same cause: 20261022T120642Z-sagemaker-createtrainingjob-stub.md claims CreateTrainingJob is an unimplemented stub (here it answered and persisted the job), and 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md covers processing / pipeline / tuning / AutoML execution and excludes the training-job surface.

Traces: s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive, s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row, s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason, s3__sagemaker.sagemaker-model-creation-references-s3-model-data-positive.

## Evidence
All four s3__sagemaker checks stop at the same terminal-status assertion right after the training job they staged from S3:
- s3__sagemaker.cfn-deploy-sagemaker-endpoint-config-stack-positive: `assert train_status == "Completed", train_status` -> AssertionError: Failed (the stack's bucket/role Outputs resolved and the train/train.csv PutObject succeeded; the next assertion, `artifact_uri.startswith(f"s3://{bucket}/")`, is unreachable)
- s3__sagemaker.sagemaker-batch-transform-and-realtime-endpoint-agree-on-every-row: `assert train_desc["TrainingJobStatus"] == "Completed"` -> AssertionError: prerequisite training job failed, so neither serving path has a model: "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"
- s3__sagemaker.sagemaker-endpoint-deploy-from-a-deleted-model-artifact-fails-with-an-s3-failure-reason: same assertion -> AssertionError: prerequisite training job failed, so there is no artifact to delete: "This emulator has no container runtime, ..."
- s3__sagemaker.sagemaker-model-creation-references-s3-model-data-positive: same assertion -> AssertionError: prerequisite training job failed: This emulator has no container runtime, ...

Expected (AWS): TrainingJobStatus 'Completed' and a model artifact object present under the request's OutputDataConfig.S3OutputPath, with DescribeTrainingJob.ModelArtifacts.S3ModelArtifacts resolving under s3://<that bucket>/. Actual (emulator): TrainingJobStatus 'Failed' with the emulator-specific FailureReason above and an empty output prefix. The Create/Describe calls return 200; the divergence is the terminal status value and the absent S3 object, not an error code.

## Steps to Reproduce
1. Deploy (or create directly) an S3 bucket plus a SageMaker execution role whose policy grants s3:GetObject/PutObject/ListBucket on that bucket; read the bucket name and role ARN back from the stack Outputs.
2. PutObject s3://<bucket>/train/train.csv with a few CSV rows.
3. CreateTrainingJob(TrainingJobName=<name>, AlgorithmSpecification={'TrainingImage': <built-in algorithm ECR image for the region>, 'TrainingInputMode': 'File'}, RoleArn=<role arn>, InputDataConfig=[{'ChannelName': 'train', 'DataSource': {'S3DataSource': {'S3DataType': 'S3Prefix', 'S3Uri': 's3://<bucket>/train/', 'S3DataDistributionType': 'FullyReplicated'}}, 'ContentType': 'text/csv'}], OutputDataConfig={'S3OutputPath': 's3://<bucket>/output/'}, ResourceConfig={'InstanceType': 'ml.m5.large', 'InstanceCount': 1, 'VolumeSizeInGB': 5}, StoppingCondition={'MaxRuntimeInSeconds': 900}).
4. Poll DescribeTrainingJob until terminal: expect TrainingJobStatus 'Completed' and ModelArtifacts.S3ModelArtifacts starting with s3://<bucket>/; observe 'Failed' with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location".
5. Confirm the S3 side effect is missing: ListObjectsV2/HeadObject on s3://<bucket>/output/ returns nothing (AWS has the artifact object there).
6. Consequences to confirm after the fix: CreateModel with ModelDataUrl=<artifact>, then (a) CreateEndpointConfig/CreateEndpoint + InvokeEndpoint and CreateTransformJob over the same rows agree per row, and (b) DeleteObject on the artifact then deploying an endpoint from it yields an S3-origin endpoint FailureReason instead of an unreachable step.
