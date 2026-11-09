Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftdata-serverless-workgroup-sql-round-trip
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftdata-serverless-workgroup-sql-round-trip
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data|BatchExecuteStatement|2xx->2xx|$.DbUser
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.DbUser
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.SubStatements[*].RedshiftQueryId
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.SubStatements[*].ResultSize
Issue-Key: redshift-data|ExecuteStatement|2xx->2xx|$.DbUser
Issue-Key: redshift-data|DescribeStatement|2xx->2xx|$.ResultSize
Issue-Key: redshift-data|GetStatementResult|2xx->2xx|$.ColumnMetadata[*].length
Issue-Key: redshift-data|GetStatementResult|2xx->2xx|$.ColumnMetadata[*].nullable
Issue-Key: redshift-data|GetStatementResult|2xx->2xx|$.ColumnMetadata[*].precision
Issue-Key: redshift-data|GetStatementResult|2xx->2xx|$.ColumnMetadata[*].schemaName
Issue-Key: redshift-data|GetStatementResult|2xx->2xx|$.ColumnMetadata[*].tableName
Issue-Commit: 7e082a9dfef7650059a714e1b731386462df20dc

# Redshift Data API serverless-workgroup statement responses diverge: DbUser omitted, ResultSize/RedshiftQueryId miscomputed, ColumnMetadata wrong (5 causes)

