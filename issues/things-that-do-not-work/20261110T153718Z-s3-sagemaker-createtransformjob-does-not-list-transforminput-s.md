Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive
Issue-Key: s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive|_call_with_role_propagation_retry#9|Timeout|time.sleep(interval_s)
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# s3__sagemaker: CreateTransformJob does not list TransformInput's S3 prefix synchronously — neither the empty-prefix refusal nor the accepted populated-prefix job is reached

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransformJob

## Description
One net-new root cause in the s3__sagemaker suite. It is NOT covered by the open s3__sagemaker issues (20261108T162008Z-s3-sagemaker-8-net-new-causes-sagemaker-job.md, 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md, 20261024T160943Z, 20261029T203709Z, 20261109T020211Z, 20261108T214320Z), which carry other checks' replay keys; this check carries none.

Cause 1 of 1: CreateTransformJob neither refuses an empty TransformInput prefix at submit time nor accepts a populated one into a non-Failed job within the caller's reach.
AWS: recorded behavior only for the synchronous listing; the operation is modelled. CreateTransformJob lists the S3 prefix named by TransformInput.DataSource.S3DataSource.S3Uri synchronously, before the job is created: when the prefix holds no objects the call is refused with a validation error naming that there are no objects to transform, and DescribeTransformJob/ListTransformJobs show NO job of that name afterwards (the refusal is pre-creation, so it leaves nothing behind). The identical call against a prefix that holds objects is accepted and the created job is not in status Failed.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — neither leg returns inside the run budget, so neither the refusal's status/code/message nor the accepted job's status was ever compared.
Why: a caller submitting a transform job against a mistyped or empty prefix gets no synchronous refusal, so the error surfaces (if at all) much later and asynchronously, and a client cannot tell whether a job name was consumed by a rejected submit. The accepted path cannot be confirmed either, so batch transform is unusable in both directions.
Fix: the CreateTransformJob handler's input validation — it must list TransformInput.DataSource.S3DataSource.S3Uri before creating the job, refuse an empty prefix with the no-objects validation error and create nothing, and otherwise create a job that is not immediately Failed.
Traces: s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive.

## Evidence
Check s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "CreateTransformJob lists TransformInput's S3 prefix synchronously: an empty prefix is rejected with a no-objects error and no job exists, while the same call against a populated prefix is accepted and not Failed." The check uploads model/artifact.tar.gz and input_populated/records.csv (b"1\n2\n3\n") and leaves a second prefix empty.

Expected: CreateTransformJob against the empty prefix is refused synchronously with a no-objects validation error and no job of that name exists; against input_populated/ it is accepted and the job is not Failed. Actual: neither call returns inside the run budget, so no status, code or message was compared. Bucket names, job names and role ARNs are per-run values.

## Steps to Reproduce
1. Create a bucket; upload a model artifact tarball and one object under input_populated/; leave a second prefix with no objects.
2. Create an execution role SageMaker can assume with read access to the bucket, and CreateModel referencing the artifact.
3. CreateTransformJob with TransformInput.DataSource.S3DataSource.S3Uri pointing at the EMPTY prefix: AWS refuses synchronously with a no-objects validation error, and DescribeTransformJob for that name reports it does not exist.
4. CreateTransformJob with the S3Uri pointing at input_populated/: AWS accepts it and the job is not Failed.
5. The emulator answers neither call. Re-replay s3__sagemaker.sagemaker-batch-transform-reads-s3-input-positive to confirm the fix.
