Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.data-api-execute-statement-typed-result-roundtrip redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span redshift.rdapi-statement-result-set-rows-and-column-metadata redshift.data-api-batch-execute-statement-substatement-ordering redshift.rdapi-batch-substatements-are-individually-reportable redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning
Issue-Key: redshift.data-api-execute-statement-typed-result-roundtrip|run#34|AssertionError|assert columns[1]["precision"] == 32, (
Issue-Key: redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span|run#27|AssertionError|assert [c.get("scale") for c in columns] == [0, 6, 6, 0, _NUMERIC_SCALE, 0], (
Issue-Key: redshift.rdapi-statement-result-set-rows-and-column-metadata|run#22|AssertionError|assert page["ColumnMetadata"] == _EXPECTED_COLUMN_METADATA, (
Issue-Key: redshift.data-api-batch-execute-statement-substatement-ordering|run#26|AssertionError|assert "QueryString" not in second, (
Issue-Key: redshift.rdapi-batch-substatements-are-individually-reportable|run#14|AssertionError|assert message == _PARENT_REJECTION, f"unexpected rejection message: {message!r}"
Issue-Key: redshift|DescribeTable|ResourceNotFoundException
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift Data API statement records and result metadata diverge: ColumnMetadata precision/length/scale/nullable/schemaName, batch records carry QueryString, wrong batch-parent refusal text, DescribeTable 404s an absent table (4 causes)

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStatementResult

## Description
Companion to the cluster-SQL-engine issue filed from the same replay. These FOUR causes are in the Data API's response projection (statement records, result metadata, catalog reporting) for statements run against a provisioned Redshift cluster; the SQL itself executed. This bundle carries FOUR causes; all four must be closed before submitting. Not covered by the open issues 20261022T180846Z-redshift-data-data-api-surface-diverges-resultformat-echoed.md (ResultFormat case, empty-result read, ListSessions, parameter validation, ClientToken) or 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md — none of them names ColumnMetadata, batch-record projection or DescribeTable.

