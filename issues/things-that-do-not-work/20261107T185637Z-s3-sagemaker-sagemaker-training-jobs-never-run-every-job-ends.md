Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative
Issue-Key: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative|run#20|AssertionError|assert good_desc["TrainingJobStatus"] == "Completed", (
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# SageMaker training jobs never run: every job ends Failed with the emulator's own 'no container runtime' reason, so no IAM negative is measurable

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrainingJob

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issues 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md, 20261025T101032Z-sagemaker-training-jobs-never-execute-every-accepted-createtrainingjob.md and 20261103T053228Z-s3-sagemaker-a-training-job-whose-role-does.md own OTHER checks of this service and do not carry this replay key.

Cause 1 of 1: a training job whose role IS trusted by sagemaker.amazonaws.com still ends Failed, so the untrusted-role refusal it calibrates cannot be observed.
AWS: CreateTrainingJob with a role trusted by sagemaker.amazonaws.com and granted the input/output S3 permissions runs the algorithm container and reaches TrainingJobStatus 'Completed', writing model.tar.gz under OutputDataConfig.S3OutputPath. A role whose trust policy omits sagemaker.amazonaws.com is instead refused — ValidationException at CreateTrainingJob, or the job reaching 'Failed' with a FailureReason attributing the failure to the role's trust relationship / AccessDenied on AssumeRole. The contrast between the two is the behaviour under test.
Emulator: the trusted-role positive control ends Failed with FailureReason "This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location" — an emulator-authored sentence returned identically for every job. No model artifact is written, and because the control fails, the untrusted-role case cannot be distinguished from it.
Why: no training can be performed, and the FailureReason field — the only diagnostic a client has — is a constant that attributes nothing, so a genuine IAM/trust defect and an unexecuted job look the same.
Fix: the SageMaker training-job executor (the component that assumes the job's RoleArn, runs the algorithm image against the input channels and writes model artifacts to S3), and the FailureReason it sets, which must name the real cause — including the role-trust refusal.
Traces: s3__sagemaker.iam-role-missing-sagemaker-trust-blocks-training-job-negative.

## Evidence
Expected: the trusted-role control job reaches TrainingJobStatus 'Completed' with a model artifact at S3OutputPath, and the untrusted-role job is refused with a FailureReason naming the trust relationship. Actual: 'positive control (trusted role, same permissions) failed to complete: This emulator has no container runtime, so the training job's algorithm image was never started and no model artifacts were written to the output S3 location'.

## Steps to Reproduce
1) Upload training input to S3; create a role trusted by sagemaker.amazonaws.com with read/write on the input and output prefixes.
2) CreateTrainingJob with that role; poll DescribeTrainingJob for 'Completed' and list S3OutputPath.
3) Repeat with a role whose trust policy omits sagemaker.amazonaws.com and read the FailureReason.
