Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes|run#11|AssertionError|assert exe["status"] == "SUCCEEDED", (
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied|run#12|AssertionError|assert good["status"] == "SUCCEEDED", (
Issue-Commit: ec04c0da7dcd461e0ce3efb10993d5c9331936ba

# Step Functions does not execute optimized Athena service integrations: a startQueryExecution.sync Task fails the execution with States.Runtime instead of running and polling the query (1 cause)

> _Note: this issue was identified against the code base at commit `ec04c0da7dcd461e0ce3efb10993d5c9331936ba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
This issue carries ONE root cause. A fixer should re-replay both check ids listed in trace_ids before submitting.

Cause 1 of 1: the Step Functions execution interpreter does not make optimized (service-specific) AWS service integrations — concretely `arn:aws:states:::athena:startQueryExecution.sync` — so any execution containing such a Task is terminated immediately instead of invoking the integration.

AWS: a Task state whose `Resource` is an optimized integration ARN of the form `arn:aws:states:::<service>:<action>[.sync]` is invoked by Step Functions under the execution role. For `athena:startQueryExecution.sync` Step Functions calls Athena StartQueryExecution with the Task `Parameters` (`QueryString`, `WorkGroup`, ...), records a `TaskSubmitted` history event whose `taskSubmittedEventDetails.output` carries the `QueryExecutionId`, then polls Athena GetQueryExecution until the query is terminal; only then does it emit `TaskSucceeded` and complete the state, with the GetQueryExecution response (`{"QueryExecution": {"QueryExecutionId": ..., "Status": {"State": "SUCCEEDED", ...}}}`) as the state result. Consequently the execution's `stopDate` and its `TaskSucceeded` event are at or after Athena's own `Status.CompletionDateTime`, and DescribeExecution reports `status=SUCCEEDED`. If the execution role lacks `athena:GetQueryExecution`, the integration's poll is denied AFTER the query was submitted, so the execution ends `status=FAILED`, `error="States.TaskFailed"` with a `cause` carrying Athena's underlying error code (`Error Code: AccessDeniedException;`) and naming the denied action `athena:GetQueryExecution`, and the history contains `TaskSubmitted` but no `TaskSucceeded`. Smithy (com.amazonaws.sfn): DescribeExecution output `status` is the required `ExecutionStatus` enum plus `output`/`error`/`cause`; GetExecutionHistory returns `HistoryEventList`, and `HistoryEventType` includes `TaskScheduled`, `TaskSubmitted`, `TaskSucceeded`, `TaskFailed`. The ASL integration catalogue itself is not in the Smithy model, so the integration semantics above are recorded behavior only (both checks' control arms succeed on real AWS).

Emulator: the execution terminates at once with `status="FAILED"`, `error="States.Runtime"`, `cause="state '<StateName>' invokes '<Resource ARN>', a service integration this emulator does not make, so nothing was run for it"` — observed as `state 'Query' invokes 'arn:aws:states:::athena:startQueryExecution.sync', a service integration this emulator does not make, so nothing was run for it`. In that message the fixed text is the emulator's own invented sentence (`invokes ... a service integration this emulator does not make, so nothing was run for it`); the state name and the Resource ARN are values taken from the submitted definition. AWS emits no such error name or message for a valid optimized-integration Task: `States.Runtime` here is wholly spurious. No Athena query is started, so no `TaskSubmitted`/`TaskSucceeded` events, no `QueryExecutionId`, and no result object in the workgroup's S3 output location ever appear.

Note: this is NOT the cause already filed as 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md (Cause 1 there is the generic "state '<Name>' is of type 'Task', which this emulator does not execute" refusal, carrying a different message and a different code path) nor 20261024T190039Z-events-lambda-sfn-http-task-arn-aws-states-http-invoke-is.md (the `http:invoke` Task). Neither of those issues carries these two replay keys, and the refusal text here names the per-integration dispatch table rather than the Task-state type dispatch.

Why: a client cannot orchestrate Athena (or any other optimized integration) from Step Functions. A workflow AWS runs to SUCCEEDED is reported FAILED with a non-existent runtime error, so callers that branch on `error`/`cause` see a bogus `States.Runtime` where AWS gives either a successful output or a `States.TaskFailed` carrying the downstream service's error; `.sync` completion ordering (the task must not finish before the query does), the query id carried across the service boundary, and the execution-role authorization applied to the integration's own poll are all unobservable.

Fix: the optimized-service-integration dispatch component of the sfn execution interpreter (the Task-state Resource resolver for `arn:aws:states:::<service>:<action>` / `.sync` ARNs) plus the `.sync` completion poller and the TaskSubmitted/TaskSucceeded history emission, and the execution-role authorization applied to the integration's own calls. The sfn control plane is not implicated: CreateStateMachine already accepts the definition.

Traces: athena__s3__sfn.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes, athena__s3__sfn.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied. (A third check in this group, athena__s3__sfn.xsvc-sfn-1-athena-sync-stop-execution-cancels-the-running-query, is already filed against the unimplemented GetExecutionHistory stub and is not bundled here.)

## Evidence
Check athena__s3__sfn.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes:
  expected DescribeExecution status SUCCEEDED (with output `{"QueryExecution": {"QueryExecutionId": <id>, "Status": {"State": "SUCCEEDED"}}}` and exactly one TaskSubmitted plus one TaskSucceeded event)
  actual   AssertionError: execution was 'FAILED': 'States.Runtime' "state 'Query' invokes 'arn:aws:states:::athena:startQueryExecution.sync', a service integration this emulator does not make, so nothing was run for it"

Check athena__s3__sfn.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied (fails on its positive control, same cause):
  expected the full-permission control execution SUCCEEDED, and the role-without-athena:GetQueryExecution twin FAILED with error 'States.TaskFailed' and a cause matching `Error Code: AccessDeniedException;` and naming `athena:GetQueryExecution`, with TaskSubmitted present and TaskSucceeded absent in the history
  actual   AssertionError: the positive control was 'FAILED': 'States.Runtime' "state 'Query' invokes 'arn:aws:states:::athena:startQueryExecution.sync', a service integration this emulator does not make, so nothing was run for it"

In both runs no Athena query is created by the execution (no QueryExecutionId in any history event), so the downstream assertions on Athena's own GetQueryExecution/GetQueryResults and on the `results/<qid>.csv` object in the workgroup bucket are never reached.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite (both check ids above fail).
2. Manually: create an S3 bucket and an Athena workgroup whose ResultConfiguration.OutputLocation is `s3://<bucket>/results/`; create an IAM role trusted by states.amazonaws.com granting athena:StartQueryExecution, athena:GetQueryExecution, athena:StopQueryExecution, athena:GetWorkGroup on that workgroup plus s3 read/write on the bucket.
3. CreateStateMachine (type STANDARD) with a single state: {"Type":"Task","Resource":"arn:aws:states:::athena:startQueryExecution.sync","Parameters":{"QueryString.$":"$.query","WorkGroup":"<workgroup>"},"End":true}.
4. StartExecution with input {"query": "SELECT 'marker' AS marker, 42 AS answer"} and poll DescribeExecution to a terminal status.
5. Observed: status FAILED, error 'States.Runtime', cause "state 'Query' invokes 'arn:aws:states:::athena:startQueryExecution.sync', a service integration this emulator does not make, so nothing was run for it". Expected: status SUCCEEDED, output carrying the QueryExecution with Status.State SUCCEEDED, and GetExecutionHistory containing one TaskSubmitted (whose details name the QueryExecutionId) and one TaskSucceeded at or after Athena's CompletionDateTime.
6. Repeat step 3-4 with a second role identical except that athena:GetQueryExecution is omitted. Expected: FAILED with error 'States.TaskFailed' and a cause carrying `Error Code: AccessDeniedException;` and the action name athena:GetQueryExecution, with TaskSubmitted present and TaskSucceeded absent, while Athena itself still reports the started query SUCCEEDED.
