Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied|run#26|AssertionError|assert qe["Status"]["State"] == "SUCCEEDED", (
Issue-Commit: bd416c0ab091bf65c958786969d0a3369dfa0e54

# athena__s3__stepfunctions: a submitted Athena query never leaves RUNNING unless a .sync poller drives it — the query started by a role denied athena:GetQueryExecution stays RUNNING forever and produces no rows

> _Note: this issue was identified against the code base at commit `bd416c0ab091bf65c958786969d0a3369dfa0e54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryExecution

## Description
One cause.

Cause 1 of 1: an Athena query execution does not progress to a terminal state on its own; progress appears to be coupled to a caller polling GetQueryExecution (in this check, to the Step Functions `.sync` task's own poll loop). When nothing polls it, the query stays RUNNING indefinitely and its results are never produced.

AWS: StartQueryExecution is asynchronous and the Athena engine drives the execution forward independently of who observes it. Smithy (athena): QueryExecutionState is an enum QUEUED|RUNNING|SUCCEEDED|FAILED|CANCELLED, and its documentation states "QUEUED indicates that the query has been submitted to the service, and Athena will execute the query as soon as resources are available. RUNNING indicates that the query is in execution phase. SUCCEEDED indicates that the query completed without errors." QueryExecutionStatus carries SubmissionDateTime and, once the query finishes, CompletionDateTime. A trivial single-row statement of the form `SELECT '<literal>' AS marker` reaches SUCCEEDED within seconds, after which GetQueryResults returns the header row plus one data row. Authorization is on the *observing* call: whether the principal that started the query (here, the Step Functions execution role) is permitted to call athena:GetQueryExecution has no bearing whatsoever on whether the query itself runs to completion. This check encodes exactly that rule: the denial is on observing the query, not on running it.

Emulator: the query submitted by the Step Functions execution whose role omits athena:GetQueryExecution is still reported as `{'State': 'RUNNING', 'SubmissionDateTime': <ts>}` when read directly by an unrestricted caller — no CompletionDateTime, no StateChangeReason, and the result rows never materialise. The identical statement submitted by the positive-control execution (whose role keeps athena:GetQueryExecution, so the `.sync` task polls GetQueryExecution in a loop) does reach SUCCEEDED and the execution SUCCEEDS. Hypothesis: the query's state transition is performed lazily inside the GetQueryExecution handler (or inside the Step Functions `.sync` waiter) rather than by an independent clock/background advance, so a query that nobody polls never advances past RUNNING.

Why: a client relying on AWS's behavior cannot start a query and read its outcome later, from another principal, or out of band — the query it submitted is permanently RUNNING, GetQueryResults yields nothing, and no result object is produced. It also makes every negative-path test indistinguishable: "the poller was denied" and "the query never ran" look identical, and any workflow that fans out queries and collects results in a second phase hangs.

Fix: the Athena query-execution lifecycle in the athena service crate (the StartQueryExecution handler / query-execution state) must advance a submitted query from QUEUED to RUNNING to a terminal state on its own, driven by the emulator's clock rather than by a GetQueryExecution call, and must set Status.CompletionDateTime and materialise the result rows when it reaches SUCCEEDED. GetQueryExecution must be a pure read of that state.

Traces: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied. Related open issues describe other Athena query-plane defects (20261104T063259Z-athena-query-executions-never-succeed-statements-are-refused.md, 20261104T083348Z-athena-s3-stepfunctions-an-athena-startqueryexecution-sync-task.md) but none carries this check's replay key or this cause: here the query DOES succeed when polled, and the defect is that progress requires a poller at all.

## Evidence
Failing check athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied.

The check's Step Functions assertions all passed: the positive-control execution SUCCEEDED; the execution under the role lacking athena:GetQueryExecution ended FAILED with error States.TaskFailed, a cause whose "Error Code: AccessDeniedException;" segment names athena:GetQueryExecution, and a history containing TaskSubmitted but no TaskSucceeded.

It then failed on the final observation that the query itself ran:
  AssertionError: Athena reports the blind role's query <QueryExecutionId> as {'State': 'RUNNING', 'SubmissionDateTime': <timestamp>}

Expected (AWS): GetQueryExecution(QueryExecutionId=<id>).QueryExecution.Status.State == 'SUCCEEDED' with a CompletionDateTime set, and GetQueryResults paginating to rows [['marker'], ['<marker literal from the request>']].
Actual (emulator): Status.State == 'RUNNING', no CompletionDateTime, no rows. (The QueryExecutionId and timestamp above are per-run values; the requirement is the terminal state and the two result rows, whose data value is the literal taken from the submitted QueryString.)

## Steps to Reproduce
1. Create an S3 bucket and an Athena workgroup whose ResultConfiguration.OutputLocation points into it, with EnforceWorkGroupConfiguration=true.
2. Create two IAM roles trusted by states.amazonaws.com, each allowed s3 read/write on the bucket and glue:GetDatabase*/GetTable* on *. Role A is allowed athena:StartQueryExecution, athena:GetQueryExecution, athena:StopQueryExecution, athena:GetWorkGroup on the workgroup ARN; role B is identical except athena:GetQueryExecution is omitted.
3. Create two STANDARD state machines, one per role, each a single Task state with Resource 'arn:aws:states:::athena:startQueryExecution.sync' and Parameters {"QueryString.$": "$.query", "WorkGroup": "<workgroup>"}.
4. Start both with input {"query": "SELECT 'some-marker' AS marker"} and wait for both to go terminal. Role A's execution SUCCEEDS; role B's execution FAILS with States.TaskFailed naming the denied athena:GetQueryExecution (both correct).
5. Read role B's execution history, take QueryExecutionId from the TaskSubmitted event's output, and call athena GetQueryExecution for it as an unrestricted caller.
   -> Emulator: Status.State is 'RUNNING' with no CompletionDateTime, and GetQueryResults returns no rows.
   -> AWS: Status.State is 'SUCCEEDED' with a CompletionDateTime, and GetQueryResults returns [['marker'], ['some-marker']].
