Issue-Source: trace-sweep-divergence
Issue-Trace: glue-glue-job-command-and-defaults-round-trip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-glue-job-command-and-defaults-round-trip glue-gtr-job-role-forms-echoed-verbatim glue-gjwr-traj-region-sweep-locates-the-job glue-gtr-job-name-case-and-batch-lookup glue-gjwr-traj-start-the-copied-job-fails-before-provisioning
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetJob|2xx->2xx|$.Job.Command.PythonVersion
Issue-Key: glue|BatchGetJobs|2xx->2xx|$.Jobs[*].JobRunQueuingEnabled
Issue-Key: glue|GetJobRun|2xx->2xx|$.JobRun.ErrorMessage
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].ErrorMessage
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].ExecutionArguments
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].JobRunQueuingEnabled
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].LogGroupName
Issue-Key: glue|GetJobRuns|2xx->2xx|$.JobRuns[*].PredecessorRuns
Issue-Related: 20261029T201859Z-glue-job-job-run-session-and-connection-reads.md
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue job and job-run reads omit modeled default and empty-collection members

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
Glue job and job-run reads elide members whose value is a service-applied default or an empty collection, so a client that reads back a job it just created cannot see the defaults Glue applied. Four causes, one root cause: the Job/JobRun serializers skip members instead of emitting their defaulted values.

Cause 1 of 4: GetJob omits Job.Command.PythonVersion and Job.JobRunQueuingEnabled. AWS: Smithy JobCommand.PythonVersion (@pattern ^([2-3]|3[.]9)$) is defaulted by Glue to '3' for a Spark/Python command, and CreateJob.JobRunQueuingEnabled targets NullableBoolean which Glue reports as false when not enabled; the recording shows PythonVersion '3' and JobRunQueuingEnabled false on the read-back. Emulator: both members absent. Why: a client that round-trips a job definition (read, modify, UpdateJob) loses the Python version and cannot tell whether run queuing is on, and `null` is not `false` to a strongly typed SDK. Fix: the GetJob output serializer / job state defaults. Traces: glue-glue-job-command-and-defaults-round-trip#task#3 (cluster also covers glue-gtr-job-role-forms-echoed-verbatim#task#3, #task#5, #task#7).

Cause 2 of 4: BatchGetJobs omits Jobs[].JobRunQueuingEnabled. AWS: Smithy BatchGetJobs returns a JobList of the same Job shape; the recording shows JobRunQueuingEnabled false on each returned job. Emulator: absent. Why: the batch read and the single read must describe a job identically; here BatchGetJobs is missing the member too. Fix: the Job serializer shared by BatchGetJobs. Traces: glue-gjwr-traj-region-sweep-locates-the-job#task#7 (cluster also covers glue-gtr-job-name-case-and-batch-lookup#task#7).

Cause 3 of 4: GetJobRun omits JobRun.ExecutionArguments, JobRun.JobRunQueuingEnabled, JobRun.LogGroupName and JobRun.PredecessorRuns. AWS: Smithy JobRun declares JobRunQueuingEnabled (NullableBoolean), LogGroupName (GenericString, documented default '/aws-glue/jobs') and PredecessorRuns (PredecessorList); the recording shows ExecutionArguments {} , JobRunQueuingEnabled false, LogGroupName '/aws-glue/jobs' and PredecessorRuns [] on every run. Emulator: all four absent. Why: a client reading a run cannot find the CloudWatch log group it must query, and the empty list/map members read as null instead of empty. (The same cluster also shows JobRun.ErrorMessage carrying the emulator's own sentence 'This emulator records job runs but executes no job script, so the job run ended without running.' — that run-never-executes root cause is already owned by open issue 20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md and is NOT re-filed here; this cause is the four omitted members.) Fix: the GetJobRun output serializer / job-run state defaults. Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4.

Cause 4 of 4: GetJobRuns omits the same four members on each JobRuns[] entry. AWS: as cause 3, per run. Emulator: JobRuns[0].ExecutionArguments, .JobRunQueuingEnabled, .LogGroupName and .PredecessorRuns all absent. Why: the list read is inconsistent with AWS in the same way as the single read, so neither path yields a usable log group. Fix: the JobRun serializer shared by GetJobRuns. Traces: glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#5.

Related open issues (do not cover these steps): 20261029T201859Z-glue-job-job-run-session-and-connection-reads.md and 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. All four clusters are 200/200 body mismatches.

Cause 1 — glue-glue-job-command-and-defaults-round-trip#task#3 GetJob: $.Job.Command.PythonVersion missing (expected '3'); $.Job.JobRunQueuingEnabled missing (expected False). 4 steps in cluster.
Cause 2 — glue-gjwr-traj-region-sweep-locates-the-job#task#7 BatchGetJobs: $.Jobs[0].JobRunQueuingEnabled missing (expected False). 2 steps in cluster.
Cause 3 — glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4 GetJobRun: $.JobRun.ExecutionArguments missing (expected {}), $.JobRun.JobRunQueuingEnabled missing (expected False), $.JobRun.LogGroupName missing (expected '/aws-glue/jobs'), $.JobRun.PredecessorRuns missing (expected []).
Cause 4 — glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#5 GetJobRuns: $.JobRuns[0].ExecutionArguments, .JobRunQueuingEnabled, .LogGroupName and .PredecessorRuns all missing with the same expected values.

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-glue-job-command-and-defaults-round-trip#task#3 — CreateJob then GetJob; look for Command.PythonVersion and JobRunQueuingEnabled.
2. glue-gjwr-traj-region-sweep-locates-the-job#task#7 — BatchGetJobs; look for Jobs[].JobRunQueuingEnabled.
3. glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#4 — GetJobRun; look for ExecutionArguments, JobRunQueuingEnabled, LogGroupName, PredecessorRuns.
4. glue-gjwr-traj-start-the-copied-job-fails-before-provisioning#task#5 — GetJobRuns; same members per entry.
