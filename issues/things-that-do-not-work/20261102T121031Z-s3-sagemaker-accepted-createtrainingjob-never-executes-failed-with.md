Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative|run#21|AssertionError|assert positive_described["TrainingJobStatus"] == "Completed", (
Issue-Key: s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive|run#16|AssertionError|assert final == "Completed", (
Issue-Key: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative|run#12|AssertionError|raise AssertionError(
Issue-Commit: 54ea3114e7f16ccf9b82e09febe15fda2a85bac2

# s3__sagemaker: accepted CreateTrainingJob never executes (Failed with emulator-authored FailureReason), and a role without a sagemaker.amazonaws.com trust relationship is accepted instead of refused (2 causes)

> _Note: this issue was identified against the code base at commit `54ea3114e7f16ccf9b82e09febe15fda2a85bac2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Two net-new root causes in the cross-service S3+SageMaker training path. Both were reported by the behavioral replay as plain failures: the report did NOT mark either as ALREADY FILED or OWNED, so neither carries an existing replay key.

Cause 1 of 2: an accepted CreateTrainingJob is never executed — the job settles Failed with an emulator-authored FailureReason and no model artifact is written to the declared S3 output path.
Failing check ids: s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive, s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative.
  AWS:      A CreateTrainingJob that is accepted (valid ECR training image, an S3 input channel the execution role can read, and an OutputDataConfig.S3OutputPath it can write) runs the algorithm container server-side; DescribeTrainingJob progresses TrainingJobStatus InProgress -> Completed and reports ModelArtifacts.S3ModelArtifacts naming a model.tar.gz that is actually present under S3OutputPath. Smithy: DescribeTrainingJob.TrainingJobStatus is enum TrainingJobStatus {InProgress, Completed, Failed, Stopping, Stopped}; FailureReason (target FailureReason) is populated only for a job that genuinely failed, and ModelArtifacts is the modeled carrier of the artifact location. The model says nothing about *how* the work is performed — recorded behavior only for the terminal state: the recordings reach TrainingJobStatus="Completed" with an artifact readable from S3.
  Emulator: the create is accepted with 200, then DescribeTrainingJob settles on TrainingJobStatus="Failed" with FailureReason exactly "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location". No object is written under the declared S3 output prefix. The FailureReason is emulator-authored prose about the emulator's own implementation; it is not any failure reason AWS emits, and its fixed text is wholly invented (AWS failure reasons name the algorithm/container/permission condition that failed, not the server's missing runtime).
  Why:      A client that waits for Completed never gets it, so every downstream step keyed on a trained artifact (CreateModel on the reported S3ModelArtifacts, an EndpointConfig/Endpoint deploy, a CFN stack whose training resource must settle) fails or hangs. A client that branches on FailureReason text sees a string that matches no AWS failure class, so error handling and retry classification are wrong. Worst, the emulator reports a FAILURE for a request AWS would have succeeded, inverting the verdict of every positive training scenario.
  Fix:      The CreateTrainingJob handler and the training-job state machine that advances a job to its terminal state: a job accepted with a resolvable image and reachable S3 input/output must progress to Completed, write a model artifact object under OutputDataConfig.S3OutputPath, and report that object's URI in DescribeTrainingJob.ModelArtifacts.S3ModelArtifacts — with FailureReason left absent. FailureReason must never carry text describing the emulator.
  Traces:   s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive ("training job ended in 'Failed': This emulator has no container runtime ..."); s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative (its positive control asserts TrainingJobStatus==Completed and got 'Failed' with the same FailureReason). Open issues 20261029T203709Z-sagemaker-an-accepted-createtrainingjob-is-never-executed-the.md, 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md and 20261030T171442Z-sagemaker-server-side-job-execution-is-absent-training.md describe the same family, but the replay report did not attribute these two checks to any of them.

Cause 2 of 2: CreateTrainingJob accepts a RoleArn whose trust policy does not allow sagemaker.amazonaws.com.
Failing check id: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative.
  AWS:      CreateTrainingJob validates, synchronously and before returning, that the supplied RoleArn can be assumed by the SageMaker service principal. A role whose AssumeRolePolicyDocument has no statement allowing sts:AssumeRole for Principal.Service sagemaker.amazonaws.com is refused at create time with a 400 ValidationException; no training job resource is created. Smithy: CreateTrainingJob.RoleArn is a required member (target RoleArn, @length 20..2048, @pattern ^arn:aws[a-z\-]*:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) and the operation's service-common ValidationException carries the refusal; the model does not state the message text — recorded behavior only: the recording shows the call refused synchronously rather than accepted. Hypothesis (not stated by the model or the recorded body): the message's fixed text names the inaccessible role and the required trust relationship, with the role ARN taken verbatim from the request's RoleArn; whatever the fixed text, the requirement is the 400 ValidationException and that the role ARN in the message comes from the request.
  Emulator: the call is accepted and returns 200 with a TrainingJobArn; the replay reports "CreateTrainingJob with a role that does not trust sagemaker.amazonaws.com was expected to be rejected synchronously, but it was accepted". No role-assumability check runs at all.
  Why:      A client (or a CloudFormation/IaC template) that mis-wires its execution role's trust policy gets a success from the emulator and only discovers the misconfiguration in real AWS. Negative tests that assert the guardrail pass vacuously, and permission-boundary scenarios cannot be exercised at all, so the emulator is systematically more permissive than AWS on the single most common SageMaker setup error.
  Fix:      The CreateTrainingJob handler's input validation (the execution-role resolution step), which must look the RoleArn up in the IAM state, inspect its trust policy for sts:AssumeRole granted to Principal.Service sagemaker.amazonaws.com, and refuse with ValidationException naming the request's role ARN when it is absent — before any training-job state is created.
  Traces:   s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative. Open issue 20261029T192819Z-sagemaker-create-operations-skip-the-execution-role-assumability.md describes a similar family for other create operations but the report did not attribute this check to it.

## Evidence
Cause 1 — expected vs actual:
  Expected: DescribeTrainingJob.TrainingJobStatus == "Completed", FailureReason absent, ModelArtifacts.S3ModelArtifacts naming a model.tar.gz readable under OutputDataConfig.S3OutputPath.
  Actual:   TrainingJobStatus == "Failed"; FailureReason == "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location"; no object under the output prefix.
  Replay detail (s3__sagemaker.cfn-deploy-sagemaker-training-job-stack-positive): AssertionError: training job ended in 'Failed': This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location
  Replay detail (s3__sagemaker.cfn-deploy-sagemaker-role-missing-s3-trust-negative): AssertionError: expected Completed, got 'Failed' (FailureReason="This emulator has no container runtime, ...")

Cause 2 — expected vs actual:
  Expected: CreateTrainingJob with a RoleArn lacking a sagemaker.amazonaws.com trust statement -> HTTP 400, Error.Code "ValidationException", message naming the request's role ARN and the missing trust relationship; DescribeTrainingJob for that name then not found.
  Actual:   HTTP 200 with a TrainingJobArn; the job exists.
  Replay detail: AssertionError: CreateTrainingJob with a role that does not trust sagemaker.amazonaws.com was expected to be rejected synchronously, but it was accepted

## Steps to Reproduce
Cause 1:
1. Create an S3 bucket, upload a CSV training input, and create an IAM role trusting sagemaker.amazonaws.com with read/write on that bucket.
2. Call CreateTrainingJob with the region's built-in XGBoost training image, an S3DataSource input channel pointing at the uploaded object, and OutputDataConfig.S3OutputPath in the bucket (equivalently, deploy an AWS::SageMaker::TrainingJob-shaped CloudFormation stack that does the same).
3. Poll DescribeTrainingJob to a terminal status. Observe TrainingJobStatus="Failed" with the no-container-runtime FailureReason instead of "Completed", and ListObjectsV2 on the output prefix returning no model.tar.gz.

Cause 2:
1. Create an IAM role whose AssumeRolePolicyDocument trusts some other principal (e.g. ec2.amazonaws.com) and NOT sagemaker.amazonaws.com.
2. Call CreateTrainingJob passing that role as RoleArn, with an otherwise valid AlgorithmSpecification, InputDataConfig and OutputDataConfig.
3. Observe a 200 response with a TrainingJobArn, instead of a 400 ValidationException; DescribeTrainingJob confirms the job was created.
