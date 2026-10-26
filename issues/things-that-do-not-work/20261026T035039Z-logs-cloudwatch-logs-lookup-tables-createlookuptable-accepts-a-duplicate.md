Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.put-lookup-table-malformed-schema-rejected logs.e2e-lookup-table-enriches-transformed-log-record
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.put-lookup-table-malformed-schema-rejected|_client_error#4|AssertionError|raise AssertionError(f"expected CreateLookupTable to reject the malformed body, but it SUCCEEDED and returned {result!r}")
Issue-Key: logs.e2e-lookup-table-enriches-transformed-log-record|run#25|AssertionError|assert message == (
Issue-Commit: b21a8aaac8db581a7a7c874ff215cc379a04e117

# CloudWatch Logs lookup tables: CreateLookupTable accepts a duplicate-column CSV header, and lookup-table not-found refusals use a generic message instead of AWS's wording (2 causes)

> _Note: this issue was identified against the code base at commit `b21a8aaac8db581a7a7c874ff215cc379a04e117`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLookupTable

## Description
Two net-new root causes in the CloudWatch Logs lookup-table surface. Both are grounded in failing behavioral checks of the `logs` service; neither is covered by an open issue (the only open issues matching "lookup" are the REST-route-table 501 issue and unrelated s3control/cloudtrail/sagemaker ones).

=== Cause 1 of 2: CreateLookupTable accepts a `tableBody` whose CSV header row declares the same column name twice ===

AWS: `CreateLookupTable` reads `tableBody` as CSV whose "first row must be a header row with column names" (Smithy doc, CreateLookupTable / member `tableBody`, TableBody @length 1..10485760). A header row that declares the same column name twice does not name a column set, and AWS refuses the create. Recorded behavior: HTTP 400, error code `ValidationException` (modeled on CreateLookupTable: errors are AccessDeniedException, InvalidParameterException, LimitExceededException, ResourceAlreadyExistsException, ServiceUnavailableException, ValidationException — all client errors at HTTP 400), message `CSV header contains duplicate field names`. That message is entirely fixed text — it carries no value from the request or from state, so it is reproduced verbatim. The refusal is total: no table is created, so `DescribeLookupTables` must not list the name afterwards. The recording also establishes the neighbouring rule, so the fix must not over-reject: a column-COUNT mismatch between the header row and a data row IS accepted by AWS; only duplicate header field names are refused.

Emulator: the create is ACCEPTED. For `tableBody="key,key\nuser_id,active\n"` the emulator answered HTTP 200 with a populated `lookupTableArn` and `createdAt`, and the table was stored under the requested name (the check had to delete it again). No `ValidationException` was raised at all.

Why: a client that relies on AWS refusing a malformed header gets a table whose columns are ambiguous. Every later read of that table diverges — `DescribeLookupTables` derives `tableFields` from the header and will report the duplicated name twice, and any enrichment keyed on that column resolves against an undefined column. Error-handling paths that branch on `ValidationException` from CreateLookupTable are never exercised, so a client that reports the malformed upload to its user instead reports success.

Fix: the CSV-body validation of this operation's handler — `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/create_lookup_table.rs` reads the body through `crate::state::parse_table_body` (called at the `LookupTableSource::TableBody` arm) and that parse recognises no duplicate-header-name condition, so the handler has nothing to refuse on. The duplicate-name check belongs where the header row is read, and must be raised as `ValidationException` (the cross-member source rules in the same handler already use `ValidationException`), not as the `InvalidParameterException` that `parse_table_body` failures currently map to, and it must be raised BEFORE anything is inserted into `store.lookup_tables`. `UpdateLookupTable` takes the same `tableBody` member and needs the same rule.

=== Cause 2 of 2: GetLookupTable / UpdateLookupTable on a deleted lookup table return a generic not-found message instead of AWS's lookup-table wording ===

AWS: `ResourceNotFoundException` is modeled on both `GetLookupTable` and `UpdateLookupTable` (HTTP 400, client error). Recorded behavior: after `DeleteLookupTable`, both operations on the dead `lookupTableArn` answer HTTP 400 / `ResourceNotFoundException` with the message template `Lookup table '<lookupTableName>' not found in account '<accountId>'`. Fixed text: `Lookup table '`, `' not found in account '`, `'`. Variable parts: the lookup table NAME carried by the ARN the request names, and the ACCOUNT ID that ARN names (both taken from the request/state — never a literal from one recording). The error code and the 400 status are correct today; only the message diverges.

