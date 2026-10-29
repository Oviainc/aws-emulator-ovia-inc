Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.e2e-endpoint-create-invoke-scale-teardown sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone sagemaker.endpoint-managed-inservice-transition sagemaker.sagemaker-endpoint-prediction-is-deterministic-and-ordered-in-its-input
Issue-Key: sagemaker.e2e-endpoint-create-invoke-scale-teardown|run#11|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", train_desc.get("FailureReason")
Issue-Key: sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone|run#15|AssertionError|assert training_desc["TrainingJobStatus"] == "Completed", training_desc.get("FailureReason")
Issue-Key: sagemaker.endpoint-managed-inservice-transition|_train_tiny_xgboost_model#4|AssertionError|raise AssertionError(
Issue-Key: sagemaker.sagemaker-endpoint-prediction-is-deterministic-and-ordered-in-its-input|run#16|AssertionError|assert train_desc["TrainingJobStatus"] == "Completed", (
Issue-Commit: 355c5d16add239c66c8799ffa26377be788e0f43

# sagemaker: an accepted CreateTrainingJob is never executed — the job ends Failed with an emulator-authored no-container-runtime FailureReason and no model artifact, so every endpoint lifecycle and inference check is blocked (1 cause)

> _Note: this issue was identified against the code base at commit `355c5d16add239c66c8799ffa26377be788e0f43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Cause 1 of 1: CreateTrainingJob is accepted but no training work is ever performed, so the job settles in Failed and no model artifact is produced. Failing checks: sagemaker.e2e-endpoint-create-invoke-scale-teardown, sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone, sagemaker.endpoint-managed-inservice-transition, sagemaker.sagemaker-endpoint-prediction-is-deterministic-and-ordered-in-its-input.

These four checks are NOT covered by the already-open training-job issues (20261025T095709Z-sagemaker-training-jobs-never-run-an-accepted-createtrainingjob.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md, 20261025T224154Z-sagemaker-createtrainingjob-is-accepted-but-never-executed-the.md, 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md): those issues carry other replay keys, and the four ids above are reported unowned by the suite. They are listed here so a fixer re-replays exactly them.

AWS: an accepted CreateTrainingJob runs the AlgorithmSpecification.TrainingImage; DescribeTrainingJob reports TrainingJobStatus advancing InProgress -> Completed (Smithy com.amazonaws.sagemaker#TrainingJobStatus = InProgress|Completed|Failed|Stopping|Stopped|Deleting), sets no FailureReason on success, and returns ModelArtifacts.S3ModelArtifacts (required S3Uri, documented as "The path of the S3 object that contains the model artifacts. For example, s3://bucket-name/keynameprefix/model.tar.gz"), with that object actually present under OutputDataConfig.S3OutputPath. CreateModel -> CreateEndpointConfig -> CreateEndpoint then loads those weights and InvokeEndpoint serves predictions.

Emulator: the job is accepted and then reported TrainingJobStatus=Failed with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location" — prose AWS never emits — and no artifact object is written. Every scenario that trains before deploying therefore stops at its first DescribeTrainingJob assertion, which is why endpoint lifecycle (Creating -> InService -> gone), managed-InService transition, autoscaling teardown and prediction-determinism are all unverifiable rather than merely failing.

Why: a client's train-then-deploy pipeline cannot run at all. There is no artifact to reference from CreateModel, so the whole control-plane lifecycle and the entire inference data plane are unreachable, and a caller that branches on TrainingJobStatus/FailureReason sees a terminal failure where AWS reports success. Suites that assert a deployment is healthy cannot be written against this emulator.

Fix: the training-job execution and state machine behind CreateTrainingJob (the component that advances TrainingJobStatus/SecondaryStatus and materializes the output artifact into the S3 object store), plus DescribeTrainingJob's ModelArtifacts projection. A deterministic synthesized model.tar.gz written under OutputDataConfig.S3OutputPath satisfies the contract; the requirement is Completed with no FailureReason and a readable artifact object.

Traces: the four check ids above; each fails on `assert <desc>["TrainingJobStatus"] == "Completed"` with the no-container-runtime FailureReason as the assertion detail.

## Evidence
Expected (AWS): DescribeTrainingJob(TrainingJobName=<job>) -> TrainingJobStatus "Completed", no FailureReason, ModelArtifacts.S3ModelArtifacts = s3://<bucket>/<prefix>/<job>/output/model.tar.gz, and that key is retrievable from S3; the subsequent CreateModel/CreateEndpointConfig/CreateEndpoint reach EndpointStatus InService and InvokeEndpoint returns a prediction.
Actual (emulator): TrainingJobStatus "Failed" with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"; no artifact object under the output prefix.
  - sagemaker.e2e-endpoint-create-invoke-scale-teardown: AssertionError: <that FailureReason> (assert train_desc["TrainingJobStatus"] == "Completed")
  - sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone: AssertionError: <that FailureReason>
  - sagemaker.endpoint-managed-inservice-transition: AssertionError: prerequisite training job did not complete: Failed (<that FailureReason>)
  - sagemaker.sagemaker-endpoint-prediction-is-deterministic-and-ordered-in-its-input: AssertionError: prerequisite training job failed, so there is nothing to deploy: "<that FailureReason>"
Model evidence: Smithy com.amazonaws.sagemaker#TrainingJobStatus includes Completed; com.amazonaws.sagemaker#ModelArtifacts.S3ModelArtifacts is a required S3Uri (pattern ^(https|s3)://([^/]+)/?(.*)$).

## Steps to Reproduce
1. Boot the emulator and replay sagemaker.endpoint-lifecycle-creating-to-inservice-to-gone.
2. Create an S3 bucket, upload a tiny training CSV, and create an execution role trusted by sagemaker.amazonaws.com.
3. Call CreateTrainingJob (built-in XGBoost TrainingImage, one ml.m5.large instance, InputDataConfig channel "train" pointing at the uploaded prefix, OutputDataConfig.S3OutputPath under the same bucket).
4. Poll DescribeTrainingJob until terminal: observe TrainingJobStatus "Failed" plus the no-container-runtime FailureReason instead of "Completed".
5. Observe that no model.tar.gz exists under OutputDataConfig.S3OutputPath, so the CreateModel step that would reference ModelArtifacts.S3ModelArtifacts cannot be performed and the remaining lifecycle/inference assertions never run.
