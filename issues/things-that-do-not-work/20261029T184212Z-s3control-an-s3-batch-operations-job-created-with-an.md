Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected
Issue-Key: emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# An S3 Batch Operations job created with an unassumable role never settles: DescribeJob reports no terminal status and no FailureReason, so the trust-mismatch refusal cannot be observed

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJob

## Description
1 net-new root cause in the S3 Control (S3 Batch Operations) crate, grounded in the cross-service check emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected (the failing calls are s3control CreateJob/DescribeJob, so the service is attributed to `s3control`). The related open issue 20261025T000830Z-s3control-an-s3-batch-operations-s3putobjecttagging-job-never-reaches.md describes the same family for the sibling replay key emr__glue__s3.iam-role-trust-s3-service-assumed-for-batch-op-positive (which the suite marks as owned by it); it does not carry this check id, which exercises the REFUSAL leg.

--- Cause 1 of 1: a batch job neither completes nor fails ---
AWS: recorded behavior only: a Job created by s3control CreateJob with ConfirmationRequired=false runs and reaches a terminal `Status` — `Complete` when its role is assumable and permitted (and the S3PutObjectTagging operation has actually tagged the manifest's target object), or `Failed` with a populated `FailureReasons` entry when S3 cannot assume the role (a role trusting ec2.amazonaws.com instead of batchoperations.s3.amazonaws.com), leaving the target object untagged. The two jobs differ only in the trust principal, so AWS's answer isolates the refusal to the trust mismatch.
Emulator: neither job settles. The check polls DescribeJob for a status in {Complete, Failed, Cancelled} for both the correctly-trusted and the wrongly-trusted job and is killed by the harness with `Check timed out after 60.0s`, never reaching its assertions — so the emulator leaves the job in a non-terminal status indefinitely, publishes no FailureReason and never tags the target object. Hypothesis: CreateJob records the job but no batch-operations runner advances or fails it, and role assumability is never evaluated.
Why: a client submitting a batch job waits forever for a terminal status; a misconfigured role (the single most common S3 Batch Operations failure) is never reported as Failed with a reason, and a correctly configured job never applies its operation, so the manifest's objects are silently left untouched.
Fix: the S3 Control crate's batch-job execution path and state machine (services/service-com-amazonaws-s3control-awss3controlservicev20180820 — the CreateJob handler and the job state that DescribeJob projects): drive the job to a terminal Status, refuse it as Failed with a FailureReason when the job role cannot be assumed by batchoperations.s3.amazonaws.com, and actually apply S3PutObjectTagging to the manifest's objects when it can.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (group emr__glue__s3: 1/46 checks passing). emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected reports exactly one detail: `Error: Check timed out after 60.0s`.

The check creates one bucket, one target object and two IAM roles with identical inline permissions (s3:GetObject, s3:PutObject on `reports/*`, s3:PutObjectTagging) differing only in the trust principal — `batchoperations.s3.amazonaws.com` (good) vs `ec2.amazonaws.com` (bad) — then runs `_run_job_until_settled(...)` for each, which polls DescribeJob until Status is in {Complete, Failed, Cancelled}. Expected: the good job Status=Complete with the target object carrying the tag {Key: sim2real, Value: tagged}; the bad job Status=Failed with a FailureReason and the target object untagged. Actual: neither job ever reports a terminal Status, so the polling loop ran until the 60 s harness ceiling and no assertion was reached (no error was raised by any API call — the emulator answers DescribeJob, it just never changes the status).

## Steps to Reproduce
Boot the emulator and replay emr__glue__s3.s3-batch-operation-role-missing-s3-trust-rejected from suite simparity-behavioral-aws-v1:
1. Create a bucket and put one target object in it; create two IAM roles with the same inline policy (s3:GetObject on the bucket, s3:PutObject on `reports/*`, s3:PutObjectTagging) but trust principals batchoperations.s3.amazonaws.com and ec2.amazonaws.com respectively.
2. Write a CSV manifest naming the target object, then s3control CreateJob (Operation=S3PutObjectTagging, ConfirmationRequired=false, Report into `reports/`) once per role.
3. Poll s3control DescribeJob for each job until Status is Complete/Failed/Cancelled. Expect the good-trust job Complete with the target object tagged sim2real=tagged, and the bad-trust job Failed with a FailureReason and the object untagged; the emulator returns a non-terminal Status indefinitely.