Emulator: both operations answer `ResourceNotFoundException` with the message `The specified lookup table does not exist.` — a generic sentence that names neither the table nor the account.

Why: the message is part of the error contract. A client or operator that parses the refusal to tell WHICH table and WHICH account was missing (the common case when an ARN crosses accounts or regions, which this handler explicitly distinguishes) gets nothing to parse, and a log line or assertion matching AWS's wording never matches.

Fix: the not-found refusal in these two handlers' lookup path — `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_lookup_table.rs` builds the message with the shared `crate::state::resource_not_found_message("lookup table")` helper, and `ops/update_lookup_table.rs` refuses the same way. Lookup tables need their own message built from the name and account the requested `lookupTableArn` carries (the handler already parses that ARN via `crate::state::lookup_table_name_from_arn`), rather than the generic shared template. Do not change the code or the status; only the message.

Traces: see trace_ids — `logs.put-lookup-table-malformed-schema-rejected` (cause 1) and `logs.e2e-lookup-table-enriches-transformed-log-record` (cause 2).

## Evidence
Cause 1 — check `logs.put-lookup-table-malformed-schema-rejected`:
  Request: CreateLookupTable(lookupTableName=<generated>, tableBody="key,key\nuser_id,active\n")
  Expected (AWS): HTTP 400, Code=ValidationException, Message="CSV header contains duplicate field names"; the table is NOT created (absent from DescribeLookupTables).
  Actual (emulator): HTTP 200, body {'lookupTableArn': 'arn:aws:logs:us-west-2:000000000000:lookup-table:sim2real_behav_plt_bad_...', 'createdAt': ...} — the generated name/arn/timestamp are per-run values, the divergence is the 200-with-a-created-table instead of a 400.
  Replay assertion: "expected CreateLookupTable to reject the malformed body, but it SUCCEEDED and returned {...}".
  Positive control in the same check (tableBody="key,value\nuser_id,active\n" accepted) must keep passing, as must AWS's acceptance of a header/row column-count mismatch.

Cause 2 — check `logs.e2e-lookup-table-enriches-transformed-log-record`:
  Sequence: CreateLookupTable -> GetLookupTable -> DescribeLookupTables -> DeleteLookupTable -> GetLookupTable -> UpdateLookupTable.
  Expected (AWS), both post-delete calls: HTTP 400, Code=ResourceNotFoundException, Message="Lookup table '<tableName>' not found in account '<accountId>'" (tableName from the requested lookupTableArn, accountId from that ARN / the caller identity).
  Actual (emulator): Code=ResourceNotFoundException (correct) but Message='The specified lookup table does not exist.'
  Replay assertion: "AssertionError: unexpected GetLookupTable message after DeleteLookupTable: 'The specified lookup table does not exist.'"
  Source of the emulator string: ops/get_lookup_table.rs calls crate::state::resource_not_found_message("lookup table").

## Steps to Reproduce
Boot the emulator and point an AWS SDK `logs` client at it.

Cause 1:
1. CreateLookupTable(lookupTableName="dup_header_probe", tableBody="key,key\nuser_id,active\n").
2. Observe HTTP 200 and a returned lookupTableArn. AWS answers HTTP 400 ValidationException "CSV header contains duplicate field names".
3. DescribeLookupTables() and observe "dup_header_probe" is listed; on AWS it is absent.
4. Control (must stay accepted): CreateLookupTable(lookupTableName="ok_header_probe", tableBody="key,value\nuser_id,active\n") -> 200.
5. Control (must stay accepted on AWS, so must not be newly rejected): a body whose data row has a different column count than the header.

Cause 2:
1. CreateLookupTable(lookupTableName="gone_probe", tableBody="ip_address,hostname,owner\n10.0.1.5,web-01,team-a\n") and keep the returned lookupTableArn.
2. DeleteLookupTable(lookupTableArn=<that arn>).
3. GetLookupTable(lookupTableArn=<that arn>) -> ResourceNotFoundException with Message 'The specified lookup table does not exist.'; AWS answers "Lookup table 'gone_probe' not found in account '<accountId>'".
4. UpdateLookupTable(lookupTableArn=<that arn>, tableBody=<same csv>) -> same divergent message.

Re-replay `logs.put-lookup-table-malformed-schema-rejected` and `logs.e2e-lookup-table-enriches-transformed-log-record` to confirm both causes are closed.
