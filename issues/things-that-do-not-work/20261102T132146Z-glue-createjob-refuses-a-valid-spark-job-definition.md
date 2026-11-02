Issue-Source: trace-sweep-divergence
Issue-Trace: glue-gjwr-traj-create-job-stores-unvalidated-references
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-gjwr-traj-create-job-stores-unvalidated-references glue-gjwr-traj-start-the-copied-job-fails-before-provisioning
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|CreateJob|2xx->4xx|$.Name
Issue-Key: glue|CreateJob|2xx->4xx|status
Issue-Key: glue|GetJob|2xx->4xx|$.Job
Issue-Key: glue|GetJob|2xx->4xx|status
Issue-Key: glue|GetJobRun|2xx->4xx|$.JobRun
Issue-Key: glue|GetJobRun|2xx->4xx|status
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue CreateJob refuses a valid Spark job definition, blocking the job and its runs

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
A CreateJob request that AWS accepts is refused 400 by the emulator, and the reads that depend on the created job and its run are refused in turn. Three causes, one root cause: CreateJob's validation rejects a job definition whose shape AWS allows, so nothing downstream exists.

Cause 1 of 3: CreateJob refuses a valid Spark job definition. AWS: Smithy CreateJob requires Name, Role and Command and the recording shows a request carrying Command {Name 'spark', ScriptLocation s3://...}, Role an IAM role ARN, WorkerType G.1X with NumberOfWorkers 2, AllocatedCapacity 2, MaxCapacity 2.0, GlueVersion '4.0', JobMode SCRIPT — answered 200 with the job's Name echoed in CreateJobResponse.Name. Note AWS does NOT validate the referenced role or script location at create time (the fixture is named for exactly that: unvalidated references are stored). Emulator: 400, with no Name member. Why: a client cannot create a job at all through this shape, so the whole job surface is unreachable; and the refusal implies the role/script reference was rejected, which AWS defers to run time. Fix: CreateJob's validation must accept this definition and store the references unvalidated. Traces: glue-gjwr-traj-create-job-stores-unvalidated-references#task#4.

Cause 2 of 3: the GetJob that reads the job back is refused. AWS: Smithy GetJob returns 200 with the full Job (Command including ScriptLocation, ExecutionProperty.MaxConcurrentRuns, GlueVersion, JobMode, JobRunQueuingEnabled, MaxCapacity, MaxRetries, Timeout, WorkerType, NumberOfWorkers, AllocatedCapacity, Role, CreatedOn/LastModifiedOn). Emulator: 400 with no Job member. Why: this is the downstream consequence of cause 1 — it must be re-replayed after the create is fixed to confirm the stored Job is rendered completely, not just that the create now succeeds. Fix: GetJob's state read, once CreateJob persists the job. Traces: glue-gjwr-traj-create-job-stores-unvalidated-references#task#5.

Cause 3 of 3: GetJobRun for a run of a job created the same way is refused. AWS: Smithy GetJobRun returns 200 with the JobRun (Id, JobName, JobRunState, StartedOn/CompletedOn/LastModifiedOn, Attempt, ExecutionTime, Timeout, GlueVersion, JobMode, MaxCapacity, WorkerType, NumberOfWorkers, AllocatedCapacity, ExecutionArguments, LogGroupName, PredecessorRuns, ErrorMessage). Emulator: 400 with no JobRun member. Why: the run record is unreachable, so no client can poll a run to completion. Fix: GetJobRun's state read, once the job and its run exist. Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#8.

The ErrorMessage content of those runs (the emulator authors its own 'records job runs but executes no job script' sentence instead of Glue's role-trust failure text) is a separate root cause already owned by open issue 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md and is not re-filed here.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue.

Cause 1 — glue-gjwr-traj-create-job-stores-unvalidated-references#task#4 CreateJob: status expected 200, got 400; $.Name missing (expected 'sim2real-probe-gjwr-spark-d8751718').
Cause 2 — glue-gjwr-traj-create-job-stores-unvalidated-references#task#5 GetJob: status expected 200, got 400; $.Job missing (expected the full Job with Command{Name 'spark', ScriptLocation s3://...}, ExecutionProperty{MaxConcurrentRuns 1}, GlueVersion '4.0', JobMode 'SCRIPT', JobRunQueuingEnabled False, MaxCapacity 2.0, MaxRetries 0, Timeout 2880, WorkerType 'G.1X', NumberOfWorkers 2, AllocatedCapacity 2, Role an IAM role ARN).
Cause 3 — glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#8 GetJobRun: status expected 200, got 400; $.JobRun missing (expected the full JobRun including JobRunState 'FAILED', LogGroupName '/aws-glue/jobs', PredecessorRuns [], ExecutionArguments {}, Timeout 5, WorkerType 'G.1X').

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-gjwr-traj-create-job-stores-unvalidated-references#task#4 — CreateJob with Command{Name 'spark'}, a role ARN, WorkerType G.1X/NumberOfWorkers 2 and GlueVersion '4.0'; the emulator answers 400 instead of 200 with Name.
2. glue-gjwr-traj-create-job-stores-unvalidated-references#task#5 — GetJob for that job; 400 instead of 200 with Job.
3. glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#8 — GetJobRun for a run of such a job; 400 instead of 200 with JobRun.
