Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-succeeds-only-after-the-job-run-completes
Issue-Key: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-succeeds-only-after-the-job-run-completes|run#12|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Commit: e20658567def5986c911ca4169de4ecb96db84bd

# glue__s3__stepfunctions: Glue StartJobRun never executes the job script and ends every run FAILED with an emulator-authored ErrorMessage, so a glue:startJobRun.sync Task can never succeed (1 cause)

> _Note: this issue was identified against the code base at commit `e20658567def5986c911ca4169de4ecb96db84bd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One net-new root cause behind 1 failing check.

Cause 1 of 1: StartJobRun records a job run but never runs its script, and drives the run to the terminal state `FAILED` carrying an emulator-authored ErrorMessage. A Step Functions Task with `Resource: arn:aws:states:::glue:startJobRun.sync` therefore always ends FAILED with `States.TaskFailed`, and a job that AWS would run to success can never produce either its side effects or a SUCCEEDED execution.

AWS: StartJobRun executes the job's script with the supplied `Arguments`; GetJobRun reports `JobRunState` progressing to `SUCCEEDED` for a script that exits cleanly, with `ExecutionTime` and `CompletedOn` set and no `ErrorMessage`, and the script's own effects (here: an object written to S3 under the key and token passed in `--key`/`--token`) are observable. `JobRunState=FAILED` is accompanied by the script's/service's own ErrorMessage. The `.sync` integration polls the run and ends the Task SUCCEEDED when the run SUCCEEDS, or fails it with `States.TaskFailed` whose `cause` is the JSON JobRun document when the run fails. Recorded behavior only for the integration's payload shape; the requirement is that a succeeding script yields `JobRunState=SUCCEEDED` and a SUCCEEDED execution.

Emulator: every run ends `"JobRunState":"FAILED"` with `"ErrorMessage":"This emulator records job runs but executes no job script, so the job run ended without running."`, `"ExecutionTime":0`, and `CompletedOn` equal to `StartedOn` plus a fixed delta. That ErrorMessage is entirely emulator-authored fixed text — it names the emulator and states that nothing ran; no AWS Glue failure uses this wording. The Step Functions Task consequently reports `status='FAILED'`, `error='States.TaskFailed'` with that JobRun document as the cause, even for the positive-control job whose script is written to succeed.

Why: a client relying on AWS's behavior cannot run any Glue work. A job whose script succeeds is reported as a failure, so a workflow's happy path is unreachable and the script's effects (S3 output, token files, catalog updates) never appear; failure handling is also broken in the other direction, because a genuinely failing job is indistinguishable from a succeeding one — both carry the same emulator ErrorMessage, so Retry/Catch and alerting keyed on the Glue error text can never discriminate.

Fix: the glue service crate's StartJobRun/GetJobRun job-run execution path — actually execute the job script (or model its completion) and drive `JobRunState` to SUCCEEDED for a clean exit with `ExecutionTime`/`CompletedOn` set and no ErrorMessage, reserving FAILED plus the script's real ErrorMessage for a genuine failure; and make the sfn `.sync` integration's terminal mapping follow that state. The open issue 20261029T183117Z-glue-s3-sfn-a-glue-startjobrun-sync-task.md records an earlier, different symptom for this suite (the glue integration not being dispatched at all, cause `names an integration with 'glue', which is not one of the services this emulator makes a 'Task' state's integration call to`); the integration is now dispatched, the job run is now created, and the remaining defect is that the run never executes — that issue does not carry this check's replay key.

Traces: glue__s3__stepfunctions.xsvc-sfn-1-glue-sync-succeeds-only-after-the-job-run-completes.

## Evidence
Expected (AWS): the glue:startJobRun.sync Task's execution reaches `status="SUCCEEDED"` only after GetJobRun reports `JobRunState="SUCCEEDED"` for the run, with the script's S3 object (the token passed in --token/--key) present.

Actual (emulator): `AssertionError: execution was 'FAILED': 'States.TaskFailed' '{"JobRun":{...,"JobRunState":"FAILED",...,"ErrorMessage":"This emulator records job runs but executes no job script, so the job run ended without running.","AllocatedCapacity":0,"ExecutionTime":0,...}}'`.

Fixed text vs values: the ErrorMessage is wholly emulator-invented fixed text; the JobRun's Id, JobName, Arguments and timestamps are per-run values. AWS for a clean script returns `JobRunState=SUCCEEDED` with no ErrorMessage; for a real failure the ErrorMessage is the script's/service's own.

## Steps to Reproduce
1. Create an S3 bucket and a Glue job whose script writes a token object to that bucket and exits cleanly.
2. Create an IAM role trusted by states.amazonaws.com granting glue:StartJobRun and glue:GetJobRun.
3. Create a STANDARD state machine with one Task state Resource=arn:aws:states:::glue:startJobRun.sync passing --bucket/--key/--token as Arguments.
4. StartExecution and poll DescribeExecution; also call GetJobRun for the created run.
5. Expected: JobRunState SUCCEEDED, the S3 object present, execution SUCCEEDED. Observed: JobRunState FAILED with the emulator ErrorMessage, no object written, execution FAILED States.TaskFailed.
