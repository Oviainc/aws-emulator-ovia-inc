Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.json-and-csv-result-readers-are-mutually-exclusive
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.json-and-csv-result-readers-are-mutually-exclusive|run#18|AssertionError|assert v2_on_json[2] == (
Issue-Commit: 5efea226428cd2abb66c0a11f0d4b2dd794f4348

# redshift-data: the result-reader format-mismatch ValidationException carries the model's documentation sentence instead of AWS's refusal message (1 cause)

> _Note: this issue was identified against the code base at commit `5efea226428cd2abb66c0a11f0d4b2dd794f4348`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStatementResultV2

## Description
ONE cause. Not covered by any open issue: 20261022T180846Z-redshift-data-statement-execution-engine-is-not-redshift.md (SQL-engine compatibility), 20261029T193810Z-redshift-data-api-statement-records-and-result-metadata.md (ColumnMetadata / batch records / DescribeTable), 20261105T195000Z-redshift-data-executestatement-target-validation-reports-the-mutual.md (ExecuteStatement target validation) and 20261105T082606Z-redshift-data-listschemas-stub.md (ListSchemas) — none of them names the reader/format mutual-exclusivity refusal message.

=== Cause 1 of 1: GetStatementResultV2 on a JSON-format statement is refused with the wrong message text ===
AWS: Smithy (com.amazonaws.redshiftdata#GetStatementResultV2) models ValidationException, errorKind client, httpError 400 — and the emulator gets both of those right. The MESSAGE is the divergence. Recorded behavior (the behavioral check asserts it as measured against the real Data API): reading a statement whose ResultFormat is json through the V2/CSV reader is refused 400 ValidationException with the fixed text "GetStatementResultV2 can only return results that are in CSV format. Try using GetStatementResult instead for JSON format" — the whole sentence is fixed text, it interpolates nothing from the request or from state (no Id, no format value, no table name). The symmetric refusal is the mirror of it: GetStatementResult on a ResultFormat=CSV statement answers 400 ValidationException "GetStatementResult can only return results that are in JSON format. Try using GetStatementResultV2 for other result formats", also entirely fixed text. Note what AWS's text does and the emulator's does not: it names the reader that WAS called and the reader the caller should switch to.
Emulator: GetStatementResultV2 on a json statement answers 400 ValidationException with "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must have specified ResultFormat as CSV." That is the operation's own Smithy DOCUMENTATION sentence, reused as a refusal message: services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs:105-107 defines CSV_RESULT_READER_REQUIREMENT from the model's prose and get_statement_result_v2.rs:205-207 returns it. The same file defines the mirror constant JSON_RESULT_READER_REQUIREMENT at state.rs:99-101 ("… must have specified ResultFormat as JSON, or let the format default to JSON."), which GetStatementResult serves on the CSV-statement path — the identical substitution, so the JSON-reader side of the rule is the same defect even though the check aborted at the V2 assertion before exercising it.
Why: the error message is part of the contract clients log, match and assert on, and here it is the ONLY signal distinguishing a format-mismatch refusal from the other 400 ValidationExceptions these two readers raise (batch-parent read, not-yet-finished statement, bad NextToken) — they all arrive as ValidationException/400. A client that routes on the message (switch readers / re-submit with the other ResultFormat) sees an unrecognised sentence and cannot recover, and every message-matching conformance assertion fails. Because the refusal is raised early, the check never reaches the JSON-vs-CSV rendering assertions behind it, so the divergence also masks whatever the CSV rendering does.
Fix: the redshift-data result-reader format-mismatch refusal — the two message constants in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs and their use in the GetStatementResultV2 and GetStatementResult handlers. Stop deriving the refusal text from the operation's model documentation (the doc states the PRECONDITION, not the refusal) and emit AWS's fixed sentence verbatim for each reader, keeping ValidationException / HTTP 400 as they already are.
Traces: redshift-data.json-and-csv-result-readers-are-mutually-exclusive.

## Evidence
Suite: simparity-behavioral-aws-v1 — redshift-data 16/24 (67%), 8 failed. This is the only redshift-data failure not already filed or owned.

[mismatch] redshift-data.json-and-csv-result-readers-are-mutually-exclusive
check_redshift_data_json_and_csv_result_readers_are_mutually_exclusive.py:221
  assert v2_on_json[2] == (
      "GetStatementResultV2 can only return results that are in CSV format. Try using "
      "GetStatementResult instead for JSON format")
  AssertionError: unexpected v2-on-json message 'The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must have specified ResultFormat as CSV.'

The two preceding assertions in the same check PASSED, so the divergence is the message alone:
  v2_on_json[0] == "ValidationException"  -> passed
  v2_on_json[1] == 400                    -> passed
  v2_on_json[2] (message)                 -> FAILED as above

expected (fixed text, no interpolated values): "GetStatementResultV2 can only return results that are in CSV format. Try using GetStatementResult instead for JSON format"
actual:                                        "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must have specified ResultFormat as CSV."

Mirror leg, same cause, not reached because the check aborted above it (check file lines 1493-1502):
expected for get_statement_result(Id=<a ResultFormat='CSV' statement>): 400 ValidationException "GetStatementResult can only return results that are in JSON format. Try using GetStatementResultV2 for other result formats"
emulator constant on that path: JSON_RESULT_READER_REQUIREMENT = "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must have specified ResultFormat as JSON, or let the format default to JSON."

Emulator source:
  services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs:85-107 (both constants, documented as deliberately taken from the model's prose)
  services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/get_statement_result_v2.rs:200-207 (`if !statement.result_format_is_csv() { return Err(validation(CSV_RESULT_READER_REQUIREMENT...)) }`)

Model corroboration (services/service-com-amazonaws-redshiftdata-redshiftdata/smithy-model/com-amazonaws-redshiftdata-redshiftdata.json): GetStatementResultV2 and GetStatementResult both model ValidationException (errorKind client, httpError 400); the model does NOT state either refusal's message text, so the messages above are recorded behavior.

(The statement Ids, workgroup/namespace names and the generated table name in this run are per-run examples and are not part of the requirement; the requirement is the status 400, the code ValidationException and each reader's fixed sentence.)

## Steps to Reproduce
1. Clone the repo and boot the emulator.
2. Provision a Redshift Serverless namespace + workgroup (redshift-serverless create_namespace / create_workgroup) and wait for the workgroup to reach AVAILABLE.
3. With a botocore redshift-data client: execute_statement(WorkgroupName=<wg>, Database='devdb', Sql='create table t(i int, amt decimal(9,3), dbl double precision, flag boolean, note varchar(16))'), then insert a row, then execute_statement(... Sql='select i, amt, dbl, flag, note from t order by i') with NO ResultFormat (so the statement is json) and poll describe_statement to FINISHED.
4. Call get_statement_result_v2(Id=<that statement id>) -> expect HTTP 400 / ValidationException with message exactly "GetStatementResultV2 can only return results that are in CSV format. Try using GetStatementResult instead for JSON format"; observe HTTP 400 / ValidationException with "The ExecuteStatement or BatchExecuteStatement operation that ran the SQL statement must have specified ResultFormat as CSV."
5. Mirror leg: re-submit the same SELECT with ResultFormat='CSV', wait FINISHED, then call get_statement_result(Id=<csv statement id>) -> expect HTTP 400 / ValidationException "GetStatementResult can only return results that are in JSON format. Try using GetStatementResultV2 for other result formats"; observe the emulator's JSON_RESULT_READER_REQUIREMENT sentence instead.
6. Re-replay redshift-data.json-and-csv-result-readers-are-mutually-exclusive to confirm the fix (it continues past the refusal into the JSON/CSV rendering and ColumnMetadata assertions, which this refusal currently masks).
