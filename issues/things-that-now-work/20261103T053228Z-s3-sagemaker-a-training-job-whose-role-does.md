Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative
Issue-Key: s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative|run#20|AssertionError|assert status != "Failed", f"positive-control job unexpectedly failed early: {status}"
Issue-Commit: 7e282e985d147868e376415a13aa29a1f3675b80

# s3__sagemaker: a training job whose role DOES trust sagemaker.amazonaws.com settles Failed immediately, so the trust-policy negative check's positive control never starts (1 cause)

> _Note: this issue was identified against the code base at commit `7e282e985d147868e376415a13aa29a1f3675b80`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
Cause 1 of 1: CreateTrainingJob accepted with a correctly-trusted execution role is never executed — the job is parked in a terminal Failed state right after creation instead of progressing InProgress -> Completed.

AWS: recorded behavior only (Smithy sagemaker#CreateTrainingJob models only ResourceInUse / ResourceLimitExceeded, and DescribeTrainingJob.TrainingJobStatus is the enum InProgress|Completed|Failed|Stopping|Stopped). A CreateTrainingJob whose RoleArn names a role whose trust policy allows sts:AssumeRole for the service principal sagemaker.amazonaws.com is accepted with 200 and the job reports TrainingJobStatus 'InProgress' while the container runs, then a terminal 'Completed' with no FailureReason; FailureReason is populated only when the job actually failed. The same check's negative half — a role trusted only by ec2.amazonaws.com — is refused correctly by the emulator today (400 ValidationException, message 'Could not assume role <RoleArn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.' where the fixed text is everything but <RoleArn>, which is the request's RoleArn), so the divergence is purely on the accepted/positive path.

Emulator: the positive-control job is accepted, and the very first DescribeTrainingJob inside the check's 90 s wait already reports TrainingJobStatus 'Failed' (replay: AssertionError "positive-control job unexpectedly failed early: Failed" at the assertion `status != "Failed"`). Nothing in the request is invalid: the role trusts sagemaker.amazonaws.com, the S3 input prefix exists and the output prefix is writable by the role's inline policy. The job therefore never leaves the terminal Failed state and never writes a model artifact under the declared S3OutputPath.

Why: a client that waits for a job to leave InProgress and asserts a non-failed outcome (every SDK estimator .fit(), every CI pipeline gate, and every negative test that needs a positive control to prove the refusal was specific to the bad role) observes an unconditional failure. Worse, because the failure is unconditional, a caller cannot distinguish a genuinely unusable role from a usable one: the trust-policy contract is unobservable in either direction.

Fix: the CreateTrainingJob handler and the training-job status/state machine it feeds (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/create_training_job.rs and the training-job state it writes) — an accepted job must enter InProgress and reach a terminal state that reflects the actual outcome, with FailureReason set only on real failure, rather than being settled Failed at creation.

Note: several open issues describe the same family of 'jobs never execute' failures (20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md, 20261102T121031Z-s3-sagemaker-accepted-createtrainingjob-never-executes-failed-with.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md). None of them carries this check's replay key — the suite reported this check as unowned — so it is filed here; a fixer closing those should re-replay the check id below as well.

Traces: s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative (its negative half passes; it fails only on the positive control).

## Evidence
Replay of check s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative:

  File "check_s3__sagemaker_sagemaker_execution_role_trust_policy_missing_negative.py", line 114, in run
    assert status != "Failed", f"positive-control job unexpectedly failed early: {status}"
  AssertionError: positive-control job unexpectedly failed early: Failed

Expected (AWS): after CreateTrainingJob with RoleArn naming a role whose trust policy allows sts:AssumeRole for sagemaker.amazonaws.com, DescribeTrainingJob reports TrainingJobStatus 'InProgress' (and later 'Completed'), with no FailureReason.
Actual (emulator): DescribeTrainingJob reports TrainingJobStatus 'Failed' within the first poll of a 90 s window.

The preceding assertions in the same check pass, proving the emulator already returns the correct refusal for the untrusted role (400 ValidationException / "Could not assume role <role arn>. Please ensure that the role exists and allows principal 'sagemaker.amazonaws.com' to assume the role.") and correctly reports the rejected job as absent (400 ValidationException / "Requested resource not found."). Only the accepted job's lifecycle diverges.

## Steps to Reproduce
1. Create an S3 bucket and upload a CSV training object under train/.
2. Create an IAM role whose trust policy allows sts:AssumeRole for the service principal sagemaker.amazonaws.com, with s3:GetObject/s3:ListBucket on the bucket and s3:PutObject on <bucket>/output/*.
3. Call CreateTrainingJob with that RoleArn, AlgorithmSpecification.TrainingImage set to the region's built-in sagemaker-xgboost image, one S3Prefix input channel s3://<bucket>/train/, OutputDataConfig.S3OutputPath s3://<bucket>/output/, ResourceConfig ml.m5.large x1 and StoppingCondition.MaxRuntimeInSeconds 900.
4. Poll DescribeTrainingJob.
Observed: TrainingJobStatus is 'Failed' on the first poll. Expected: 'InProgress', then a terminal status reflecting the real outcome.
Equivalently: run behavioral check s3__sagemaker.sagemaker-execution-role-trust-policy-missing-negative.
