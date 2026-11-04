Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes|run#30|AssertionError|assert f"results/{qid}.csv" in keys, (
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed|run#17|JSONDecodeError|cause = json.loads(bad["cause"])
Issue-Key: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied|run#14|AssertionError|assert bad["status"] == "FAILED", (
Issue-Commit: 4158638b0e6fd8cb210c88acf50a6ef4ca666704

# athena__s3__stepfunctions: an Athena startQueryExecution.sync task writes no result object, carries no JSON QueryExecution payload, and succeeds for a role denied athena:GetQueryExecution (3 causes)

> _Note: this issue was identified against the code base at commit `4158638b0e6fd8cb210c88acf50a6ef4ca666704`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
The Step Functions optimized Athena integration completes without doing the work the integration is defined to do. 3 net-new root causes. (The open issue 20261025T112726Z-sfn-step-functions-does-not-execute-optimized-athena-service.md reports a startQueryExecution.sync task failing with States.Runtime; that is a different outcome from the three below and that issue carries none of these check ids.)

Cause 1 of 3: the query the sync task ran leaves no result object in the configured output location.
AWS: an athena:startQueryExecution.sync task waits for the query to reach SUCCEEDED, and a SUCCEEDED Athena query has written '<ResultConfiguration.OutputLocation><QueryExecutionId>.csv' to S3; a client listing that prefix after the execution finds exactly that object.
Emulator: the execution reports success but the prefix is empty — a ListObjectsV2 of the results prefix returns [] for the QueryExecutionId the task reports.
Why: a workflow whose next state consumes the result object (or a client that fetches results from S3 rather than GetQueryResults) finds nothing, while Step Functions says the task succeeded.
Fix: the athena crate's query-execution engine (result materialisation into S3) as reached through the sfn crate's optimized Athena integration.
Traces: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes.

Cause 2 of 3: the sync task's output and its failure Cause are not the JSON QueryExecution document AWS returns.
AWS: an athena:startQueryExecution.sync task's result (and, on failure, the States.TaskFailed Cause) is a JSON document containing a QueryExecution object — output carries QueryExecution.QueryExecutionId, and a failure Cause carries QueryExecution.Status with State FAILED and Status.AthenaError.ErrorMessage (for a missing table, text beginning 'TABLE_NOT_FOUND' and naming the table); the TaskSubmitted history event's output is likewise a JSON document carrying QueryExecutionId.
Emulator: the value served in those fields is not JSON at all — parsing it raises JSONDecodeError 'Expecting value: line 1 column 1 (char 0)', i.e. the field is empty or non-JSON. Hypothesis: the integration never populates the QueryExecution payload on either the success or the failure path.
Why: any state machine that reads $.QueryExecution.QueryExecutionId from the task result, or a client that parses the Cause to report why a query failed, crashes on an unparseable value instead of branching on the query's status.
Fix: the sfn crate's optimized Athena service integration — the task result/Cause serialization must emit the QueryExecution JSON document (and its TaskSubmitted event details).
Traces: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed.

Cause 3 of 3: a state-machine role denied athena:GetQueryExecution still drives the sync task to success.
AWS: the .sync pattern polls GetQueryExecution with the state machine's execution role; without athena:GetQueryExecution that polling is denied and the execution ends FAILED (the task cannot observe completion).
Emulator: the execution ends SUCCEEDED — the integration performs no IAM authorization for the polling call it makes on the role's behalf.
Why: a least-privilege misconfiguration that AWS surfaces as a failed execution passes silently here, so a client cannot discover the missing permission until it runs against AWS.
Fix: the sfn crate's optimized Athena integration (authorize the GetQueryExecution poll under the state machine role) and the athena crate's request authorization for GetQueryExecution.
Traces: athena__s3__stepfunctions.xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied.

## Evidence
Replay of the simparity-behavioral-aws-v1 suite against the emulator built from this checkout (service heading '### athena__s3__stepfunctions').
Cause 1 — xsvc-sfn-1-athena-sync-succeeds-only-after-the-query-completes: AssertionError: Athena's result object for 5369c112-90db-46e5-b837-2e61b916d306 is not in s3://sim2real-xsvc-athsfn-26a8066bb9d6/results/: [].
Cause 2 — xsvc-sfn-1-athena-sync-missing-table-query-fails-as-task-failed: json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0), raised from the check's json.loads over the sync task's execution output / States.TaskFailed cause (the check then asserts cause['QueryExecution']['Status']['AthenaError']['ErrorMessage'] matches ^TABLE_NOT_FOUND\b and names the table).
Cause 3 — xsvc-sfn-1-athena-sync-without-get-query-execution-is-denied: AssertionError: a role without athena:GetQueryExecution ended 'SUCCEEDED', expected FAILED.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-behavioral-aws-v1 behavioral suite and look at the '### athena__s3__stepfunctions' section.
3. Re-replay the checks listed in trace_ids.
4. Minimal manual repro: define a state machine with a Task state Resource 'arn:aws:states:::athena:startQueryExecution.sync' and start it; (a) list the results prefix for the reported QueryExecutionId — expect the '<id>.csv' object; (b) json-parse the execution output (and, for a statement naming a missing table, the States.TaskFailed Cause) — expect a JSON document with QueryExecution/QueryExecutionId and Status.AthenaError.ErrorMessage beginning 'TABLE_NOT_FOUND'; (c) run the same machine with an execution role lacking athena:GetQueryExecution — expect the execution to end FAILED.
