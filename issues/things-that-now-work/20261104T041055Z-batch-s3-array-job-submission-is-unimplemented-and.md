Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__s3.batch-array-24-shards-write-own-object-and-out-of-scope-put-denied batch__s3.batch-job-s3-write-outside-job-role-scope-is-denied batch__s3.batch-job-writes-s3-object-read-back-byte-exact
Issue-Key: batch__s3.batch-job-s3-write-outside-job-role-scope-is-denied|_poll_until#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: batch__s3.batch-job-writes-s3-object-read-back-byte-exact|_poll_until#14|Timeout|return last
Issue-Related: 20261104T041055Z-batch-array-jobs-unimplemented-managed-ces-have-no.md
Issue-Commit: c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09

# batch__s3: array-job submission is unimplemented and jobs never settle, so a Batch job never writes an S3 object and the job-role scope is never exercised (2 causes)

> _Note: this issue was identified against the code base at commit `c2d690adb5c99cf0fe6b339d9b4e7b6ccf196d09`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SubmitJob

## Description
Two net-new behavioral root causes grounded in failing checks of the simparity-behavioral-aws-v1 replay (batch__s3 0/3). Both must be closed before submitting.

Cause 1 of 2 — SubmitJob with arrayProperties is refused NotImplemented, so array shards never run.
AWS: Smithy com.amazonaws.batch#SubmitJob, POST /v1/submitjob, success 200; the optional member `arrayProperties` targets ArrayProperties{size:Integer}; modeled errors are only ClientException (400) and ServerException (500) — NotImplemented is not a Batch error. AWS answers 200 {jobArn,jobName,jobId} and creates `size` children, each carrying ArrayPropertiesDetail.index, so each shard can write its own S3 object keyed by its index.
Emulator: the submission is refused — ClientError (NotImplemented) "operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator".
Why: no sharded array workload can be submitted, so neither the per-shard object writes nor the out-of-scope PutObject denial they are paired with can be observed; and the client sees an error class AWS never returns for a modeled member.
Fix: the arrayProperties branch of the SubmitJob handler in the batch crate (child-job creation and per-child index).
Traces: batch__s3.batch-array-24-shards-write-own-object-and-out-of-scope-put-denied.

Cause 2 of 2 — a submitted job never reaches a terminal status, so its container never writes to S3 and its job role is never enforced.
AWS: after SubmitJob returns 200 Batch runs the container server-side; the job reaches SUCCEEDED or FAILED, a PutObject the container makes with the job role's credentials lands in the bucket byte-for-byte, and a PutObject outside the job role's scope is denied by S3 (AccessDenied) while the job itself still reaches a terminal status.
Emulator: the job never settles — one check reports TimeoutError "batch job <job-id> status in ['FAILED', 'SUCCEEDED']: the run budget ended first; not graded" and the other exhausts the harness budget with no terminal status and no object written. (Same defect family as the "submitted jobs never reach a terminal status" cause filed for service heading `batch` in this sweep; filed here because these checks are attributed to batch__s3 and carry their own replay keys.)
Why: the data path a Batch job exists for — run a container that reads/writes S3 under a job role — produces nothing: no object, no outcome, and no authorization decision, so a client cannot tell a denied write from work that never started.
Fix: the job scheduler/state machine in the batch crate (place and run the job, record its attempt and terminal status) so the container's S3 calls are made under the job role.
Traces: batch__s3.batch-job-s3-write-outside-job-role-scope-is-denied, batch__s3.batch-job-writes-s3-object-read-back-byte-exact.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against this checkout; batch__s3 0/3 passed.

Cause 1: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the SubmitJob operation: operation SubmitJob with arrayProperties is scaffolded but not implemented in this emulator.

Cause 2: TimeoutError: batch job c3e32b61-4cb6-4171-b2d8-893a8776712e status in ['FAILED', 'SUCCEEDED']: the run budget ended first; not graded (batch-job-s3-write-outside-job-role-scope-is-denied), and Error: Check timed out after 60.0s with no object written (batch-job-writes-s3-object-read-back-byte-exact).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: aws batch submit-job --job-name j --job-queue <enabled queue> --job-definition <jd> --array-properties size=24 -> NotImplemented; AWS answers 200 and creates 24 children.
3. Cause 2: register a FARGATE job definition whose container writes an object to an S3 bucket using a jobRoleArn scoped to one prefix, submit the job, then poll aws batch describe-jobs --jobs <jobId> and aws s3api head-object -> the job never settles and the object never appears.
4. Re-replay the check ids in trace_ids.
