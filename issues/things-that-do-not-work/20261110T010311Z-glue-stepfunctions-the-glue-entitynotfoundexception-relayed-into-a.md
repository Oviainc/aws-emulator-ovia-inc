Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__stepfunctions.start-job-run-sync-for-an-unknown-job-name-fails-as-task-failed
Issue-Key: glue__stepfunctions.start-job-run-sync-for-an-unknown-job-name-fails-as-task-failed|run#8|AssertionError|assert masked == (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# glue__stepfunctions: the Glue EntityNotFoundException relayed into a glue:startJobRun.sync task cause omits the Request ID segment

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Cause 1 of 1: the relayed Glue error envelope in a Step Functions task cause is missing its Request ID segment.

AWS: a `arn:aws:states:::glue:startJobRun.sync` task naming a Glue job that does not exist fails the execution with error `Glue.EntityNotFoundException` and cause `Failed to start job run due to missing metadata. (Service: AWSGlue; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <request id>; Proxy: null)`. The fixed text is `Failed to start job run due to missing metadata. (Service: AWSGlue; Status Code: 400; Error Code: EntityNotFoundException; Request ID: `, then the request id of the underlying Glue call, then `; Proxy: null)`. The request id's VALUE is per-run and is not the requirement; the presence of the `Request ID: <id>;` segment is. No job run is submitted (no TaskSubmitted event).

Emulator: the error name is correct, but the cause is `Failed to start job run due to missing metadata. (Service: AWSGlue; Status Code: 400; Error Code: EntityNotFoundException; Proxy: null)` — the `Request ID: <id>;` segment is absent from the envelope.

Why: a client (or an operator reading the execution) cannot correlate the failed task with the underlying Glue call, and any consumer that parses the five-field AWS SDK envelope out of the cause fails to parse it. The same envelope is produced for every Glue error relayed through an optimized integration, so every Glue task failure is affected.

Fix: the downstream-error envelope builder for the Glue optimized task integration — it must propagate the Glue call's request id into the rendered cause.

Traces: glue__stepfunctions.start-job-run-sync-for-an-unknown-job-name-fails-as-task-failed

## Evidence
Expected (check assertion, request id masked to `<id>`):
`Failed to start job run due to missing metadata. (Service: AWSGlue; Status Code: 400; Error Code: EntityNotFoundException; Request ID: <id>; Proxy: null)`

Actual (DescribeExecution `cause`):
`Failed to start job run due to missing metadata. (Service: AWSGlue; Status Code: 400; Error Code: EntityNotFoundException; Proxy: null)`

Status/error name matched: execution status FAILED, `error == 'Glue.EntityNotFoundException'`. Only the `Request ID: <id>;` segment of the fixed envelope is missing.

## Steps to Reproduce
1. Create an execution role trusted by states.amazonaws.com with glue:StartJobRun and glue:GetJobRun.
2. CreateStateMachine with a single Task whose Resource is `arn:aws:states:::glue:startJobRun.sync` and whose Parameters name a Glue job name that does not exist.
3. StartExecution and poll DescribeExecution to a terminal state.
4. Compare `cause` to the AWS envelope above (masking the request id); also confirm GetExecutionHistory contains no TaskSubmitted event.
