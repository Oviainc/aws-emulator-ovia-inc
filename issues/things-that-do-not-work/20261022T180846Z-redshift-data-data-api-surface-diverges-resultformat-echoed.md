Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.get-statement-result-v2-csv-roundtrip redshift-data.json-and-csv-result-readers-are-mutually-exclusive redshift-data.get-statement-result-no-result-set-rejected redshift-data.list-sessions-conflicting-filters-rejected redshift-data.sql-named-parameters-bind-and-mismatched-parameters-rejected redshift-data.client-token-reuse-silently-ignores-the-new-sql-over-permissive
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.get-statement-result-v2-csv-roundtrip|run#11|AssertionError|assert select_final["ResultFormat"] == "csv", (
Issue-Key: redshift-data.json-and-csv-result-readers-are-mutually-exclusive|run#8|AssertionError|assert json_stmt["ResultFormat"] == "json", (
Issue-Key: redshift-data.get-statement-result-no-result-set-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: redshift-data.list-sessions-conflicting-filters-rejected|run#9|AssertionError|assert code == "ValidationException", code
Issue-Key: redshift-data.sql-named-parameters-bind-and-mismatched-parameters-rejected|_rejection#17|AssertionError|raise AssertionError(f"expected a rejection but the call SUCCEEDED, returning {result!r}")
Issue-Key: redshift-data.client-token-reuse-silently-ignores-the-new-sql-over-permissive|run#18|AssertionError|assert rewritten["QueryString"] == second_sql, (
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# redshift-data Data API surface diverges: ResultFormat echoed in upper case, GetStatementResult succeeds where AWS refuses, ListSessions is a NotImplemented stub, parameter-name mismatches accepted, ClientToken replay does not rewrite QueryString (5 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStatementResult

## Description
Companion to the redshift-data statement-execution-engine issue filed from the same replay. These five causes are in the API response/validation surface rather than the SQL engine. This bundle carries FIVE causes; all five must be closed before submitting.

=== Cause 1 of 5: ResultFormat is reported in upper case where AWS reports it lower case ===
AWS: Smithy (com-amazonaws-redshiftdata-redshiftdata): ResultFormatString is an enum whose values are JSON|CSV and it is the type of ExecuteStatementInput.ResultFormat, DescribeStatementResponse.ResultFormat and GetStatementResultV2Response.ResultFormat — so the model only fixes the INPUT spelling. Recorded behavior (both checks were measured against real AWS): a statement submitted with ResultFormat='CSV' is reported back by DescribeStatement as 'csv', and a statement submitted with NO ResultFormat is reported as 'json'. AWS echoes the lower-case form on the way out.
Emulator: DescribeStatement returns 'CSV' and 'JSON' respectively — the submitted/defaulted enum token is echoed verbatim.
Why: this is the field a client reads to decide which reader to use (GetStatementResult for json, GetStatementResultV2 for csv). Any caller or waiter comparing `ResultFormat == 'csv'` / `== 'json'` — the values AWS actually sends — takes the wrong branch, and the two reader-selection checks in this suite cannot proceed past it.
Fix: the redshift-data DescribeStatement (and GetStatementResultV2) serializer / statement-record projection — emit the lower-case form AWS emits when rendering ResultFormat, while continuing to accept the modeled upper-case input tokens.
Traces: redshift-data.get-statement-result-v2-csv-roundtrip, redshift-data.json-and-csv-result-readers-are-mutually-exclusive.

=== Cause 2 of 5: GetStatementResult on a FINISHED statement with no result set answers 200 with an empty result instead of refusing it ===
AWS: Smithy: GetStatementResult models ResourceNotFoundException (client, httpError 404) and ValidationException (client, 400); Records is a @required member. Recorded behavior: for a FINISHED DDL statement (HasResultSet false) AWS refuses the read with error code ResourceNotFoundException and the fixed message 'Query does not have result. Please check query status with DescribeStatement.' (the same refusal is recorded for a FAILED and for an ABORTED statement), and the DDL statement's own record is untouched by the attempt.
Emulator: HTTP 200 with {'Records': [], 'ColumnMetadata': [], 'TotalNumRows': 0} — the call succeeds and reports an empty result set.
Why: 'no result set' and 'an empty result set' are different answers. A client that distinguishes a DDL/failed statement from a SELECT that matched nothing (and every negative test asserting the refusal) is silently told the wrong thing, and a reader loop will happily consume results from statements that never produced any.
Fix: the GetStatementResult / GetStatementResultV2 handler — gate the read on the statement having a result set (HasResultSet true and a terminal FINISHED status) and raise the not-found error with AWS's fixed message instead of synthesising an empty page.
Traces: redshift-data.get-statement-result-no-result-set-rejected.

