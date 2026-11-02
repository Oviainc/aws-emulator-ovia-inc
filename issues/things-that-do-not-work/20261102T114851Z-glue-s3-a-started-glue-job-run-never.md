Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run
Issue-Key: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file|run#8|AssertionError|assert run1["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run|run#8|AssertionError|assert good["JobRunState"] == "SUCCEEDED", (
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# glue__s3: a started Glue job run never executes its script — every run ends FAILED with an emulator-authored ErrorMessage, so bookmark deltas and Parquet output are never produced (1 cause)

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
ONE cause, grounded in two glue__s3 checks that the suite reports as failing and that no open issue carries the replay key for. The same defect family is filed for other check sets (20261029T185330Z-glue-job-runs-are-recorded-but-never-executed-e42472.md, 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261102T091623Z-glue-job-runs-never-execute-every-run-ends.md); none of them names these two checks, so these two replay units are not gated by any open issue.

Cause 1 of 1: StartJobRun records a run and immediately terminates it FAILED without running the script.
AWS: Smithy: StartJobRun returns JobRunId and the run progresses through JobRunState STARTING -> RUNNING -> SUCCEEDED for a job whose script succeeds; GetJobRun.JobRun.ErrorMessage is populated only for a failed run. A Spark/pythonshell script that reads its S3 source and writes its S3 sink does exactly that, and with job bookmarks enabled a committed run advances the bookmark so the next run sees only new files, while ResetJobBookmark makes the next run reprocess every file.
Emulator: every StartJobRun settles JobRunState='FAILED' with the emulator-authored ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' — a sentence that names the emulator and that real Glue never returns. No script side effect is produced: no S3 output objects, no Parquet part files or partition layout, and the job bookmark never advances or resets.
Why: a client cannot observe a successful Glue run at all. Every downstream assertion collapses: no output object to read, no bookmark delta semantics, no SUCCEEDED state to branch on, and the failure reason tells the caller nothing actionable about its own job (it describes the emulator).
Fix: the StartJobRun / job-run state machine and its run executor in the glue service crate — a recorded run must execute the job's Command.ScriptLocation (or model its effects) and reach SUCCEEDED, and ErrorMessage must be left unset on a successful run.
Traces: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file, glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run

## Evidence
glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file: AssertionError: run 1 ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.' (expected JobRunState 'SUCCEEDED').
glue__s3.glue-spark-parquet-partition-layout-and-footer-aggregates-and-a-malformed-row-fails-the-run: AssertionError: the aggregate run ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.' (expected JobRunState 'SUCCEEDED', followed by Parquet partition-layout and footer-aggregate assertions on the S3 output).

## Steps to Reproduce
1. Create an S3 bucket, upload a Spark/pythonshell script and CSV source objects.
2. glue:CreateJob with Command.ScriptLocation pointing at the uploaded script (Glue 4.0, WorkerType G.1X, NumberOfWorkers 2), with '--job-bookmark-option job-bookmark-enable' for the bookmark arm.
3. glue:StartJobRun, then poll glue:GetJobRun until terminal.
4. Observe JobRunState 'FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' instead of 'SUCCEEDED', and no output objects under the sink prefix.