> _Note: this issue was identified against the code base at commit `7e082a9dfef7650059a714e1b731386462df20dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStatement

## Description
Replay of the wire corpus `simparity-wire-aws-v1` against the emulator (84 steps, 74 passed, 6 divergence clusters) found that EVERY redshift-data statement response in the serverless-workgroup SQL round trip answers HTTP 200 with a body that diverges from the recording in its statement-identity and statement-counter members. All five causes below are in the redshift-data response projection (the statement record and the result metadata) for statements submitted against a SERVERLESS WORKGROUP target; the SQL itself ran and the status/HTTP code are right in every case, so nothing here is a transport or routing problem.

This bundle carries FIVE causes (Cause 1 of 5 .. Cause 5 of 5); all five must be closed before submitting. Related but NOT covering issues: the open behavioral issue 20261029T193810Z-redshift-data-api-statement-records-and-result-metadata.md describes the same ColumnMetadata projection defect for a PROVISIONED cluster target and carries no wire replay key for these fixture steps (Cause 5 here is the wire-keyed occurrence on a serverless workgroup and must be re-replayed against this fixture); 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md is about the SQL engine's statement support, not response projection. The sixth divergence in this replay (redshiftdata-target-selection-validation#task#2, the mutual-exclusivity ValidationException wording) is already OWNED by 20261105T195000Z-redshift-data-executestatement-target-validation-reports-the-mutual.md and is deliberately not re-filed here.

=== Cause 1 of 5: DbUser is absent from ExecuteStatement, BatchExecuteStatement and DescribeStatement responses for a statement run against a serverless workgroup ===
AWS: Smithy (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): `ExecuteStatementOutput.DbUser` (String), `BatchExecuteStatementOutput.DbUser` (String) and `DescribeStatementResponse.DbUser` (String) are all modeled members, documented as 'The database user name.' Recorded behavior: for a submission that names a `WorkgroupName` and carries NO `DbUser` and NO `SecretArn` in the request, the Data API STILL reports `DbUser` in the submit response and in every later `DescribeStatement` of that statement — the database user the statement actually ran as, which the service derives from the authenticated caller rather than echoing from the request (the fixture asserts the member's PRESENCE and shape, not a particular value, because the value is identity-derived and per-environment). Hypothesis: the reported user is the IAM-derived database user (an `IAMR:<role-name>` / `IAM:<user-name>` style name for a signed caller), which is why it exists even when the request names none.
Emulator: `DbUser` is MISSING from the response body of all three operations. The emulator stores and reports only the request's own `DbUser`: the submission carries `db_user: submission.db_user.clone()` (state.rs) and each reader reports `db_user: statement.db_user.clone()` (describe_statement.rs), so a serverless-workgroup submission that named no DbUser has none to report and the member is omitted entirely.
Why: `DbUser` is how a client (and any audit/attribution layer built on the Data API) learns which database identity a statement executed as. A consumer that reads `response['DbUser']` gets a KeyError / absent-field on every serverless-workgroup statement where AWS always supplies it, and code that correlates a statement back to a database principal silently loses the principal. Because the member is omitted on BOTH the submit and the describe path, there is no way for a caller to recover it.
Fix: the redshift-data connection/authorization resolution that builds a submission's `Connection` (`ConnectionRequest` -> `Connection` in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs) must resolve the database user for a submission that names neither `DbUser` nor `SecretArn` — deriving it from the signed caller's identity — so that the single stored `Statement.db_user` is populated and the ExecuteStatement, BatchExecuteStatement and DescribeStatement serializers all report `DbUser` without further change.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#9 (BatchExecuteStatement), #task#11 (ExecuteStatement), #task#10 and #task#12 (DescribeStatement).

=== Cause 2 of 5: a FINISHED statement that produced no result set reports ResultSize -1 instead of 0 ===
AWS: Smithy: `DescribeStatementResponse.ResultSize` and `SubStatementData.ResultSize` are Long with `default: 0`, documented 'The size in bytes of the returned results. A -1 indicates the value is null.' Recorded behavior: for each sub-statement of a batch that FINISHED without returning rows (DDL/DML — the recorded batch's statements), `SubStatements[n].ResultSize` is 0, i.e. the null marker -1 is NOT what a successful no-result statement reports; -1 is reserved for the case where the value really is null.
Emulator: every sub-statement of the batch reports `ResultSize: -1`. The no-result success path hard-codes the null marker — `Ok(Produced::Affected(rows)) => Execution { ... result_size: -1, ... }` in state.rs (with the comment 'No result set, so the size is the modelled null marker') — and the batch parent's aggregate does the same (`result_size: -1`).
Why: -1 is a documented SENTINEL for 'value is null'. A client that sums or compares `ResultSize` across a batch's sub-statements (progress reporting, byte accounting, 'did this statement return anything' checks) reads -1 as a negative size or as 'unknown' where AWS reported a definite zero, so arithmetic over a batch produces negative totals and existence checks invert.
Emulator status/code: HTTP 200, no error — only the body value diverges.
Fix: the redshift-data statement-outcome recording in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs — a statement whose execution FINISHED with no result set must record ResultSize 0 (and the batch parent aggregate likewise), leaving -1 for the genuinely-null case.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#10 ($.SubStatements[0].ResultSize and $.SubStatements[1].ResultSize: expected 0, got -1).

=== Cause 3 of 5: a batch sub-statement reports an emulator-minted sequential RedshiftQueryId where the recording reports 0 ===
AWS: Smithy: `SubStatementData.RedshiftQueryId` is Long with `default: 0`, documented 'The identifier of the query generated by Amazon Redshift. These identifiers are also available in the query_id column of the SYS_QUERY_HISTORY system view.' Recorded behavior only: in the recorded serverless-workgroup batch, `SubStatements[0].RedshiftQueryId` is 0 — the member is at its modeled default, i.e. the service reported no cluster-side query id for that sub-statement.
Emulator: `SubStatements[0].RedshiftQueryId` is 1. The emulator mints its own monotonically increasing id for every sub-statement at submission time (`self.next_query_id += 1; ... redshift_query_id: self.next_query_id` in state.rs's batch build), so the value is a counter of statements this emulator process has accepted rather than anything the warehouse generated.
Why: `RedshiftQueryId` is the join key into SYS_QUERY_HISTORY. A client that treats a non-zero value as 'this statement has a warehouse-side query record' will go looking for a query id that does not exist, and a client that keys a cache or a correlation table on it gets ids that differ from the recorded contract and that restart from 1 with every emulator process.
Fix: the redshift-data submission-recording path in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs — report `RedshiftQueryId` from what the engine actually recorded for the sub-statement (the modeled default 0 when there is no warehouse-side query record) instead of a per-process sequence number.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#10 ($.SubStatements[0].RedshiftQueryId: expected 0, got 1).

=== Cause 4 of 5: ResultSize for a statement that DID return rows is computed with the wrong byte accounting ===
AWS: Smithy: `DescribeStatementResponse.ResultSize` — 'The size in bytes of the returned results.' Recorded behavior only: for the recorded SELECT over the three-column `gadget` relation the Data API reports `ResultSize` 46 for the identical result set.
Emulator: the same statement reports `ResultSize` 47 — one byte more for the same rows. The emulator derives the figure by summing a per-value size taken from the in-process SQLite storage class: `value_bytes()` in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs charges 0 for NULL, `size_of::<i64>()` (8) for any integer, `size_of::<f64>()` (8) for any real, and the raw byte length for text/blob, and that sum is returned as the statement's ResultSize. That is the size of the emulator's own storage representation, not the size of the result Redshift reports, so it agrees with AWS only by coincidence.
Why: `ResultSize` is what clients use to decide whether to page, to spool to disk, or to refuse a result, and it is asserted directly by conformance consumers. A size derived from the emulator's storage classes diverges per column type and per value, so a client calibrated against AWS's figure mis-sizes buffers and any equality assertion on the member fails.
Fix: the redshift-data result-size computation in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs (`value_bytes` and the `reported` accumulator feeding `Produced::Rows`) — size each value as Redshift sizes it for the column's DECLARED type rather than by the in-process storage class.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#12 ($.ResultSize: expected 46, got 47).

=== Cause 5 of 5: GetStatementResult.ColumnMetadata is projected wrongly — nullable always 1, precision 0 for integer columns, the declared width reported in length, and schemaName/tableName omitted ===
AWS: Smithy (ColumnMetadata): `nullable` (Integer, default 0) 'A value that indicates whether the column is nullable'; `precision` (Integer, default 0) 'The precision value of a decimal number column, or the column length for a non-numeric column'; `length` (Integer, default 0) 'The length of the column'; `schemaName` 'The name of the schema that contains the table that includes the column'; `tableName` 'The name of the table that includes the column'. Recorded behavior: for a result drawn from a relation, EVERY column carries `schemaName` (the containing schema) and `tableName` (the relation), `length` is 0 for every column — the declared width is carried in `precision`, not in `length` — an integer column reports its numeric precision in `precision` (10 for a 4-byte integer), and a column declared NOT NULL reports `nullable` 0.
Emulator: for the same result set, every column reports `nullable` 1 (even for NOT NULL columns), `schemaName` and `tableName` are ABSENT from every column, the character/binary width appears in `length` (32 for the varchar column, 8 for the third column) where AWS reports 0, and the integer column reports `precision` 0 where AWS reports 10. The metadata is built from the engine cursor alone (`column_metadata(name, column_type)` over the SQLite column list in state.rs), which knows neither the relation a column came from nor its declared nullability.
Why: ColumnMetadata is what generic result readers (JDBC/ODBC-shaped clients, data-frame loaders, code generators) bind types from. A width read out of `length` (0 on AWS) mis-sizes buffers, `precision` 0 for an integer makes the column untyped, `nullable` 1 for a NOT NULL column makes a reader accept nulls it should reject, and absent `schemaName`/`tableName` breaks every consumer that qualifies a column by its origin.
Fix: the redshift-data result metadata projection in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs (the `column_metadata` construction shared by GetStatementResult and DescribeTable) — carry the declared width in `precision` and leave `length` 0, derive `precision` for numeric types from the declared/inferred type, set `nullable` 0 for a NOT NULL column, and populate `schemaName`/`tableName` for columns that originate in a relation. This is the same projection named as Cause 1 of the open behavioral issue 20261029T193810Z-redshift-data-api-statement-records-and-result-metadata.md; that issue carries no wire key for this fixture, so this cause must be verified by re-replaying the step below.
Traces: redshiftdata-serverless-workgroup-sql-round-trip#task#13 (12 diverging ColumnMetadata keys across the three columns).

## Evidence
Replay summary (trace_set_id simparity-wire-aws-v1, emulator_up true, bootstrap_signal false): steps_total 84, steps_passed 74, steps_mismatched 10, steps_diverged 6, score_pct 88.1. All five causes below are verdict=mismatch at the CORRECT HTTP status (200), so the divergence is purely in the response body.

Cause 1 — fixture redshiftdata-serverless-workgroup-sql-round-trip:
  step task#9, BatchExecuteStatement, expected 200 / actual 200:
    $.DbUser: missing (expected a DbUser member, got None)
  step task#11, ExecuteStatement, expected 200 / actual 200:
    $.DbUser: missing (expected a DbUser member, got None)
  step task#10, DescribeStatement, expected 200 / actual 200:
    $.DbUser: missing (expected a DbUser member, got None)
  step task#12, DescribeStatement, expected 200 / actual 200:
    $.DbUser: missing (expected a DbUser member, got None)
  (the recording asserts the member's presence/shape, not a literal user name — the name is identity-derived and environment-specific, so the requirement is that DbUser is reported, not any one value)
  emulator source: src/ops/describe_statement.rs:236 `db_user: statement.db_user.clone()`; src/state.rs:3187 `db_user: submission.db_user.clone()`; src/state.rs:2255-2256 (`ConnectionRequest.db_user` is only the request's own member) and src/state.rs:2269-2270 (`Connection.db_user`).

Cause 2 — step task#10, DescribeStatement, 200/200:
    $.SubStatements[0].ResultSize: value_mismatch (expected 0, got -1)
    $.SubStatements[1].ResultSize: value_mismatch (expected 0, got -1)
  emulator source: src/state.rs:1492-1501 (`Produced::Affected` => `result_size: -1`) and src/state.rs:3246-3247 (batch aggregate `result_size: -1`); reported through `Statement::result_size()` at src/state.rs:784-790.

Cause 3 — step task#10, DescribeStatement, 200/200:
    $.SubStatements[0].RedshiftQueryId: value_mismatch (expected 0, got 1)
  emulator source: src/state.rs:2973-2982 (`self.next_query_id += 1; ... redshift_query_id: self.next_query_id` for each sub-statement) and src/state.rs:3200.

Cause 4 — step task#12, DescribeStatement, 200/200:
    $.ResultSize: value_mismatch (expected 46, got 47)
  emulator source: src/state.rs:1693-1703 (`value_bytes`: NULL 0, Integer size_of::<i64>() = 8, Real size_of::<f64>() = 8, Text/Blob byte length) accumulated at src/state.rs:1638/1648 and returned as the statement's ResultSize at src/state.rs:1684.

Cause 5 — step task#13, GetStatementResult, 200/200 (three-column result over the `gadget` relation in schema `public`):
    $.ColumnMetadata[0].nullable: value_mismatch (expected 0, got 1)
    $.ColumnMetadata[0].precision: value_mismatch (expected 10, got 0)
    $.ColumnMetadata[0].schemaName: missing (expected the containing schema, recorded 'public', got None)
    $.ColumnMetadata[0].tableName: missing (expected the containing relation, recorded 'gadget', got None)
    $.ColumnMetadata[1].length: value_mismatch (expected 0, got 32)
    $.ColumnMetadata[1].nullable: value_mismatch (expected 0, got 1)
    $.ColumnMetadata[1].schemaName / tableName: missing (recorded 'public' / 'gadget', got None)
    $.ColumnMetadata[2].length: value_mismatch (expected 0, got 8)
    $.ColumnMetadata[2].nullable: value_mismatch (expected 0, got 1)
    $.ColumnMetadata[2].schemaName / tableName: missing (recorded 'public' / 'gadget', got None)
  emulator source: src/state.rs:1668-1672 (`column_metadata(name, column_type)` built from the engine cursor only).
  (the recorded schema/relation names 'public'/'gadget' and the widths 32/8 are this fixture's values; the requirements are that length is 0 and the declared width lives in precision, that an integer column reports its numeric precision, that a NOT NULL column reports nullable 0, and that schemaName/tableName are reported for a column drawn from a relation)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): ExecuteStatementOutput/BatchExecuteStatementOutput/DescribeStatementResponse all model `DbUser` (String, 'The database user name.'); DescribeStatementResponse.ResultSize and SubStatementData.ResultSize are Long default 0 ('The size in bytes of the returned results. A -1 indicates the value is null.'); SubStatementData.RedshiftQueryId is Long default 0; ColumnMetadata members nullable(Integer, default 0)/precision/length/scale/schemaName/tableName/typeName with the documentation quoted above.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it (the /__admin/health endpoint answers 200 and the data listener accepts).
2. Create a redshift-serverless namespace + workgroup, then point a redshift-data client at it by WorkgroupName ONLY — no DbUser, no SecretArn (this is what fixture redshiftdata-serverless-workgroup-sql-round-trip does).
3. Cause 1: batch_execute_statement(WorkgroupName=..., Database=..., Sqls=[<DDL>, <DML>]) -> expect a `DbUser` member in the 200 response; observe it absent (fixture step task#9). Repeat with execute_statement(...) -> expect `DbUser`; observe absent (task#11). describe_statement on either Id -> expect `DbUser`; observe absent (task#10, task#12).
4. Cause 2: describe_statement(Id=<batch id>) once the batch has FINISHED -> expect SubStatements[*].ResultSize == 0 for the sub-statements that returned no rows; observe -1 for each (task#10).
5. Cause 3: in the same response -> expect SubStatements[0].RedshiftQueryId == 0; observe 1 (an emulator-minted per-process sequence number) (task#10).
6. Cause 4: execute_statement a SELECT over the three-column relation, wait FINISHED, describe_statement(Id=...) -> expect ResultSize 46 for that result set; observe 47 (task#12).
7. Cause 5: get_statement_result(Id=<that SELECT's Id>) -> expect every ColumnMetadata entry to carry schemaName/tableName, length 0 with the declared width in precision, precision 10 for the integer column, and nullable 0 for the NOT NULL columns; observe no schemaName/tableName, the width in length (32 and 8), precision 0 for the integer column, and nullable 1 everywhere (task#13).
8. Re-replay the wire fixture steps named in Issue-Trace/Issue-Step and in Issue-Trace-Ids (redshiftdata-serverless-workgroup-sql-round-trip task#9, task#10, task#11, task#12, task#13) to confirm all five causes are closed.