=== Cause 3 of 5: ListSessions answers NotImplemented ===
AWS: Smithy: ListSessions is a bound, readonly, paginated operation (input NextToken/MaxResults 0..100/SessionId/Status/RoleLevel/ClusterIdentifier/WorkgroupName/Database; output Sessions @required + NextToken) whose modeled errors are ValidationException (400), ResourceNotFoundException (404) and InternalServerException (500). Recorded behavior: SessionId together with another filter is refused ValidationException with the message 'SessionId cannot be specified with other filter parameters.', while SessionId alone returns the one live session.
Emulator: the call fails with error code `NotImplemented` (the check observed code 'NotImplemented' where ValidationException was expected), so neither the conflicting-filter rejection nor the positive single-filter listing is served. The suite did not flag this as an already-filed scaffold stub (the error was consumed by the check's own error-capture helper), and no open issue names ListSessions.
Why: a client cannot enumerate or inspect its Data API sessions at all, and the mutually-exclusive-filter contract is unenforced.
Fix: implement the redshift-data ListSessions operation handler over the session state (services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/list_sessions.rs), including the SessionId-excludes-every-other-filter validation.
Traces: redshift-data.list-sessions-conflicting-filters-rejected.

=== Cause 4 of 5: ExecuteStatement accepts named-parameter sets that do not match the SQL ===
AWS: Smithy: ExecuteStatementInput.Parameters is a SqlParametersList with only @length(min 1), so none of these rules is model-declared — recorded behavior (measured us-west-2 2026-09-11): all three name mismatches are SYNCHRONOUS rejections, ValidationException with HTTP 400 and three distinct fixed messages: a parameter the SQL does not name -> 'Parameters list doesn't contain all parameters. Missing parameters: [i]'; an extra parameter -> 'Parameters list contains unused parameters. Unused parameters: [x]'; a repeated name -> 'Found duplicate named parameter "i"'. None of the refusals registers a statement.
Emulator: the submit SUCCEEDS — HTTP 200 with an Id and Status 'SUBMITTED' — for a Parameters list whose names do not match the SQL's placeholders. (Binding itself works: the check's earlier assertions that a quoted value round-trips and an injection payload matches zero rows all passed.)
Why: the mismatch is only discoverable later, as a failed or wrong statement, instead of at submit time; a caller that mis-names a parameter gets an Id for a statement AWS would never have created, and the statement record/ListStatements gain an entry AWS does not create.
Fix: ExecuteStatement's input validation (shared with BatchExecuteStatement) — resolve the SQL's named placeholders against Parameters before admitting the statement and emit the 400 ValidationException with AWS's missing/unused/duplicate message for each case.
Traces: redshift-data.sql-named-parameters-bind-and-mismatched-parameters-rejected.

=== Cause 5 of 5: replaying a ClientToken with different SQL returns the first statement's Id but DescribeStatement keeps reporting the old QueryString ===
AWS: Smithy: ExecuteStatementInput.ClientToken is an @idempotencyToken (length 1..64) and the model says nothing about a mismatched replay — recorded behavior: re-submitting with the SAME ClientToken but DIFFERENT Sql is NOT rejected; it returns the FIRST statement's Id, nothing new runs (CreatedAt, UpdatedAt, Status and ResultRows all stay frozen at the first execution, and the new SQL's write never happens), but DescribeStatement for that Id thereafter reports the NEWLY submitted QueryString — i.e. the record misreports which SQL it ran.
Emulator: the Id is correctly the first statement's, but DescribeStatement still reports the first SQL ('insert into sim2real_behav_tok_<id> values (\'first\')'), so the QueryString is never rewritten by the mismatched replay.
Why: this is the over-permissive quirk a client must be able to observe. A caller (or a conformance test) that reads QueryString back to confirm what the token's statement actually ran sees a different value on AWS than on the emulator, and the follow-on assertions about frozen CreatedAt/UpdatedAt/ResultRows are never reached.
Fix: ExecuteStatement's idempotency/ClientToken handling — on a token hit, keep the existing statement and its execution state but overwrite the stored QueryString (and QueryParameters) with the newly submitted request, as AWS does.
Traces: redshift-data.client-token-reuse-silently-ignores-the-new-sql-over-permissive.

