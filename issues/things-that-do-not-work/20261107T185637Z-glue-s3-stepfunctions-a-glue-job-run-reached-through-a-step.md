Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run|run#13|AssertionError|assert state == "RUNNING", f"run {run_id} never reached RUNNING, last state {state!r}"
Issue-Commit: d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea

# A Glue job run reached through a Step Functions sync task never reaches RUNNING — it is FAILED immediately

> _Note: this issue was identified against the code base at commit `d6cb6c274eaec5e2dce099ad9d2ddf00c4faf8ea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One root cause, grounded in a failing check this report does NOT mark ALREADY FILED or OWNED. Open issues 20261106T214711Z-glue-s3-stepfunctions-glue-startjobrun-never-executes-the.md and 20261029T183117Z-glue-s3-sfn-a-glue-startjobrun-sync-task.md own other checks of this service and do not carry this replay key.

Cause 1 of 1: StartJobRun never produces a RUNNING job run, so there is nothing for StopExecution to stop.
AWS: StartJobRun moves the run through JobRunState 'STARTING' -> 'RUNNING' while the script executes, and GetJobRun observes RUNNING for the duration; stopping the owning Step Functions execution of a .sync Glue task then propagates a BatchStopJobRun, and the run ends 'STOPPED'. The observable RUNNING window is the precondition of the behaviour under test.
Emulator: the run goes straight to 'FAILED' half a second after StartedOn, carrying the emulator's own constant ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.' and ExecutionTime 0, so RUNNING is never observed — the check reports "run <id> never reached RUNNING, last state 'FAILED'".
Why: no Glue job can be observed running, cancelled or stopped; a client's orchestration that waits for RUNNING, or that cancels a long job, has no state to act on, and the run's failure reason is a constant that attributes nothing.
Fix: the Glue job-run executor (StartJobRun/GetJobRun state machine) — it must execute the script and expose a real RUNNING state, and honour BatchStopJobRun by ending the run STOPPED.
Traces: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

## Evidence
Expected GetJobRun to report JobRunState 'RUNNING' while the script runs, then 'STOPPED' after the owning execution is stopped. Actual: 'run <jr-id> never reached RUNNING, last state FAILED' with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'

## Steps to Reproduce
1) CreateJob with a long-running script in S3.
2) StartExecution of a state machine with an arn:aws:states:::glue:startJobRun.sync task; poll GetJobRun for 'RUNNING'.
3) StopExecution and poll GetJobRun for 'STOPPED'.
