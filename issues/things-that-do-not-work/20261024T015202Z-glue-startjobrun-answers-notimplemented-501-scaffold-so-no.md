Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive
Issue-Key: emr__glue__s3|StartJobRun|NotImplemented
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Glue StartJobRun answers NotImplemented (501 scaffold), so no Glue job run reached through the EMR/Glue/S3 cross-service checks can ever start

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: Glue's StartJobRun is still an unimplemented scaffold, and it is reached only after the emulator has successfully served the whole set-up — CreateBucket/PutObject, IAM CreateRole/AttachRolePolicy, Glue CreateConnection and Glue CreateJob all return 200 — so the cross-service checks get all the way to the data-plane call and then die on the stub.
AWS: Smithy com.amazonaws.glue#StartJobRun accepts JobName (+ Arguments/WorkerType/NumberOfWorkers/Timeout) and returns a JobRunId with HTTP 200; its modeled errors are the client/throttling/concurrent-run family (InvalidInputException, EntityNotFoundException, ResourceNumberLimitExceededException, ConcurrentRunsExceededException, InternalServiceException, OperationTimeoutException). NotImplemented is not among them, and a run started this way subsequently becomes observable through GetJobRun.
Emulator: returns error code NotImplemented with message "operation StartJobRun is scaffolded but not implemented in this emulator" for a job that CreateJob accepted moments earlier.
Why: no client can start a Glue job run at all — the whole job-execution surface (run state machine, GetJobRun polling, job bookmarks, the IAM role actually being assumed and its S3 side effects, connection associations surviving a run) is unreachable, and a caller catching Glue's modeled exceptions does not catch NotImplemented, so it surfaces as an opaque ClientError rather than a handled job failure.
Fix: implement the operation in its handler, services/service-com-amazonaws-glue-awsglue/src/ops/start_job_run.rs, and the job-run state it must create for GetJobRun/GetJobBookmark to read.
Traces: emr__glue__s3.glue-connection-uses-role-with-glue-trust-positive, emr__glue__s3.glue-job-bookmark-persists-processed-s3-keys-positive. The replay report did NOT mark either check as already filed or owned. The open issue 20261023T123230Z-glue-startjobrun-is-an-unimplemented-501-scaffold-and.md names the same operation but does not carry these two checks' replay keys, so this issue records them; whoever fixes that one should re-replay the two check ids above as well.

Service attribution note: the leading token of these check ids is the composite 'emr__glue__s3' (the checks declare SERVICES = ['emr','glue','s3']), which is not a single service token. The failing operation is Glue's StartJobRun and the stub lives in the Glue crate, so this issue is filed under service 'glue'.

## Evidence
Expected: glue.start_job_run(JobName=<job created moments earlier>) returns HTTP 200 with a JobRunId, and glue.get_job_run(JobName=..., RunId=...) then reports the run progressing to a terminal state (SUCCEEDED for these two checks' scripts).
Actual, both checks: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartJobRun operation: operation StartJobRun is scaffolded but not implemented in this emulator.
The calls preceding it in the same checks all succeeded against the emulator: s3.create_bucket/put_object, iam.create_role (+ managed/inline policy), glue.create_connection (JDBC) and glue.create_job (pythonshell with MaxCapacity=0.0625, and glueetl with GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2).

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. s3.create_bucket(Bucket=b); s3.put_object(Bucket=b, Key='scripts/job.py', Body=<any script>).
3. iam.create_role(RoleName=r, AssumeRolePolicyDocument=<trust for glue.amazonaws.com>) and attach a policy granting s3 access to that bucket.
4. glue.create_job(Name=j, Role=<role arn>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<b>/scripts/job.py'}, MaxCapacity=0.0625, Timeout=5) -> succeeds.
5. glue.start_job_run(JobName=j) -> expect HTTP 200 with a JobRunId; observe 'NotImplemented: operation StartJobRun is scaffolded but not implemented in this emulator'.
6. Re-replay the check ids in trace_ids.