## Evidence
Suite: simparity-behavioral-aws-v1, redshift-data 6/24 (25%), 18 failed.

Cause 1 — get-statement-result-v2-csv-roundtrip: expected DescribeStatement ResultFormat 'csv' for a statement submitted with ResultFormat='CSV'; actual 'CSV'. json-and-csv-result-readers-are-mutually-exclusive: expected 'json' for a statement submitted with no ResultFormat; actual 'JSON'.

Cause 2 — check_redshift_data_get_statement_result_no_result_set_rejected.py:28
  expected: ClientError ResourceNotFoundException, Message 'Query does not have result. Please check query status with DescribeStatement.'
  actual:   HTTP 200, {'Records': [], 'ColumnMetadata': [], 'TotalNumRows': 0}

Cause 3 — check_redshift_data_list_sessions_conflicting_filters_rejected.py:41
  expected: list_sessions(SessionId=<id>, Status='AVAILABLE') -> ValidationException 'SessionId cannot be specified with other filter parameters.'
  actual:   AssertionError: NotImplemented

Cause 4 — check_redshift_data_sql_named_parameters_bind_and_mismatched_parameters_rejected.py:215
  expected: execute_statement(Sql='select id from t where id = :i', Parameters=[{'name':'zzz','value':'1'}]) -> ValidationException / 400 / "Parameters list doesn't contain all parameters. Missing parameters: [i]"
  actual:   HTTP 200 {'Id': '724a4b8c-...', 'Status': 'SUBMITTED', 'Database': 'devdb', 'WorkgroupName': 'sim2real-behav-rsd-wg-...'}

Cause 5 — client-token-reuse-silently-ignores-the-new-sql-over-permissive
  expected: after a same-token/different-sql replay, DescribeStatement QueryString == the SECOND sql (with Status/CreatedAt/UpdatedAt/ResultRows frozen at the first execution)
  actual:   AssertionError: the record must now MISREPORT itself as the newly submitted sql; got "insert into sim2real_behav_tok_b922f093de16 values ('first')"

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json, awsJson1_1): ResultFormatString values [JSON, CSV]; GetStatementResult errors include ResourceNotFoundException (404) and ValidationException (400); ListSessions bound+paginated with Sessions @required; ExecuteStatementInput.ClientToken @idempotencyToken length 1..64; ExecuteStatementInput.Parameters @length(min 1) only.

## Steps to Reproduce
1. Clone the repo, boot the emulator, provision a Redshift Serverless namespace + workgroup, and point a botocore redshift-data client at the emulator.
2. Cause 1: execute_statement(..., Sql='select 1', ResultFormat='CSV'), poll to FINISHED, then describe_statement -> expect ResultFormat 'csv'; observe 'CSV'. Repeat with no ResultFormat -> expect 'json'; observe 'JSON'.
3. Cause 2: execute_statement(Sql='CREATE TABLE t1 (id INT)'), poll to FINISHED (HasResultSet false), then get_statement_result(Id=...) -> expect ResourceNotFoundException 'Query does not have result. Please check query status with DescribeStatement.'; observe HTTP 200 with empty Records.
4. Cause 3: execute_statement(..., SessionKeepAliveSeconds=90), take SessionId, then list_sessions(SessionId=<id>, Status='AVAILABLE') -> expect ValidationException; observe NotImplemented. list_sessions(SessionId=<id>) alone is equally unserved.
5. Cause 4: execute_statement(Sql='select id from t where id = :i', Parameters=[{'name':'zzz','value':'1'}]) -> expect 400 ValidationException; observe 200/SUBMITTED. Repeat with an extra parameter and with a duplicated name for the other two messages.
6. Cause 5: execute_statement(Sql=A, ClientToken=T) and wait for FINISHED; execute_statement(Sql=B, ClientToken=T) -> same Id (correct); describe_statement(Id) -> expect QueryString == B; observe A.
7. Re-replay every check id in trace_ids to confirm the fix.
