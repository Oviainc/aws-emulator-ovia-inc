Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.op-contract-batch-get-query-execution
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.op-contract-batch-get-query-execution|run#14|AssertionError|assert unprocessed[0]["ErrorCode"] == "QUERY_EXECUTION_NOT_FOUND", (
Issue-Commit: 2d22b922898c4268bec96b0923119d5f2b2856eb

# Athena BatchGetQueryExecution classifies an unknown execution id as INVALID_INPUT instead of QUERY_EXECUTION_NOT_FOUND (1 cause)

> _Note: this issue was identified against the code base at commit `2d22b922898c4268bec96b0923119d5f2b2856eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetQueryExecution

## Description
1 cause.

Cause 1 of 1: an unresolvable-but-well-formed QueryExecutionId is reported under UnprocessedQueryExecutionIds with the wrong ErrorCode.
AWS: Smithy com.amazonaws.athena#UnprocessedQueryExecutionId models QueryExecutionId (length 1-128, pattern ^\\S+$), ErrorCode (length 1-256, doc: "The error code returned when the query execution failed to process, if applicable") and ErrorMessage; BatchGetQueryExecution returns resolvable ids under QueryExecutions and everything else under UnprocessedQueryExecutionIds instead of raising. The model does not enumerate the ErrorCode values, so this is recorded behavior: an id that is syntactically a valid execution id but resolves to no execution in the caller's account/region is reported with the fixed ErrorCode text 'QUERY_EXECUTION_NOT_FOUND'. INVALID_INPUT is the code for an id that is not a well-formed execution id at all; the only thing that varies in the entry is QueryExecutionId, which is echoed from the request.
Emulator: for the request [<real id>, <real id>, 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee'] it returns the two real executions correctly and puts the third under UnprocessedQueryExecutionIds with the correct QueryExecutionId but ErrorCode 'INVALID_INPUT' — it classifies every unresolved id as malformed input rather than distinguishing "not well formed" from "not found".
Why: ErrorCode is the field a client branches on for a partial-batch result. A caller that polls a stored set of execution ids cannot tell an execution that never existed (or has aged out of Athena's 45-day history) from a request it built wrongly; it retries or raises a validation alarm on what AWS reports as an ordinary not-found, and code that only treats QUERY_EXECUTION_NOT_FOUND as benign will mis-handle every unknown id.
Fix: the BatchGetQueryExecution handler's unprocessed-id classification — decide the ErrorCode from whether the id is well formed versus whether it resolves in state, instead of emitting one code for both.
Traces: athena.op-contract-batch-get-query-execution.

Note: no open issue carries this cluster. 20261104T163600Z-athena-constraint-refusals-raise-an-unmodeled-validationexception-instead.md and 20261104T163600Z-athena-conflict-not-found-and-output-location-refusals.md cover refusal codes/messages of modeled exceptions, not the in-band ErrorCode of UnprocessedQueryExecutionIds.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout, service heading 'athena'.

athena.op-contract-batch-get-query-execution [mismatch]:
  AssertionError: expected QUERY_EXECUTION_NOT_FOUND for unknown execution id, got 'INVALID_INPUT'

The check started two literal-only queries (SELECT 0 AS n / SELECT 1 AS n) to SUCCEEDED, then called BatchGetQueryExecution with both real ids plus one UUID-shaped id that was never issued. QueryExecutions, the per-execution Query/Status/Statistics assertions and UnprocessedQueryExecutionIds[0].QueryExecutionId all matched; only ErrorCode diverged: expected 'QUERY_EXECUTION_NOT_FOUND', got 'INVALID_INPUT'.

Smithy cited: com.amazonaws.athena#UnprocessedQueryExecutionId (members QueryExecutionId, ErrorCode, ErrorMessage); the code string itself is recorded behavior.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Run simparity-behavioral-aws-v1 and inspect athena.op-contract-batch-get-query-execution under the 'athena' heading.
3. Manual repro: start any query execution and wait for a terminal state, then call athena.batch_get_query_execution(QueryExecutionIds=[<that id>, 'aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee']) and read UnprocessedQueryExecutionIds[0]['ErrorCode'] — expect 'QUERY_EXECUTION_NOT_FOUND', observe 'INVALID_INPUT'.
