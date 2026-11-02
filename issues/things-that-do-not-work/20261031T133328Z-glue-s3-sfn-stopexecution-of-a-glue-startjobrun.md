Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run|_wait_run_state#12|Timeout|time.sleep(interval_s)
Issue-Commit: c1b34f592c646ed434adfb831b97b313d8f8d1bc

# glue__s3__sfn: StopExecution of a glue:startJobRun.sync Task never stops the downstream Glue job run — the Task is never dispatched and the check hangs to the harness timeout (1 cause)

> _Note: this issue was identified against the code base at commit `c1b34f592c646ed434adfb831b97b313d8f8d1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopExecution

## Description
Cause 1 of 1: a Task state whose Resource is `arn:aws:states:::glue:startJobRun.sync` is never dispatched, so the StopExecution semantics that this integration owes — aborting the execution AND stopping the Glue job run it submitted — can never be observed. The check never gets past waiting for a submitted/RUNNING job run and is killed by the harness timeout rather than returning an answer. Failing check: glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

AWS: recorded behavior only for the `.sync` integration's stop propagation (Smithy models the pieces separately: sfn `StopExecution` takes `executionArn` (required, @length 1..256), `error` (@length 0..256, sensitive) and `cause` (@length 0..32768, sensitive) and returns `stopDate`; `DescribeExecution` returns `status` (ExecutionStatus enum), `error` and `cause`; Glue `StartJobRun`/`GetJobRun` expose `JobRun.JobRunState`). Real Step Functions, for a Task with Resource `arn:aws:states:::glue:startJobRun.sync`, submits StartJobRun under the execution role, emits TaskSubmitted, and parks the Task while polling GetJobRun. When StopExecution is called while the Task is parked, the service (a) stops the downstream Glue job run, so GetJobRun reports `JobRunState` `STOPPED` and the job's side effects (here, the output object the script would have written) never appear, and (b) terminates the execution with `status` `ABORTED`, carrying back verbatim the `error` and `cause` strings supplied on the StopExecution request, with the history ending in a single `ExecutionAborted` event and no `TaskSucceeded` and no `TaskFailed` for the stopped Task. The fixed text of this contract is the enum/event vocabulary — `STOPPED`, `ABORTED`, `ExecutionAborted` — while the `error`/`cause` values, the job name, the run id, the bucket and the key are all taken from the request and from state and are not part of the requirement.

Emulator: the execution never reaches a state where a job run has been submitted. The same dispatch gap is already described for the sibling check in the open issue 20261029T183117Z-glue-s3-sfn-a-glue-startjobrun-sync-task.md, which records the emulator refusing the Task outright with `status='FAILED'`, `error='States.Runtime'` and `cause="state 'Job': 'arn:aws:states:::glue:startJobRun.sync' names an integration with 'glue', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"`; that issue does not carry this check's replay key, and the related 20261026T234618Z-glue-s3-sfn-step-functions-refuses-the-sync.md and 20261024T043150Z-sfn-5-of-the-6-interpreter-causes-remain-open.md do not either. For THIS check the observable emulator behaviour is that no job run is ever submitted and no terminal answer is produced within the window: replay reports `Error: Check timed out after 60.0s`. Hypothesis: because the emulator-authored cause text contains the substring `startJobRun`, the check's `_is_propagation_failure` guard reads the refusal as the IAM-propagation race and keeps re-starting the execution until the harness deadline instead of failing fast — i.e. the refusal is also mis-shaped enough to be indistinguishable from a transient role failure.

Why: a client relying on AWS's behaviour gets a workflow that neither runs the Glue job nor honours cancellation. StopExecution appears to succeed at the execution level while the downstream job run keeps whatever state the emulator gave it, so a cancelled pipeline can still produce its output objects (nothing is stopped, nothing is cleaned up), and a client polling GetJobRun for `STOPPED`, or DescribeExecution for `ABORTED` with its own `error`/`cause`, or reading GetExecutionHistory for a terminating `ExecutionAborted`, waits forever. Because the Task is refused with the same `States.Runtime` in every arm, Catch/Retry keyed on Glue error names never matches, and the refusal's wording is close enough to a role-propagation failure that clients (and this check) retry it indefinitely instead of surfacing it.

Fix: the `.sync` service-integration dispatch for glue in the sfn service crate's Task-state handler, plus the stop path of the execution state machine — submit StartJobRun under the execution role, emit TaskSubmitted, park the Task while polling GetJobRun, and on StopExecution propagate the stop to the submitted Glue job run (driving it to `STOPPED` and suppressing its side effects) before terminating the execution as `ABORTED` with the caller's `error`/`cause` and a final `ExecutionAborted` history event and no TaskSucceeded/TaskFailed for the stopped Task.

Traces: glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

## Evidence
Expected: while the Task is parked, DescribeExecution reports RUNNING and GetJobRun reports JobRunState RUNNING; after StopExecution(error=<caller error>, cause=<caller cause>), GetJobRun reports JobRunState STOPPED, the output key the job script would have written does not exist, DescribeExecution reports status ABORTED carrying back exactly the caller's error and cause, and GetExecutionHistory ends with ExecutionAborted and contains neither TaskSucceeded nor TaskFailed.
Actual: the check never observes a submitted job run and is terminated by the harness — replay verdict `[mismatch] glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run`, `Error: Check timed out after 60.0s`. No Glue job run is submitted by the Task, so no STOPPED state, no ABORTED execution carrying the caller's error/cause, and no ExecutionAborted event is ever produced. The sibling arm of the same integration is recorded elsewhere answering `status='FAILED'`, `error='States.Runtime'` with an emulator-authored cause stating that `glue` is not one of the services the emulator makes a Task integration call to.

## Steps to Reproduce
1. Create an S3 bucket and a Glue job whose script sleeps and then writes an output object under `out/<token>.txt` in that bucket.
2. Create an IAM role trusted by states.amazonaws.com with glue:StartJobRun and glue:GetJobRun on that job.
3. Create a STANDARD state machine with a single Task state whose Resource is `arn:aws:states:::glue:startJobRun.sync`, passing the sleep mode, the key and the token as input.
4. StartExecution and poll GetExecutionHistory for TaskSubmitted to learn the Glue run id; poll GetJobRun until JobRunState is RUNNING, and confirm DescribeExecution reports RUNNING.
5. Call StopExecution(executionArn=<arn>, error="sim2real.Stop", cause="stop while parked").
6. Poll GetJobRun for a terminal JobRunState and read DescribeExecution plus GetExecutionHistory.
7. Observe that step 4 never completes: no job run is ever submitted by the Task, so the stop propagation cannot be observed and the check runs to the harness timeout.
