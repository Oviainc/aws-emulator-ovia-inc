Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__s3.batch-array-24-shards-write-own-object-and-out-of-scope-put-denied batch__s3.batch-job-s3-write-outside-job-role-scope-is-denied batch__s3.batch-job-writes-s3-object-read-back-byte-exact
Issue-Key: batch__s3.batch-job-s3-write-outside-job-role-scope-is-denied|run#17|AssertionError|assert control["status"] == "SUCCEEDED", (
Issue-Key: batch__s3.batch-job-writes-s3-object-read-back-byte-exact|run#14|AssertionError|assert job["status"] == "SUCCEEDED", (
Issue-Key: batch__s3.batch-array-24-shards-write-own-object-and-out-of-scope-put-denied|run#5|AssertionError|assert summary.get("SUCCEEDED") == _ARRAY_SIZE and summary.get("FAILED", 0) == 0, (
Issue-Shape-Id: com.amazonaws.batch#AWSBatchV20160810
Issue-Related: 20261104T041055Z-batch-s3-array-job-submission-is-unimplemented-and.md
Issue-Commit: 83a0ba9f0336a789b943448648f5b4ab74a7cd4a

# batch__s3: a placed Batch job's container is never RUN, so a job reaches FAILED instead of SUCCEEDED and writes no S3 object (1 cause, platform-blocked)

> _Note: this is the residual cause of
> `issues/things-that-now-work/20261104T041055Z-batch-s3-array-job-submission-is-unimplemented-and.md`.
> That issue's two other causes are closed: `SubmitJob` now creates array children with
> their indexes, and a submitted job is now placed from the compute environments of its
> queue and reaches a terminal status. What is left is the container itself._

## Operation
SubmitJob

## Description
Cause 1 of 1 — the container of a placed job is never run, so no job can SUCCEED and no
object a job would write ever appears.

AWS: after `SubmitJob` returns 200 Batch runs the job's container image server-side; the
job reaches `SUCCEEDED` (or `FAILED` with the container's own exit code), a `PutObject` the
container makes with the job role's credentials lands in the bucket byte-for-byte, and a
`PutObject` outside the job role's scope is denied by S3 (`AccessDenied`) while the job
still reaches a terminal status. For an array job each child writes its own object, keyed
by its `ArrayPropertiesDetail.index`.

Emulator: the job is placed and settles, but in the modelled failure state with
`statusReason` "This emulator does not run containers, so the job's container was never
started" (`state::NO_CONTAINER_RUNTIME_REASON`), and nothing is written to S3. Nothing is
fabricated for the attempt: no `startedAt`, `stoppedAt`, `attempts` entry or `exitCode`.

Why: the data path a Batch job exists for — run a container that reads and writes S3 under
a job role — produces no object, no exit code and no authorization decision, so a client
cannot tell a denied write from work that never started.

## Platform blocker
This cause needs a CONTAINER RUNTIME, which a generated emulator does not have:
`vendor/engine/src/lib.rs` lines 13-15 state "Containers and external processes are not
available to a generated emulator yet". Nor is there a faithful in-process substitute: the
work is a container image, not a language the emulator can evaluate, and serving the
recorded jobs by matching on their command text and hand-writing a subset of the tool they
invoke is what the same contract forbids. Closing this cause therefore requires either that
capability in the core, or `mark_wont_fix(platform_blocker)` citing
`vendor/engine/src/lib.rs`.

## Evidence
Behavioral replay (simparity-behavioral-aws-v1) against the checkout at the commit above:

* `batch-job-writes-s3-object-read-back-byte-exact`: AssertionError "the copying job must
  SUCCEED, got 'FAILED' statusReason=\"This emulator does not run containers, so the job's
  container was never started\"".
* `batch-job-s3-write-outside-job-role-scope-is-denied`: AssertionError "the in-scope
  control write (dst/) must SUCCEED, got 'FAILED' ...".
* `batch-array-24-shards-write-own-object-and-out-of-scope-put-denied`: AssertionError "all
  24 array children must SUCCEED; statusSummary={'FAILED': 24, ...}" — the array is now
  created and summarised, and only the children's own runs are missing.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Register a FARGATE job definition whose container writes an object to an S3 bucket using
   a `jobRoleArn` scoped to one prefix, create a MANAGED, ENABLED compute environment and an
   ENABLED queue over it, and submit the job.
3. Poll `aws batch describe-jobs --jobs <jobId>`: the job reaches `FAILED` with the
   no-container-runtime `statusReason`, and `aws s3api head-object` finds no object.
4. Re-replay the check ids in trace_ids.
