Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone
Issue-Key: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone|run#16|AssertionError|assert training_desc["TrainingJobStatus"] == "Completed", training_desc.get("FailureReason")
Issue-Commit: fdc7c4a374233335cd2732abbc09ac690a8a7f08

# SageMaker endpoint lifecycle check blocked: an accepted CreateTrainingJob is never executed — the job settles Failed with an emulator-authored no-container-runtime FailureReason and no model artifact (1 cause)

> _Note: this issue was identified against the code base at commit `fdc7c4a374233335cd2732abbc09ac690a8a7f08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Cause 1 of 1: CreateTrainingJob is accepted but no training work is ever performed, so DescribeTrainingJob settles the job in Failed with an emulator-authored FailureReason and no model artifact is materialized under OutputDataConfig.S3OutputPath. Failing check: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone.

Relation to already-open issues: this is the same root cause as the open issue 20261029T203709Z-sagemaker-an-accepted-createtrainingjob-is-never-executed-the.md (and the earlier 20261025T095709Z-sagemaker-training-jobs-never-run-an-accepted-createtrainingjob.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md, 20261025T224154Z-sagemaker-createtrainingjob-is-accepted-but-never-executed-the.md, 20261030T171442Z-sagemaker-server-side-job-execution-is-absent-training.md). The current replay reports this check as UNOWNED — the suite matched no open issue's replay key to it, while it did mark the sibling checks (e2e-endpoint-create-invoke-scale-teardown, endpoint-managed-inservice-transition, sagemaker-endpoint-prediction-is-deterministic-and-ordered-in-its-input) as owned — so it is filed here to keep this check's replay key attached to a work item. A fixer closing the training-execution cause should close this issue together with those, after re-replaying the check id below.

AWS: an accepted CreateTrainingJob actually runs AlgorithmSpecification.TrainingImage. DescribeTrainingJob then advances TrainingJobStatus through the modeled enum (Smithy com.amazonaws.sagemaker#TrainingJobStatus = InProgress|Completed|Failed|Stopping|Stopped|Deleting) to Completed, with SecondaryStatus progressing through com.amazonaws.sagemaker#SecondaryStatus (Starting, Downloading, Training, Uploading, Completed), sets NO FailureReason on success, and returns ModelArtifacts.S3ModelArtifacts (Smithy com.amazonaws.sagemaker#ModelArtifacts.S3ModelArtifacts, required S3Uri, @length 0..1024, @pattern ^(https|s3)://([^/]+)/?(.*)$, documented as "The path of the S3 object that contains the model artifacts. For example, s3://bucket-name/keynameprefix/model.tar.gz") pointing at an object that is really readable under the request's OutputDataConfig.S3OutputPath. Only then can CreateModel -> CreateEndpointConfig -> CreateEndpoint reach EndpointStatus InService and InvokeEndpoint serve a prediction.

Emulator: the job is accepted, and the first terminal DescribeTrainingJob reports TrainingJobStatus="Failed" with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location" — prose AWS never emits for any failure class — and no artifact object exists under the output prefix. The whole fixed sentence is emulator-authored: AWS's FailureReason for a genuine algorithm failure is container/algorithm-sourced text (e.g. "ClientError: ...") and is absent entirely on a Completed job. The check therefore stops at its first status assertion and never reaches the Creating -> InService -> post-DeleteEndpoint ValidationException part of the lifecycle it exists to verify.

Why: a client's train-then-deploy pipeline cannot run at all against this emulator. A caller that branches on TrainingJobStatus sees a terminal failure where AWS reports success; a caller that reads ModelArtifacts.S3ModelArtifacts to feed CreateModel has no artifact to reference, so the endpoint control-plane lifecycle and the entire inference data plane are unreachable rather than merely wrong.

Fix: the training-job execution and state machine behind CreateTrainingJob — the component that advances TrainingJobStatus/SecondaryStatus toward Completed and materializes the output artifact into the S3 object store — plus DescribeTrainingJob's ModelArtifacts projection. A deterministically synthesized model.tar.gz written under the request's OutputDataConfig.S3OutputPath satisfies the contract; the requirement is Completed with no FailureReason and a readable artifact object at the returned S3Uri.

Traces: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone, which fails on `assert training_desc["TrainingJobStatus"] == "Completed", training_desc.get("FailureReason")` with that FailureReason as the assertion detail.

## Evidence
Expected (AWS): DescribeTrainingJob(TrainingJobName=<job>) -> TrainingJobStatus "Completed", SecondaryStatus "Completed", no FailureReason member, ModelArtifacts.S3ModelArtifacts = s3://<output-bucket>/<prefix>/<job-name>/output/model.tar.gz (the bucket/prefix come from the request's OutputDataConfig.S3OutputPath, the job name from the request), and that key retrievable via S3 GetObject; the subsequent CreateModel/CreateEndpointConfig/CreateEndpoint reach EndpointStatus InService, InvokeEndpoint returns a non-empty body, and DescribeEndpoint after DeleteEndpoint is refused with ValidationException.
Actual (emulator): TrainingJobStatus "Failed" with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location" (fixed, emulator-authored text with no variable parts — AWS emits no such sentence), and no object under the output prefix. Replay detail: check_sagemaker_endpoint_lifecycle_creating_to_inservice_to_gone.py line 75 -> AssertionError: This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location.
Model evidence: Smithy com.amazonaws.sagemaker#TrainingJobStatus includes Completed; com.amazonaws.sagemaker#SecondaryStatus includes Starting/Downloading/Training/Uploading/Completed; com.amazonaws.sagemaker#ModelArtifacts.S3ModelArtifacts is a required S3Uri (@length 0..1024, @pattern ^(https|s3)://([^/]+)/?(.*)$).

## Steps to Reproduce
1. Boot the emulator and replay the behavioral check sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone.
2. Create an IAM role trusted by sagemaker.amazonaws.com, create an S3 bucket, and PutObject a small CSV at train/train.csv.
3. Call CreateTrainingJob with a built-in XGBoost AlgorithmSpecification.TrainingImage (TrainingInputMode File), RoleArn = that role, InputDataConfig channel "train" with S3DataSource.S3Uri = s3://<bucket>/train/, OutputDataConfig.S3OutputPath = s3://<bucket>/output/, ResourceConfig one ml.m5.large, StoppingCondition.MaxRuntimeInSeconds 900.
4. Poll DescribeTrainingJob until terminal: observe TrainingJobStatus "Failed" with the no-container-runtime FailureReason instead of "Completed" with no FailureReason.
5. GetObject/ListObjectsV2 under s3://<bucket>/output/ : no model.tar.gz exists, so ModelArtifacts.S3ModelArtifacts cannot be fed to CreateModel and the remaining lifecycle assertions (Creating -> InService -> invoke -> DeleteEndpoint -> ValidationException) never execute.
