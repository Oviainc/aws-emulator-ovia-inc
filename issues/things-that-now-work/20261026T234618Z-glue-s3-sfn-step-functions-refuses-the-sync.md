Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run|_start_until_submitted#10|AssertionError|raise AssertionError(f"the task never submitted its job run: {last!r}")
Issue-Commit: da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a

# glue__s3__sfn: Step Functions refuses the `.sync` service-integration pattern outright, so a glue:startJobRun.sync task never submits a job run and StopExecution can never be shown to stop it

> _Note: this issue was identified against the code base at commit `da7bc09c8c1c199d36cb9fc39c5c5796d768dc3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: a Task whose Resource is `arn:aws:states:::glue:startJobRun.sync` is not executed; the execution fails immediately with States.Runtime instead of submitting the Glue job run and parking on it ===
AWS: a Task state whose Resource ends in `.sync` uses the "run a job" service-integration pattern: Step Functions calls the downstream API (here glue:StartJobRun), records a TaskSubmitted history event carrying the started resource's id, PARKS the execution while the job run progresses, and only then resumes with TaskSucceeded or TaskFailed. While it is parked, DescribeExecution reports status RUNNING (Smithy: com.amazonaws.states#DescribeExecution output `status` is ExecutionStatus = RUNNING | SUCCEEDED | FAILED | TIMED_OUT | ABORTED | PENDING_REDRIVE). StopExecution(executionArn, error, cause) answers 200 and aborts the execution: DescribeExecution then reports status ABORTED carrying exactly the submitted `error` and `cause`, the history's last event is ExecutionAborted and contains neither TaskSucceeded nor TaskFailed, and — this is the cross-service contract the check grades — the abort PROPAGATES to the resource the caller never touched: the Glue job run the task started ends STOPPED (glue:GetJobRun) and never completes its side effect (the S3 marker object is never written). `States.Runtime` is reserved for a genuine runtime fault and is not an answer to a well-formed `.sync` definition.
Emulator: the execution terminates ~0.5 s after StartExecution with status FAILED, error "States.Runtime" and cause "state 'Job': 'arn:aws:states:::glue:startJobRun.sync' asks for the 'sync' integration pattern, whose outcome arrives after the call that starts it, so nothing was run for it" — an emulator-authored cause stating that the pattern is simply not implemented. No StartJobRun is ever issued, no TaskSubmitted event exists, and no Glue job run id is ever produced, so everything the check grades afterwards (the parked RUNNING execution, StopExecution, the downstream STOPPED job run, the absent S3 marker, the ExecutionAborted history) is unreachable. In that cause string the fixed text is the emulator's own sentence and the only variable parts are the state name (`Job`) and the Resource ARN taken from the definition; AWS emits no such cause for this definition at all.
Why: `.sync` is the standard way a client orchestrates Glue (and EMR, ECS, Batch, SageMaker…) from Step Functions, and the reason it exists is that the caller wants the state machine to own the downstream job's lifetime — including cancellation. A client gets a terminal FAILED with an unmodelled runtime error for a definition AWS runs, so it cannot test or rely on the abort propagating across the service boundary; a workflow that is stopped mid-job leaves the Glue job run running in the client's mental model, and nothing in the emulator ever contradicts that.
Related open issue that does NOT cover this check: 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md carries the SIBLING check glue__s3__sfn.xsvc-sfn-1-glue-sync-succeeds-only-after-the-job-run-completes, and its cause 1 describes a different emulator answer — "state '<Name>' is of type 'Task', which this emulator does not execute". Task states are now executed; what remains refused is the `.sync` integration pattern specifically, with a different cause string, and this check id appears in no open issue's replay set.
Fix: the service-integration dispatch of the sfn execution interpreter — implement the `.sync` pattern (issue the downstream start call, emit TaskSubmitted, suspend the execution, poll the downstream resource to a terminal state, resume with TaskSucceeded/TaskFailed) and wire the StopExecution path so aborting a parked execution cancels the resource it started (glue:BatchStopJobRun for a glue:startJobRun.sync task) before writing ExecutionAborted.
Traces: glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run.

## Evidence
glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run [mismatch]

Expected (AWS): after StartExecution on a state machine whose single Task is Resource "arn:aws:states:::glue:startJobRun.sync", the execution stays RUNNING while the job run runs; the history contains TaskSubmitted with the Glue job run id; glue:GetJobRun reports RUNNING; StopExecution(error="sim2real.Stop", cause="stop while parked") returns 200; DescribeExecution then reports status "ABORTED" with error "sim2real.Stop" and cause "stop while parked"; the history ends with "ExecutionAborted" and contains no TaskSucceeded/TaskFailed; glue:GetJobRun reports JobRunState "STOPPED"; and the job's output object was never written.

Actual (emulator): DescribeExecution immediately reports a terminal failure and no job run was ever submitted:
  AssertionError: the task never submitted its job run: {'executionArn': 'arn:aws:states:us-west-2:...:execution:...', 'status': 'FAILED', 'startDate': ...:34:16.727, 'stopDate': ...:34:17.227, 'error': 'States.Runtime', 'cause': "state 'Job': 'arn:aws:states:::glue:startJobRun.sync' asks for the 'sync' integration pattern, whose outcome arrives after the call that starts it, so nothing was run for it", 'redriveCount': 0, 'redriveStatus': 'REDRIVABLE'}

The diverging values are: HTTP 200 on DescribeExecution but status "FAILED" where AWS has "RUNNING" then "ABORTED"; error "States.Runtime" where AWS emits no error at all for this definition (and, after the stop, carries the caller's own error "sim2real.Stop"); and a cause string that is emulator prose. The execution ARNs, state-machine name, token/key names and the 500 ms start/stop gap are per-run values and are not part of the requirement; the requirement is that the task submits a job run, the execution parks RUNNING, and the stop propagates.

## Steps to Reproduce
1. s3.create_bucket for the job script and output; put a pythonshell script that sleeps and then writes a marker key.
2. glue.create_job for that script; iam.create_role trusting states.amazonaws.com with glue:StartJobRun/GetJobRun/BatchStopJobRun on that job.
3. sfn.create_state_machine with a single Task state: {"Type":"Task","Resource":"arn:aws:states:::glue:startJobRun.sync","Parameters":{"JobName":"<job>","Arguments":{...}},"End":true}.
4. sfn.start_execution with an input that makes the job sleep, then poll sfn.get_execution_history for a TaskSubmitted event.
   -> AWS: TaskSubmitted appears with the Glue job run id and DescribeExecution stays RUNNING.
   -> Emulator: the execution is already FAILED with error "States.Runtime" and cause "state 'Job': 'arn:aws:states:::glue:startJobRun.sync' asks for the 'sync' integration pattern, whose outcome arrives after the call that starts it, so nothing was run for it"; no job run exists.
5. (Blocked by step 4, and what must pass after the fix) poll glue.get_job_run until RUNNING; sfn.stop_execution(executionArn, error="sim2real.Stop", cause="stop while parked"); poll glue.get_job_run until terminal -> must be "STOPPED"; assert the marker key does not exist; sfn.describe_execution -> status "ABORTED" with that error/cause; sfn.get_execution_history -> last event type "ExecutionAborted", with no TaskSucceeded or TaskFailed.

Replay glue__s3__sfn.xsvc-sfn-1-glue-sync-stop-execution-stops-the-running-job-run to confirm a fix.
