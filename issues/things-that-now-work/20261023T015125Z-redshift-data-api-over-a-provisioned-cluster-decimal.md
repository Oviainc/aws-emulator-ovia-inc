Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.rdapi-statement-result-set-rows-and-column-metadata redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span redshift.data-api-execute-statement-typed-result-roundtrip redshift.data-api-batch-execute-statement-substatement-ordering redshift.rdapi-batch-substatements-are-individually-reportable redshift.data-api-sql-failure-surfaces-as-async-failed-status redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning
Issue-Key: redshift.rdapi-statement-result-set-rows-and-column-metadata|run#19|AssertionError|assert records == _EXPECTED_RECORDS, (
Issue-Key: redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span|run#22|AssertionError|assert members == [
Issue-Key: redshift.data-api-execute-statement-typed-result-roundtrip|run#34|AssertionError|assert columns[1]["precision"] == 32, (
Issue-Key: redshift.data-api-batch-execute-statement-substatement-ordering|run#25|AssertionError|assert second["Id"] == batch_id, (
Issue-Key: redshift.rdapi-batch-substatements-are-individually-reportable|_error#11|AssertionError|raise AssertionError("the call SUCCEEDED where real AWS rejects it")
Issue-Key: redshift.data-api-sql-failure-surfaces-as-async-failed-status|run#23|AssertionError|assert result_code == "ResourceNotFoundException", (
Issue-Key: redshift|ListSessions|NotImplemented
Issue-Key: redshift|DescribeTable|NotImplemented
Issue-Commit: 8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6

# Redshift Data API over a provisioned cluster: DECIMAL rendered as doubleValue, ColumnMetadata.precision always 0, batch sub-statement id identity wrong, parent-id result read accepted, FAILED-statement read mis-coded, ListSessions/DescribeTable NotImplemented (7 causes)

