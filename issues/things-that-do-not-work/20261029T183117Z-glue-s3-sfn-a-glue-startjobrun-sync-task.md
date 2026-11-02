Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-failing-job-run-fails-as-task-failed
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-failing-job-run-fails-as-task-failed|run#11|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# glue__s3__sfn: a glue:startJobRun.sync Task is refused outright, so a failing Glue job run never surfaces as States.TaskFailed (1 cause)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Cause 1 of 1: a Task state with Resource `arn:aws:states:::glue:startJobRun.sync` is not dispatched at all — the execution is failed immediately with `States.Runtime` and an emulator-authored cause — so the behaviour under test (a Glue job run that fails must surface in the execution as a TaskFailed naming the Glue failure) can never be observed.

AWS: recorded behavior only for the integration's failure mapping (Smithy models StartJobRun/GetJobRun and StartExecution separately). Real Step Functions submits the job run under the execution role, polls it to completion, and on a failed run fails the Task with a TaskFailed whose `error` is the Glue-prefixed error name (`Glue.AWSGlueException`, or `States.TaskFailed` for the run-level failure) and whose `cause` carries the job run's own ErrorMessage, taken from the Glue state; the job run id is observable through GetJobRun. The positive control (a succeeding run) ends the execution SUCCEEDED.

Emulator: `status='FAILED'`, `error='States.Runtime'`, `cause="state 'Job': 'arn:aws:states:::glue:startJobRun.sync' names an integration with 'glue', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"`. No job run is submitted, so neither the positive control nor the failure mapping is reachable.

Why: a client relying on AWS's behaviour cannot run a Glue job from a workflow at all, and Catch/Retry keyed on the Glue error name never matches because every outcome is the same `States.Runtime`.

Fix: the `.sync` service-integration dispatch for glue in the sfn service crate's Task-state handler — submit StartJobRun under the execution role, park the Task while polling GetJobRun, and map the terminal run state onto SUCCEEDED or a TaskFailed carrying the Glue error name and the run's ErrorMessage. The already-open issue 20261026T234618Z-glue-s3-sfn-step-functions-refuses-the-sync.md describes the same dispatch gap for the sibling checks but does not carry this check's replay key.

Traces: glue__s3__sfn.xsvc-sfn-1-glue-sync-failing-job-run-fails-as-task-failed.

## Evidence
Expected: the positive control execution reaches SUCCEEDED, and the failing-job arm reaches FAILED with a TaskFailed whose error is the Glue-prefixed failure name and whose cause carries the job run's ErrorMessage.
Actual: `AssertionError: the positive control was 'FAILED': 'States.Runtime' "state 'Job': 'arn:aws:states:::glue:startJobRun.sync' names an integration with 'glue', which is not one of the services this emulator makes a 'Task' state's integration call to, so nothing was run for it"` — no job run is submitted in either arm.

## Steps to Reproduce
1. Create an S3 bucket, a Glue job whose script succeeds, and a second whose script fails.
2. Create an IAM role trusted by states.amazonaws.com with glue:StartJobRun and glue:GetJobRun.
3. Create a STANDARD state machine with one Task state Resource=arn:aws:states:::glue:startJobRun.sync.
4. StartExecution against each job and read DescribeExecution/GetExecutionHistory.
5. Observe both executions FAILED with error States.Runtime and no job run submitted.
