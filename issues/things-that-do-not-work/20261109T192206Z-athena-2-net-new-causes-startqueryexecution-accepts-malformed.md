Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.start-query-execution-malformed-sql-rejected athena.notebook-mutations-throttle-too-many-requests
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.start-query-execution-malformed-sql-rejected|run#10|AssertionError|raise AssertionError("expected StartQueryExecution to reject malformed SQL, but it accepted it")
Issue-Key: athena.notebook-mutations-throttle-too-many-requests|run#26|AssertionError|assert not unexpected, f"unexpected error codes among concurrent CreateNotebook calls: {unexpected}"
Issue-Commit: fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc

# Athena: 2 net-new causes — StartQueryExecution accepts malformed SQL instead of refusing it synchronously, and notebook mutations enforce no request-rate quota

> _Note: this issue was identified against the code base at commit `fdb8a4e953f01d6c4e0b3bc9b573d323716eabbc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Two net-new root causes in the Athena emulator, both grounded in failing behavioral checks that the replay report left unmarked (no ALREADY-FILED and no OWNED-by marker).

Cause 1 of 2: StartQueryExecution performs no synchronous statement parse, so a malformed statement is ACCEPTED and a QueryExecutionId is minted.
Failing check id: athena.start-query-execution-malformed-sql-rejected
  AWS:      Smithy (services/service-com-amazonaws-athena-amazonathena/smithy-model/com-amazonaws-athena-amazonathena.json): StartQueryExecution input QueryString is required, @length min 1 max 262144; modeled errors are InternalServerException (500), InvalidRequestException (400) and TooManyRequestsException (400). Real Athena parses the statement in the StartQueryExecution call itself: a statement that does not parse is refused synchronously with HTTP 400 / Error Code InvalidRequestException and the Trino parser diagnostic as the message, and NO QueryExecutionId is minted (the workgroup's ListQueryExecutions stays empty). The message for this error class is the parser's own text: the fixed shape is "line <line>:<col>: mismatched input '<offending token>'. Expecting: <the parser's comma-separated list of statement-start keywords>, <query>"; the line/column, the offending token are values taken from the submitted QueryString, and the keyword list is the parser's fixed vocabulary for a statement-start position. For the recorded submission "SELEC 1 FRM dual !!!" that renders as: "line 1:1: mismatched input 'SELEC'. Expecting: 'ALTER', 'ANALYZE', 'CALL', 'COMMENT', 'COMMIT', 'CREATE', 'DEALLOCATE', 'DELETE', 'DENY', 'DESC', 'DESCRIBE', 'DROP', 'EXECUTE', 'EXPLAIN', 'GRANT', 'INSERT', 'MERGE', 'PREPARE', 'REFRESH', 'RESET', 'REVOKE', 'ROLLBACK', 'SET', 'SHOW', 'START', 'TRUNCATE', 'UNLOAD', 'UPDATE', 'USE', <query>" — the quoted keyword list and the fixed words "mismatched input" / "Expecting:" are the template; 'SELEC' and the 1:1 position come from the request.
  Emulator: StartQueryExecution returns 200 with a QueryExecutionId for the malformed statement — no ClientError at all. The check's first assertion fires: "expected StartQueryExecution to reject malformed SQL, but it accepted it". The emulator therefore never produces the synchronous InvalidRequestException for this class, and it mints an execution id for a statement that can never run.
  Why:      A client that relies on StartQueryExecution's synchronous parse refusal to validate user-supplied SQL (an editor, a query-builder, a job submitter that only polls after a successful submit) sees a success and an execution id, so it reports the bad statement as submitted and surfaces the parse error much later (or not at all). It also leaks a phantom execution into ListQueryExecutions for the workgroup, which AWS never does for a statement rejected at submit time.
  Fix:      StartQueryExecution's handler/validation (services/service-com-amazonaws-athena-amazonathena/src/ops/start_query_execution.rs) must parse the statement before any execution id is minted and, when it does not parse, refuse with 400 InvalidRequestException carrying the parser diagnostic, recording no query execution. Related open issue 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md covers the SQLite-derived diagnostics of statements that ARE accepted and then settle FAILED; it does not cover the absence of the synchronous submit-time refusal, which is this cause.
  Traces:   athena.start-query-execution-malformed-sql-rejected

Cause 2 of 2: no request-rate quota is enforced for notebook mutations; concurrent CreateNotebook calls answer NotImplemented and never a throttling error.
Failing check id: athena.notebook-mutations-throttle-too-many-requests
  AWS:      Smithy: CreateNotebook models TooManyRequestsException (400) alongside InvalidRequestException and InternalServerException. Real Athena enforces a request-rate quota on notebook mutations: under a sustained concurrent burst of CreateNotebook on one Spark workgroup, at least one call SUCCEEDS and at least one is refused with a throttling error (TooManyRequestsException, or another code botocore classifies as throttling), and no other error code appears. The error code is the requirement here; the message is AWS's throttling text for that class ("Rate exceeded"-class wording), with no request-derived variable parts.
  Emulator: every call in the burst is refused with error code NotImplemented ("operation CreateNotebook is scaffolded but not implemented in this emulator"); the check collects hundreds of NotImplemented codes and zero successes and zero throttles, failing on "unexpected error codes among concurrent CreateNotebook calls: ['NotImplemented', ...]". So the emulator serves neither the success side nor the TooManyRequestsException side of the quota: no notebook-mutation rate limiting exists.
  Why:      A client that drives notebooks concurrently and relies on the modeled TooManyRequestsException to back off never sees a throttle, so its retry/back-off path is never exercised against the emulator; and because the code it does get is an unmodeled NotImplemented, generic SDK throttling classification does not apply and the call fails hard instead of being retried.
  Fix:      CreateNotebook's handler (services/service-com-amazonaws-athena-amazonathena/src/ops/create_notebook.rs) plus the notebook rate-quota accounting in Athena's state. Prerequisite: open issue 20261104T163600Z-athena-createnotebook-stub.md asks for CreateNotebook to be implemented at all — that issue does not carry this check's replay key and does not describe the rate quota, so the quota enforcement asserted here is the net-new cause; implementing the stub alone will leave this check failing on the missing throttle. Open issue 20261108T041854Z-athena-notebook-operations-refuse-without-athenaerrorcode-errorcode-and.md covers notebook refusal representation, not rate limiting.
  Traces:   athena.notebook-mutations-throttle-too-many-requests

## Evidence
Cause 1 — athena.start-query-execution-malformed-sql-rejected (verdict: mismatch)
Request: StartQueryExecution(QueryString="SELEC 1 FRM dual !!!", WorkGroup=<a workgroup created by the check with ResultConfiguration.OutputLocation set>).
Expected (AWS): ClientError, HTTP 400, Error.Code == "InvalidRequestException", Error.Message == "line 1:1: mismatched input 'SELEC'. Expecting: 'ALTER', 'ANALYZE', 'CALL', 'COMMENT', 'COMMIT', 'CREATE', 'DEALLOCATE', 'DELETE', 'DENY', 'DESC', 'DESCRIBE', 'DROP', 'EXECUTE', 'EXPLAIN', 'GRANT', 'INSERT', 'MERGE', 'PREPARE', 'REFRESH', 'RESET', 'REVOKE', 'ROLLBACK', 'SET', 'SHOW', 'START', 'TRUNCATE', 'UNLOAD', 'UPDATE', 'USE', <query>"; and ListQueryExecutions(WorkGroup=<wg>).QueryExecutionIds == [] (no id minted).
Actual (emulator): HTTP 200 with a QueryExecutionId; no error raised. Replay failure: AssertionError: expected StartQueryExecution to reject malformed SQL, but it accepted it (check line 40).

Cause 2 — athena.notebook-mutations-throttle-too-many-requests (verdict: mismatch)
Request: 120 concurrent workers issuing CreateNotebook(WorkGroup=<a PySpark-engine-v3 workgroup with ExecutionRole>, Name=<unique>) back-to-back with retries disabled.
Expected (AWS): among the collected outcomes, >=1 success (None) and >=1 code in the throttling family (TooManyRequestsException / ThrottlingException / RequestThrottled / SlowDown / LimitExceededException / ...), and no code outside that family.
Actual (emulator): every outcome is error code "NotImplemented" with message "operation CreateNotebook is scaffolded but not implemented in this emulator" — 0 successes, 0 throttles. Replay failure: AssertionError: unexpected error codes among concurrent CreateNotebook calls: ['NotImplemented', 'NotImplemented', ... ] (check line 118).

## Steps to Reproduce
Cause 1:
1. s3:CreateBucket <out-bucket>.
2. athena:CreateWorkGroup(Name=<wg>, Configuration={ResultConfiguration:{OutputLocation: s3://<out-bucket>/out/}}).
3. athena:StartQueryExecution(QueryString="SELEC 1 FRM dual !!!", WorkGroup=<wg>) — observe HTTP 200 and a minted QueryExecutionId; AWS answers 400 InvalidRequestException with the Trino parser diagnostic and mints nothing.
4. athena:ListQueryExecutions(WorkGroup=<wg>) — observe the phantom execution id that AWS would not have created.
5. Positive control: athena:StartQueryExecution(QueryString="SELECT 1", WorkGroup=<wg>) must still be accepted and listed.

Cause 2:
1. iam:CreateRole trusting athena.amazonaws.com; s3:CreateBucket <out-bucket>.
2. athena:CreateWorkGroup(Name=<wg>, Configuration={ResultConfiguration:{OutputLocation: s3://<out-bucket>/}, EngineVersion:{SelectedEngineVersion:"PySpark engine version 3"}, ExecutionRole:<role arn>}).
3. With retries disabled, run ~120 concurrent threads issuing athena:CreateNotebook(WorkGroup=<wg>, Name=<unique>) continuously (deleting each created notebook immediately, at most 8 outstanding) until a throttle appears or ~130 s elapse.
4. Collect the error codes: on AWS at least one call succeeds and at least one returns a throttling code and nothing else appears; on the emulator every call returns NotImplemented, so neither side of the quota is observable.

Replay both with: athena.start-query-execution-malformed-sql-rejected, athena.notebook-mutations-throttle-too-many-requests