> _Note: this issue was identified against the code base at commit `8087bf5e6100a6c9af7468e05f3ad1b7d1320fd6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStatementResult

## Description
Companion to the redshift cluster SQL-engine issue filed from the same replay. These seven causes are in the Data API's response/validation surface over a provisioned cluster rather than in the SQL engine: the checks got far enough to read a result set or an error, and the shape of what came back diverges. This bundle carries SEVEN causes; all seven must be closed before submitting.

=== Cause 1 of 7: DECIMAL/NUMERIC values are returned as doubleValue instead of stringValue ===
AWS: Smithy (com-amazonaws-redshiftdata-redshiftdata): Field is a union of isNull | booleanValue | longValue | doubleValue | stringValue | blobValue, so the discriminator a value is rendered under is the service's choice and is the observable contract. Recorded behavior: Redshift never renders an exact-numeric column as a lossy JSON number — a DECIMAL/NUMERIC column comes back as stringValue carrying the value with its declared scale (e.g. '10.50', '-3.25'), alongside int -> longValue, boolean -> booleanValue, and VARCHAR/DATE/TIMESTAMP/TIMESTAMPTZ -> stringValue.
Emulator: DECIMAL columns are rendered as doubleValue (10.5, -3.25), i.e. under the floating-point discriminator.
Why: a client reading a Field union must switch on the member name. Code written against AWS reads stringValue for a DECIMAL and finds it absent (or crashes on the unexpected member), and even when it tolerates the member it has lost the exact decimal: scale is dropped ('10.50' becomes 10.5) and the value has been round-tripped through binary floating point, which is exactly what AWS avoids for monetary/exact-numeric columns.
Fix: the Data API result serializer's SQL-type -> Field-member mapping (the GetStatementResult/GetStatementResultV2 record projection) — map exact-numeric types to stringValue with the column's declared scale, leaving doubleValue for FLOAT/REAL/DOUBLE PRECISION.
Traces: redshift.rdapi-statement-result-set-rows-and-column-metadata, redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span.

=== Cause 2 of 7: ColumnMetadata.precision is 0 for a non-numeric column instead of the column length ===
AWS: Smithy: ColumnMetadata.precision is documented as 'The precision value of a decimal number column, or the column length for a non-numeric column' (Integer, default 0). Recorded behavior: for a VARCHAR(32) column, GetStatementResult's ColumnMetadata reports precision 32.
Emulator: precision is 0 for a VARCHAR(32) column — the modelled default is emitted instead of the declared width.
Why: ColumnMetadata is how a Data API client builds a result schema without a JDBC connection. A driver, ORM or dataframe loader sizing a string column from precision gets 0, so it either errors or silently widens/truncates; nothing else in the response carries the declared width.
Fix: the Data API ColumnMetadata projection — populate precision from the column's declared length for non-numeric types (and from the numeric precision, with scale, for exact-numeric types) instead of leaving the default.
Traces: redshift.data-api-execute-statement-typed-result-roundtrip.

=== Cause 3 of 7: DescribeStatement on a batch sub-statement id echoes the suffixed address as the record's Id ===
AWS: Smithy: DescribeStatementResponse.Id is @required UUID with @pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$, and SubStatementData.Id documents the suffix as positional — 'A suffix indicates the number of the SQL statement. For example, d9b6c0c9-...:2 has a suffix of :2 that indicates the second SQL statement of a batch query.' Recorded behavior: DescribeStatement(Id='<batch-uuid>:2') is accepted and answers with that sub-statement's own status/duration, but the Id field it reports is the PARENT batch uuid — the ':n' suffix is an address used to reach the sub-statement, not the identity it reports.
Emulator: DescribeStatement(Id='<batch-uuid>:2') reports Id '<batch-uuid>:2'.
Why: a client that walks DescribeStatement's SubStatements and then describes each sub-statement uses the returned Id to correlate the record back to the batch it belongs to. Echoing the address breaks that correlation and makes a sub-statement look like an independent top-level statement.
Fix: the DescribeStatement handler's statement-record projection — when the requested id carries a ':n' suffix, resolve the sub-statement but report the parent batch id in the Id member.
Traces: redshift.data-api-batch-execute-statement-substatement-ordering.

=== Cause 4 of 7: GetStatementResult on a batch PARENT id succeeds instead of being refused ===
AWS: Smithy: GetStatementResult models ValidationException (client, @httpError 400) and ResourceNotFoundException (client, 404); BatchExecuteStatement returns one parent Id whose results live on the per-sub-statement ':n' ids. Recorded behavior: reading results from the parent id of a batch is refused with error code ValidationException — results are only readable per sub-statement.
Emulator: the call SUCCEEDS on the parent batch id (the check's error helper reported 'the call SUCCEEDED where real AWS rejects it').
Why: over-permissive. A client that mistakenly reads the parent id is handed a result set AWS would never return, so it silently consumes the wrong (or an empty) answer instead of being told to address the sub-statements, and the per-sub-statement reporting contract is unenforced.
Fix: the GetStatementResult handler's statement lookup — when the resolved statement is a batch parent, raise the 400 ValidationException instead of serving a result page.
Traces: redshift.rdapi-batch-substatements-are-individually-reportable.

=== Cause 5 of 7: GetStatementResult on a FAILED statement answers ValidationException instead of ResourceNotFoundException ===
AWS: Smithy: GetStatementResult models both ResourceNotFoundException (client, @httpError 404) and ValidationException (client, 400). Recorded behavior (measured ra3.large, us-west-2): the refusal for a statement that produced no result — including a FAILED statement that demonstrably exists and that DescribeStatement reports — is ResourceNotFoundException; the Data API models 'there is no result to fetch' as a missing RESULT resource, not as a request-validation error.
Emulator: the read is refused, but with error code ValidationException (example message: 'Query 7a68bf58-... did not finish successfully; its status is FAILED').
Why: the error code, and the 404-vs-400 status it maps to, are the requirement. A client distinguishing 'this statement's result is not available' (retry/branch to DescribeStatement) from 'my request was malformed' (never retry) takes the wrong branch, and SDK error mappers and retry policies classify 400 and 404 differently.
Fix: the GetStatementResult/GetStatementResultV2 handler's no-result path — raise the 404-mapped ResourceNotFoundException for FAILED/ABORTED and for FINISHED-with-no-result-set statements, via one shared constructor so the readers cannot drift. Note the related open issue 20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md, whose Cause 2 covers the FINISHED-DDL variant of this same path on the serverless entry point (there the emulator answers 200 with an empty result set); it does not cover this FAILED-statement path or this service.
Traces: redshift.data-api-sql-failure-surfaces-as-async-failed-status.

=== Cause 6 of 7: ListSessions answers NotImplemented ===
AWS: Smithy: ListSessions is a bound, @readonly, paginated operation (input NextToken / MaxResults 0..100 / SessionId / Status / RoleLevel / ClusterIdentifier / WorkgroupName / Database; output Sessions @required plus NextToken) whose modelled errors are ValidationException (400), ResourceNotFoundException (404) and InternalServerException (500). Recorded behavior: the live sessions created by ExecuteStatement with SessionKeepAliveSeconds are listable, and SessionId combined with any other filter is refused ValidationException.
Emulator: HTTP error code 'NotImplemented' — 'operation ListSessions is scaffolded but not implemented in this emulator'. The suite did NOT mark this check as an already-filed scaffold stub (it is reported as a plain [error] with no stub annotation), so nothing covers it on this service.
Why: a client cannot enumerate or inspect its Data API sessions at all; the session-scoped transaction model (two sessions, one of which must not see the other's uncommitted rows) is unobservable, and the mutually-exclusive-filter contract is unenforced.
Fix: implement the redshift-data ListSessions handler over the emulator's session state (services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/list_sessions.rs), including the SessionId-excludes-every-other-filter validation. The related open issue 20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md records the same stub from the serverless side (its Cause 3); this check is a second, unowned grounding of it on the provisioned-cluster path.
Traces: redshift.data-api-session-transaction-hides-uncommitted-rows-from-a-second-session.

=== Cause 7 of 7: DescribeTable answers NotImplemented ===
AWS: Smithy: DescribeTable is a bound, @readonly, paginated operation (input Database @required, ClusterIdentifier/WorkgroupName/SecretArn/DbUser, ConnectedDatabase, Schema, Table, NextToken, MaxResults 0..1000; output TableName + ColumnList + NextToken) with modelled errors ValidationException (400), ResourceNotFoundException (404), QueryTimeoutException (400), DatabaseConnectionException (500) and InternalServerException (500). Recorded behavior: it returns the table's column list for a table that exists in the named database/schema.
Emulator: HTTP error code 'NotImplemented' — 'operation DescribeTable is scaffolded but not implemented in this emulator'. Again not marked by the suite as an already-filed stub, and no open issue names DescribeTable.
Why: the Data API's metadata path is unusable — a client cannot discover a table's columns without issuing SQL, and anything that re-reads a table definition (a snapshot/re-read flow, a BI schema refresh) fails outright.
Fix: implement the redshift-data DescribeTable handler over the emulator's catalog state (services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/describe_table.rs), with pagination over ColumnList and the 404 for an absent table.
Traces: redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning.

## Evidence
Suite: simparity-behavioral-aws-v1. redshift 2/29 (7%), 27 failed.

Cause 1 — rdapi-statement-result-set-rows-and-column-metadata
  expected: [[{'longValue': 1}, {'stringValue': 'alpha'}, {'stringValue': '10.50'}, {'booleanValue': True}, {'stringValue': '2026-09-22 10:11:12'}], [{'longValue': 2}, {'isNull': True}, {'stringValue': '-3.25'}, {'booleanValue': False}, {'stringValue': '2026-01-02 03:04:05'}]]
  actual:   the DECIMAL cells came back as {'doubleValue': 10.5} and {'doubleValue': -3.25}
  data-api-temporal-types-round-trip-...: expected discriminators int -> longValue and TIMESTAMP/TIMESTAMPTZ/DATE/DECIMAL/VARCHAR all -> stringValue; actual ['longValue', 'stringValue', 'stringValue', 'stringValue', 'doubleValue', 'stringValue']

Cause 2 — data-api-execute-statement-typed-result-roundtrip
  expected: ColumnMetadata precision 32 for a VARCHAR(32) column
  actual:   precision 0 ('VARCHAR(32) must report its declared width 32 in precision, got 0')

Cause 3 — data-api-batch-execute-statement-substatement-ordering
  expected: describe_statement(Id='26a13264-f2ee-4d00-8fb6-ea1dbfb82834:2')['Id'] == '26a13264-f2ee-4d00-8fb6-ea1dbfb82834'
  actual:   '26a13264-f2ee-4d00-8fb6-ea1dbfb82834:2'

Cause 4 — rdapi-batch-substatements-are-individually-reportable (check line 81)
  expected: get_statement_result(Id=<batch parent id>) -> ClientError, code ValidationException
  actual:   the call SUCCEEDED where real AWS rejects it

Cause 5 — data-api-sql-failure-surfaces-as-async-failed-status
  expected: GetStatementResult on a FAILED statement refused with ResourceNotFoundException
  actual:   ValidationException ('Query 7a68bf58-9941-473e-99b4-ca0efd6a62c3 did not finish successfully; its status is FAILED')

Cause 6 — data-api-session-transaction-hides-uncommitted-rows-from-a-second-session
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSessions operation: operation ListSessions is scaffolded but not implemented in this emulator

Cause 7 — data-api-statement-result-is-a-snapshot-reread-without-rerunning
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeTable operation: operation DescribeTable is scaffolded but not implemented in this emulator

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json, awsJson1_1):
  Field = union{isNull, booleanValue, longValue, doubleValue, stringValue, blobValue}
  ColumnMetadata.precision -> Integer, default 0, doc 'The precision value of a decimal number column, or the column length for a non-numeric column.'
  DescribeStatementResponse.Id -> UUID, required, @pattern ^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$ ; SubStatementData.Id documents the ':n' suffix as the batch position
  GetStatementResult errors: ValidationException (client, 400), ResourceNotFoundException (client, 404), ActiveWaitingRequestsExceededException (400), InternalServerException (500)
  ListSessions: bound, readonly, paginated(items=Sessions), output Sessions @required
  DescribeTable: bound, readonly, paginated(items=ColumnList), input Database @required

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. CreateCluster a single-node cluster, wait for ClusterStatus 'available', and point a botocore redshift-data client at the emulator.
3. Cause 1/2: execute_statement(Sql='CREATE TABLE t (id INT, name VARCHAR(32), amt DECIMAL(10,2), flag BOOLEAN, ts TIMESTAMP)'), insert a row with amt 10.50, poll to FINISHED, then get_statement_result(Id=...). Expect the amt cell as {'stringValue': '10.50'} and ColumnMetadata precision 32 for name; observe {'doubleValue': 10.5} and precision 0.
4. Cause 3: batch_execute_statement(Sqls=[a, b, c]), poll the parent to FINISHED, then describe_statement(Id='<parent>:2'). Expect the response Id to be the parent uuid; observe '<parent>:2'.
5. Cause 4: with that same batch, get_statement_result(Id='<parent>') (no suffix). Expect ValidationException; observe a successful result page.
6. Cause 5: execute_statement with SQL that fails (e.g. SELECT from a missing table), poll to FAILED, then get_statement_result(Id=...). Expect ResourceNotFoundException (404-mapped); observe ValidationException.
7. Cause 6: execute_statement(..., SessionKeepAliveSeconds=90), then list_sessions(SessionId=<id>). Expect the session (and a ValidationException when SessionId is combined with another filter); observe NotImplemented.
8. Cause 7: describe_table(ClusterIdentifier=<cluster>, Database='devdb', Schema='public', Table='t'). Expect TableName + ColumnList; observe NotImplemented.
9. Re-replay every check id in trace_ids to confirm a fix.
