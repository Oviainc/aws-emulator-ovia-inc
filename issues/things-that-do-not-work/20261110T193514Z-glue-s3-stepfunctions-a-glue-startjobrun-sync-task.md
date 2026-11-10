Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out|run#23|AssertionError|assert bad.get("error") == "States.Timeout", (
Issue-Commit: bd416c0ab091bf65c958786969d0a3369dfa0e54

# glue__s3__stepfunctions: a glue:startJobRun.sync task whose role lacks glue:GetJobRun fails the execution with States.TaskFailed instead of letting the state's TimeoutSeconds expire into TaskTimedOut / States.Timeout

> _Note: this issue was identified against the code base at commit `bd416c0ab091bf65c958786969d0a3369dfa0e54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One cause.

Cause 1 of 1: the `.sync` completion poll's authorization refusal is relayed as a task failure instead of being treated as "the run's outcome was not observed", so the execution's terminal error is States.TaskFailed where AWS reports States.Timeout.

AWS: for the `.sync` service-integration pattern Step Functions submits the call and then polls the target service for completion using the state machine's execution role. For 'arn:aws:states:::glue:startJobRun.sync' that completion poll is glue:GetJobRun (Smithy: glue GetJobRun takes the required JobName and RunId and returns JobRun; its modeled errors are EntityNotFoundException, InvalidInputException, OperationTimeoutException and InternalServiceException — an authorization refusal is the service-common AccessDeniedException, not one of these). When that poll is denied, Step Functions does not surface the refusal as a failure of the Task: the task simply never observes the run reaching a terminal JobRunState, so the Task state's own TimeoutSeconds elapses. The execution history then records TaskSubmitted followed by TaskTimedOut, with no TaskSucceeded and no TaskFailed, and the execution ends with status FAILED and error "States.Timeout". (Execution status TIMED_OUT is reserved for the execution-level TimeoutSeconds, which is not set here; a state-level TimeoutSeconds yields status FAILED with error States.Timeout.) Independently of all this, the Glue job run itself runs to JobRunState SUCCEEDED and its script's side effects land — the denial is on observing the run, not on running it. Recorded behavior: this is exactly what the check asserts, in contrast to the sibling athena:startQueryExecution.sync check, where AWS *does* fail the task; the two optimized integrations differ and must not be unified.

Emulator: the execution under the role that omits glue:GetJobRun ends FAILED with error 'States.TaskFailed' — i.e. the denied GetJobRun poll is propagated as a task-level failure (an AccessDeniedException-class cause) rather than leaving the task waiting, so no TaskTimedOut event is produced and the state's TimeoutSeconds is never reached.

Why: a client cannot distinguish "the Glue job failed" from "the job's outcome could never be observed". A Catch/Retry on States.Timeout — the handler AWS requires for this case — never fires, while a Catch on States.TaskFailed fires where AWS would not, so least-privilege diagnosis, timeout-based compensation and alarming all take the wrong branch. The history is also wrong shape: consumers asserting on TaskTimedOut see TaskFailed.

Fix: the Step Functions `.sync` service-integration waiter (the glue:startJobRun.sync completion-poll path) must classify an authorization refusal of the completion poll as "not yet observed" and keep waiting, letting the Task state's TimeoutSeconds drive the terminal outcome: emit TaskTimedOut and end the execution with status FAILED and error States.Timeout. Only refusals that are a genuine terminal verdict on the underlying run (e.g. the run no longer exists) belong on the TaskFailed path.

Traces: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out. The open issue 20261109T220008Z-glue-s3-stepfunctions-2-net-new-causes-a.md covered this check at an earlier assertion (back then the execution ended SUCCEEDED) and asserted AWS fails it with States.TaskFailed; that is not AWS's behavior for this integration and that issue does not carry this check's current replay key. 20261107T185637Z-glue-s3-stepfunctions-a-glue-job-run-reached-through-a-step.md covers a neighbouring check and does not cover this cause either.

## Evidence
Failing check glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-without-get-job-run-is-denied-and-times-out.

The positive control passed: the state machine whose role keeps glue:GetJobRun SUCCEEDED and its job wrote its marker object. The blind-role execution did reach a terminal FAILED status (the previously filed cause), but with the wrong error:
  AssertionError: the execution timed out with error 'States.TaskFailed', expected States.Timeout

Expected (AWS), for a Task state with Resource 'arn:aws:states:::glue:startJobRun.sync' and a state-level TimeoutSeconds, run under a role allowed glue:StartJobRun but not glue:GetJobRun:
  - DescribeExecution: status 'FAILED', error 'States.Timeout'
  - GetExecutionHistory: contains TaskSubmitted and TaskTimedOut; contains neither TaskSucceeded nor TaskFailed
  - glue GetJobRun(JobName, RunId from the TaskSubmitted output): JobRunState 'SUCCEEDED', and the job's output object present in S3
Actual (emulator): status 'FAILED' with error 'States.TaskFailed'. The error string 'States.Timeout' is fixed text (a States.* reserved error name); the TimeoutSeconds value, job name, run id and object key are values taken from the request and state.

## Steps to Reproduce
1. Create an S3 bucket and a Glue Python-shell job whose script writes a marker object to that bucket from its arguments.
2. Create two IAM roles trusted by states.amazonaws.com. Role A is allowed glue:StartJobRun, glue:GetJobRun (and the other glue actions the task needs) on the job ARN; role B is identical except glue:GetJobRun is omitted.
3. Create two STANDARD state machines, one per role, each a single Task state with Resource 'arn:aws:states:::glue:startJobRun.sync', Parameters naming the job and its arguments, and a state-level "TimeoutSeconds" (e.g. 150).
4. Start both executions with an input that makes the job write its marker, and wait for each to go terminal.
5. Inspect the blind-role (role B) execution.
   -> Emulator: status FAILED with error 'States.TaskFailed' (the denied GetJobRun poll is relayed as a task failure).
   -> AWS: status FAILED with error 'States.Timeout'; history has TaskSubmitted + TaskTimedOut and no TaskFailed/TaskSucceeded; GetJobRun for the submitted RunId reports JobRunState 'SUCCEEDED' and the marker object is in the bucket.
