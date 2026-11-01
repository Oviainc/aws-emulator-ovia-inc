Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds
Issue-Key: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file|run#9|AssertionError|assert run1["JobRunState"] == "SUCCEEDED", (
Issue-Key: glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds|run#22|AssertionError|assert warmup["JobRunState"] == "SUCCEEDED", (
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Glue: a started job run executes no script, so every run ends FAILED and produces no side effects

> _Re-filed from `20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md`, which bundled two root causes. The CloudFormation one (AWS::Glue::Database / ::Table / ::Crawler not existing under their declared names, and `Ref` resolving to a generated physical id) is fixed and that report is closed; this one was NOT, so it keeps its two replay keys here rather than being carried into the closed directory. It was identified against commit `e9591ea0ffea53e5939624d35a733297d20386ca`; the code base may have drifted._

## Operation
StartJobRun

## Description
A started job run executes no script, so every run ends FAILED and produces no side effects.

AWS: StartJobRun runs the job's ScriptLocation; the run reaches JobRunState='SUCCEEDED', the script's S3 writes land, and a GlueContext job bookmark advances at job.commit() so a later run over the same prefix sees only new files (and a reset reprocesses everything). A workflow trigger that fires the job observes the same terminal state.

Emulator: the run ends JobRunState='FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' (`JOB_RUN_NOT_EXECUTED_MESSAGE` in services/service-com-amazonaws-glue-awsglue/src/state.rs, materialised by `JobRunEntry::settle_if_due`).

Why: no Glue data-plane behavior can be observed at all — bookmarks never advance, downstream objects are never written, and any orchestration gated on a SUCCEEDED run (workflow triggers, Step Functions sync tasks) can never proceed.

Fix: run the submitted script and drive the run to SUCCEEDED, including bookmark advancement at commit. PLATFORM-BLOCKED as of this report: the script a Glue job run executes is a PySpark/GlueContext program at the job's ScriptLocation, so serving this faithfully needs an in-process Spark + Python runtime, and this platform provides neither that nor the containers or external processes such a runtime would otherwise need. The refusal must NOT be re-worded in the meantime (answering a different error, or synthesising a SUCCEEDED state, a duration or side effects for work nothing ran, is worse than the honest failure), so the run keeps the message above until the runtime exists. The same family is open as 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md and 20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md.

Traces: glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file, glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds.

## Evidence
glue__s3.glue-job-bookmark-delta-then-reset-reprocesses-every-file: "AssertionError: run 1 ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"; glue__s3.glue-workflow-runs-its-second-trigger-only-after-the-first-job-succeeds: "AssertionError: the standalone producer control ended 'FAILED': 'This emulator records job runs but executes no job script, so the job run ended without running.'"

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. Upload a pythonshell/glueetl script that writes an S3 object, CreateJob over it, StartJobRun, then poll GetJobRun — expect JobRunState='SUCCEEDED' and the object present.