=== Cause 1 of 4: ColumnMetadata is projected wrongly — precision/length swapped, no scale for TIMESTAMP, nullable always 1, schemaName/tableName omitted ===
AWS: Smithy (com.amazonaws.redshiftdata#ColumnMetadata): `precision` is documented as 'The precision value of a decimal number column, or the column length for a non-numeric column', `length` as 'The length of the column', `scale` as 'The scale value of a decimal number column', `nullable` as 'A value that indicates whether the column is nullable' (Integer, default 0), and `schemaName` / `tableName` are modeled members ('The name of the schema that contains the table that includes the column' / 'The name of the table that includes the column'). Recorded behavior (all three checks are marked MEASURED against real Redshift, ra3.large us-west-2): the declared character width is carried in `precision` and `length` is 0 for EVERY column (int4, a CAST('x' AS VARCHAR(32)) expression column, an unbounded varchar, and a table column declared VARCHAR(32) alike); int4 reports precision 10, bool precision 1, timestamp precision 29 with scale 6, DECIMAL(9,2) precision 9 scale 2, TIMESTAMP/TIMESTAMPTZ scale 6 (microsecond fractional-seconds precision); a column declared NOT NULL reports nullable 0; and every column of a result drawn from a table carries schemaName ('public') and tableName (the table).
Emulator: GetStatementResult.ColumnMetadata reports the width in `length` and leaves `precision` 0 for expression columns (a CAST to VARCHAR(32) reports precision 0, length 0 expected 32/0; a table varchar reports precision 32 AND length 32 where AWS reports 32/0); `scale` is 0 for TIMESTAMP and TIMESTAMPTZ instead of 6; `nullable` is 1 for a column declared NOT NULL; `precision` is 0 for int4 (expected 10), bool (expected 1) and timestamp (expected 29); and `schemaName` and `tableName` are absent from every column.
Why: ColumnMetadata is what JDBC/ODBC-style and generic consumers use to build result readers: a column whose width is read from `length` (0 on AWS) or whose declared NOT NULL is reported nullable binds the wrong type, mis-sizes buffers and accepts nulls it should reject; missing schemaName/tableName breaks any consumer that qualifies columns by origin; a timestamp with scale 0 loses the fractional-second precision claim.
Fix: the redshift-data result/catalog metadata projection that builds ColumnMetadata (the ColumnMetadata construction in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs, shared by GetStatementResult and DescribeTable) — derive precision from the declared/inferred type (character width, numeric precision, 10 for int4, 1 for bool, 29 for timestamp), leave length 0, set scale from the type (6 for timestamp/timestamptz, the declared scale for decimal), set nullable 0 for NOT NULL columns, and populate schemaName/tableName for columns that come from a relation.
Traces: redshift.data-api-execute-statement-typed-result-roundtrip, redshift.data-api-temporal-types-round-trip-and-the-engine-computes-their-span, redshift.rdapi-statement-result-set-rows-and-column-metadata.

=== Cause 2 of 4: DescribeStatement on a '<batch-id>:<n>' address returns a batch record carrying a top-level QueryString ===
AWS: Smithy: DescribeStatementResponse.QueryString is an OPTIONAL member ('The SQL statement text'), while SubStatements ('The SQL statements from a multiple statement run') carries SubStatementData entries each with their own required Id matching the pattern '...(:\d{0,2})?' and their own QueryString. Recorded behavior (MEASURED against the real Data API, ra3.large us-west-2): DescribeStatement(Id='<batch-id>:2') returns the PARENT batch record — Id is the bare batch id, there is NO QueryString member at all (a batch has no single query) — with SubStatements NARROWED to exactly the addressed position, so the n-th SQL is read at SubStatements[0].QueryString.
Emulator: the same call returns the parent Id correctly but POPULATES the top-level QueryString with the addressed sub-statement's SQL.
Why: QueryString's presence is how a client tells a single-statement record from a batch record; a batch record that reports one of its SQLs as 'the' statement text makes a batch indistinguishable from a single statement and mis-reports what the parent ran. A consumer reading QueryString and ignoring SubStatements silently believes a 3-SQL batch was one INSERT.
Fix: the redshift-data DescribeStatement handler/serializer for batch statements — omit QueryString on a batch (parent) record and express the ':n' address only by narrowing SubStatements to that position.
Traces: redshift.data-api-batch-execute-statement-substatement-ordering.

=== Cause 3 of 4: GetStatementResult on a batch PARENT id is refused with the wrong message ===
AWS: Smithy: GetStatementResult models ValidationException (client, httpError 400). Recorded behavior (MEASURED, single-node ra3.large us-west-2): reading a batch's result from the parent id is refused ValidationException / HTTP 400 with the fixed message 'BatchExecuteStatement result can only be retrieved with sub-statement id.' (entirely fixed text — it carries no values from the request).
Emulator: the code (ValidationException) and status (400) are right, but the message is 'Query is a batch statement; fetch the results of each SQL statement of the batch with its own Id, which is this Id with a :<n> suffix'.
Why: the error message is part of the contract clients log, match and assert on; a reworded message breaks message-matching clients and every conformance assertion, and it is the only signal distinguishing this refusal from the other ValidationExceptions GetStatementResult raises.
Fix: the redshift-data GetStatementResult handler's batch-parent refusal — emit AWS's fixed message verbatim.
Traces: redshift.rdapi-batch-substatements-are-individually-reportable.

=== Cause 4 of 4: DescribeTable on a table the catalog does not hold answers ResourceNotFoundException instead of 200 with an empty ColumnList ===
AWS: Smithy: DescribeTable returns DescribeTableResponse with an OPTIONAL ColumnList ('A list of columns in the table') and models ResourceNotFoundException (client, httpError 404). Recorded behavior: DescribeTable naming a Schema/Table that does not exist in an existing database is a SUCCESS — HTTP 200 with an EMPTY ColumnList — which is what lets a caller prove a table is gone (both the snapshot check and the static-parameter check assert ColumnList == []); the 404 is for a missing warehouse/database, not for an absent table.
Emulator: ResourceNotFoundException with message 'Table does not exist: public.<table>' — services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/describe_table.rs:362-364 refuses when `described.is_empty() && table.is_some()`, via `no_such_table` at lines 409-416.
Why: a client probing for a table's existence gets an exception where AWS returns an empty, successful answer; any code path that treats 404 as fatal (or that distinguishes 'database absent' from 'table absent' by the error) diverges, and the check cannot even verify that a DROP TABLE took effect.
Fix: the redshift-data DescribeTable handler — return 200 with an empty ColumnList (and no TableName) when the named database exists but the named schema/table matches nothing, reserving ResourceNotFoundException for an absent database/warehouse.
Traces: redshift.data-api-statement-result-is-a-snapshot-reread-without-rerunning (the terminal DescribeTable after DROP TABLE); the same absent-table read also appears in the OWNED static-parameter check.

## Evidence
Suite: simparity-behavioral-aws-v1 — redshift 5/28 (18%), 23 failed.

Cause 1 — check_redshift_rdapi_statement_result_set_rows_and_column_metadata.py:120, for CREATE TABLE t (id INT NOT NULL, name VARCHAR(32), amt DECIMAL(9,2), ok BOOLEAN, ts TIMESTAMP):
  expected (measured): id {nullable:0, precision:10, scale:0, length:0, schemaName:'public', tableName:'<t>', typeName:'int4'}; name {nullable:1, precision:32, length:0, schemaName/tableName set}; amt {precision:9, scale:2, length:0}; ok {precision:1, length:0}; ts {precision:29, scale:6, length:0}
  actual:   id {nullable:1, precision:0, scale:0, length:0, NO schemaName/tableName}; name {precision:32, length:32}; amt {precision:9, scale:2, length:9}; ok {precision:0, length:0}; ts {precision:0, scale:0, length:0}
  check_redshift_data_api_execute_statement_typed_result_roundtrip.py:304, for SELECT 1 AS n, CAST('sim2real' AS VARCHAR(32)) AS s, CAST(NULL AS INTEGER) AS z, CAST('s2r' AS VARCHAR(8)) AS w:
  expected: s.precision 32, w.precision 8, every length 0
  actual:   AssertionError: VARCHAR(32) must report its declared width 32 in precision, got 0
  check_redshift_data_api_temporal_types_round_trip_and_the_engine_computes_their_span.py:504:
  expected: scale == [0, 6, 6, 0, 4, 0] (int4/date/varchar 0, TIMESTAMP and TIMESTAMPTZ 6, DECIMAL 4)
  actual:   [0, 0, 0, 0, 4, 0]

Cause 2 — check_redshift_data_api_batch_execute_statement_substatement_ordering.py:292, after BatchExecuteStatement(Sqls=[CREATE TABLE..., INSERT...'first', INSERT...SELECT...]):
  expected: describe_statement(Id='<batch-id>:2') -> Id == '<batch-id>', NO 'QueryString' member, SubStatements narrowed to [{'Id': '<batch-id>:2', 'QueryString': <2nd sql>}]
  actual:   AssertionError: a batch record has no single QueryString; got "INSERT INTO <table> VALUES (1, 'first')"

Cause 3 — check_redshift_rdapi_batch_substatements_are_individually_reportable.py:86:
  expected: get_statement_result(Id=<parent batch id>) -> ValidationException / 400 / 'BatchExecuteStatement result can only be retrieved with sub-statement id.'
  actual:   ValidationException / 400 / 'Query is a batch statement; fetch the results of each SQL statement of the batch with its own Id, which is this Id with a :<n> suffix'

Cause 4 — check_redshift_data_api_statement_result_is_a_snapshot_reread_without_rerunning.py (terminal leg, after DROP TABLE <t>):
  expected: describe_table(ClusterIdentifier=..., Database='devdb', Schema='public', Table='<t>') -> HTTP 200 with ColumnList == []
  actual:   botocore ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeTable operation: Table does not exist: public.<t>
  emulator source: services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/describe_table.rs:362-364 and :409-416

(table names, cluster ids and generated suffixes are per-run examples; the requirements are the metadata values each declared/inferred type must report, the absence of QueryString on a batch record, AWS's fixed refusal message, and the 200/empty-ColumnList answer.)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ColumnMetadata members isCaseSensitive/isCurrency/isSigned/label/name/nullable(Integer, default 0)/precision/scale/schemaName/tableName/typeName/length/columnDefault with the docs quoted above; DescribeStatementResponse.QueryString optional + SubStatements(SubStatementList); SubStatementData.Id @required with pattern '^[a-z0-9]{8}(-[a-z0-9]{4}){3}-[a-z0-9]{12}(:\d{0,2})?$'; GetStatementResult errors ValidationException(400)/ResourceNotFoundException(404); DescribeTableResponse.ColumnList optional, DescribeTable errors include ResourceNotFoundException(404).

## Steps to Reproduce
1. Clone the repo, boot the emulator, CreateCluster a single-node Redshift cluster, wait for 'available', and point a botocore redshift-data client at it (ClusterIdentifier + DbUser + Database).
2. Cause 1: execute_statement(Sql="CREATE TABLE m (id INT NOT NULL, name VARCHAR(32), amt DECIMAL(9,2), ok BOOLEAN, ts TIMESTAMP)"), insert a row, then execute_statement(Sql="SELECT id, name, amt, ok, ts FROM m") and get_statement_result -> expect ColumnMetadata with precision 10/32/9/1/29, scale 0/0/2/0/6, length 0 for all five, nullable 0 for id, and schemaName 'public' + tableName 'm' on every column; observe width in length, precision 0 for int4/bool/timestamp, scale 0 for timestamp, nullable 1 for id, no schemaName/tableName. Repeat with SELECT CAST('x' AS VARCHAR(32)) -> expect precision 32/length 0; observe precision 0.
3. Cause 2: batch_execute_statement(Sqls=[A,B,C]), wait FINISHED, then describe_statement(Id='<batch-id>:2') -> expect no QueryString member and SubStatements narrowed to position 2; observe a top-level QueryString holding B.
4. Cause 3: get_statement_result(Id='<batch-id>') -> expect 400 ValidationException 'BatchExecuteStatement result can only be retrieved with sub-statement id.'; observe the emulator's own batch-suffix wording.
5. Cause 4: execute_statement(Sql='CREATE TABLE s (id INT)'), then 'DROP TABLE s', then describe_table(Database='devdb', Schema='public', Table='s') -> expect HTTP 200 with ColumnList == []; observe ResourceNotFoundException 'Table does not exist: public.s'. Same for a table that never existed.
6. Re-replay every check id in trace_ids to confirm the fix.
