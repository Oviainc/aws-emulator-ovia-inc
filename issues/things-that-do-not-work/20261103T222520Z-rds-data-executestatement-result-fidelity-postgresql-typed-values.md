Issue-Source: trace-sweep-divergence
Issue-Trace: rdsdata-execute-statement-field-union-and-error-envelopes
Issue-Step: task#18
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rdsdata-execute-statement-field-union-and-error-envelopes rdsdata-execute-statement-live-cluster-round-trip
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|2xx->2xx|$.formattedRecords
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.columnMetadata
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.numberOfRecordsUpdated
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|$.records
Issue-Key: rds-data|ExecuteStatement|2xx->4xx|status
Issue-Commit: 8ab069bdff3621cfe04618790b58949e3dee53e7

# rds-data ExecuteStatement result fidelity: PostgreSQL typed values and statements diverge from AWS

> _Note: this issue was identified against the code base at commit `8ab069bdff3621cfe04618790b58949e3dee53e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Two net-new causes, both in rds-data ExecuteStatement's result path. They share one root fault: the emulator serves the statement through a SQLite-shaped engine and hands the engine's own values and errors straight back, instead of producing the PostgreSQL-typed result envelope the RDS Data API serves for an Aurora PostgreSQL cluster. A fixer must close BOTH causes before submitting.

Cause 1 of 2: formattedRecords renders blob and boolean columns in the backing engine's notation instead of the RDS Data API's JSON notation. Fixture rdsdata-execute-statement-field-union-and-error-envelopes, step task#18.
AWS: Smithy (services/service-com-amazonaws-rdsdata-rdsdataservice/smithy-model/com-amazonaws-rdsdata-rdsdataservice.json): ExecuteStatement is POST /Execute, http code 200; ExecuteStatementResponse.formattedRecords is documented as "A string value that represents the result set of a SELECT statement in JSON format. This value is only present when the formatRecordsAs parameter is set to JSON" (RecordsFormatType values NONE | JSON). The Field union models blobValue as a blob (base64 on the wire in the awsJson/REST-JSON encoding) and booleanValue as a boolean. Accordingly, in the JSON-formatted result set AWS renders a binary column as its base64 text and a boolean column as the JSON literals true/false. The recording confirms it: a bytea column of bytes 01 02 FF appears as "AQL/" (its base64 form) and a boolean array appears as [true,false]. NULLs, integers, doubles, strings and string/number arrays in the same row are already correct in the emulator, so only the blob and boolean mappings diverge.
Emulator: the same step returns 200 but the formattedRecords JSON carries the binary column as the backing engine's hex-escape literal (a "\\x" prefix followed by lowercase hex digits, e.g. "\\x0102ff" for those three bytes) in place of base64, and boolean array elements as the integers 1 and 0 in place of true and false. Every other field of the three recorded rows matches.
Why: formattedRecords is parsed as JSON by the client, so a client gets integers where its schema says boolean and an engine-specific hex string where it expects base64 bytes. Binary round-trips silently corrupt (base64-decoding "\\x0102ff" yields garbage, not the stored bytes), and boolean-typed deserializers either fail or coerce 1/0, so code that works against Aurora PostgreSQL misreads data against the emulator with no error raised.
Fix: the ExecuteStatement result serializer for the JSON records format in the rds-data service — the value-to-JSON mapping used to build formattedRecords must encode a binary column as base64 and a boolean column as a JSON boolean, consistently with the Field union's blobValue/booleanValue typing used for the records form.

Cause 2 of 2: a PostgreSQL SELECT of typed expressions is rejected with 400 instead of returning the result set. Fixture rdsdata-execute-statement-live-cluster-round-trip, step task#10.
AWS: ExecuteStatement on an Aurora PostgreSQL cluster evaluates the statement in PostgreSQL and answers 200 with the modeled ExecuteStatementResponse: numberOfRecordsUpdated 0 for a SELECT, records as a list of Field unions, and columnMetadata (when includeResultMetadata is set) as one ColumnMetadata per projected column carrying name/label, the JDBC type code, the database-specific typeName, nullable, precision and scale, plus the isSigned/isCaseSensitive/isCurrency/isAutoIncrement flags (Smithy: ExecuteStatementResponse.records/columnMetadata/numberOfRecordsUpdated; shapes Field, ColumnMetadata, Metadata). The recording shows exactly that for a six-column projection of PostgreSQL-typed expressions: typeNames int4, text, bool, numeric, float8 and text, with the integer as longValue, the text as stringValue, the boolean as booleanValue, the numeric as a stringValue (AWS returns exact-numeric columns as strings), the float8 as doubleValue and the typed NULL as isNull true. None of ExecuteStatement's modeled client errors applies to a well-formed SELECT against an existing relation.
Emulator: the step is answered 400 with no result envelope at all — columnMetadata, records and numberOfRecordsUpdated are all absent from the body. Hypothesis: the backing engine does not accept PostgreSQL type syntax/typed expressions in the statement and its parse/lookup failure is surfaced as a client error; the sibling cluster on this service (service=rds-data|ExecuteStatement|status=400->400, representative rdsdata-execute-statement-live-cluster-round-trip#task#23, already owned by open issue 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions) shows the engine's raw wording leaking through as "no such table: <name>" in place of AWS's PostgreSQL diagnostic, which is the same leak seen from the other side.
Why: a statement AWS executes successfully fails outright against the emulator, so the round trip a client uses to read back what it wrote cannot complete and no columnMetadata is ever available. Any PostgreSQL-typed projection (casts, typed literals, numeric/bool/bytea columns) becomes a hard 400, which is indistinguishable at the client from a genuine BadRequest in the client's own SQL and makes the operation unusable for its primary purpose.
Fix: the ExecuteStatement handler and its result-set construction in the rds-data service — the statement path must execute a PostgreSQL-typed projection and build the 200 ExecuteStatementResponse (records, numberOfRecordsUpdated 0 for a SELECT, and columnMetadata with the PostgreSQL typeName/type/precision/scale/flags per column) rather than rejecting the statement; engine-level parse failures must not be returned as modeled client errors for statements AWS accepts.

Note on existing issues: no open rds-data issue covers either cause — the replay reports both clusters unowned. 20261023T045849Z-rds-data-skips-resource-secret-and-transaction-preconditions covers the precondition and error-classification gaps (and owns the "no such table" wording cluster), 20261029T191739Z-rds-data-committransaction-skips-the-http-endpoint-enabled covers CommitTransaction's endpoint gate, and 20261101T205955Z-rds-data-executestatement-never-answers-on-the-live covers an ExecuteStatement step that returns no response at all (task#8 of the live-cluster fixture). None of them concerns the content or the typing of a successful result set.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator booted from this checkout (emulator_up true, bootstrap_signal false, /__admin/health 200 and the data listener accepting; 99 steps, 61 passed, score 61.6%).

Cause 1 — divergence cluster service=rds-data|ExecuteStatement|status=200->200|keys=$.formattedRecords, verdict "mismatch", stub_path empty, owned_by empty:
  fixture_id: rdsdata-execute-statement-field-union-and-error-envelopes
  step_key:   task#18
  expected_status 200, actual_status 200
  affected: [rdsdata-execute-statement-field-union-and-error-envelopes#task#18] (affected_count 1)
  Expected (AWS) formattedRecords, row 1: {"id":1,"label":"rdsd-first","qty":42,"ratio":0.25,"active":true,"payload":"AQL/","note":null,"tags":["red","blue"],"scores":[10,20,30],"weights":[0.5,1.25],"flags":[true,false]}
  Actual (emulator), same row: ..."payload":"\\x0102ff"..."flags":[1,0]...
  Diverging parts: the binary column's encoding (base64 text vs the engine's "\\x"+hex literal) and the boolean array elements (JSON true/false vs integers 1/0). Rows 2 and 3, and all other members of row 1, match. The byte sequence 01 02 FF and the column names are this recording's values, not requirements; the requirement is base64 for binary and JSON booleans for boolean, for whatever column the request projects.

Cause 2 — divergence cluster service=rds-data|ExecuteStatement|status=200->400|keys=$.columnMetadata,$.numberOfRecordsUpdated,$.records,status, verdict "mismatch", stub_path empty, owned_by empty:
  fixture_id: rdsdata-execute-statement-live-cluster-round-trip
  step_key:   task#10
  expected_status 200, actual_status 400
  affected: [rdsdata-execute-statement-live-cluster-round-trip#task#10] (affected_count 1)
  detail: status: expected 200, got 400; $.columnMetadata missing (expected one ColumnMetadata per column with typeNames int4, text, bool, numeric, float8, text — nullable 1, the int4 isSigned true, the text columns isCaseSensitive true with precision 2147483647, the float8 precision/scale 17 — got None); $.numberOfRecordsUpdated missing (expected 0, got None); $.records missing (expected [[{longValue 1},{stringValue "abc"},{booleanValue true},{stringValue "2.5"},{doubleValue 1.5},{isNull true}]], got None)
  Expected (AWS): 200 with that ExecuteStatementResponse envelope. Actual (emulator): 400 and no envelope members at all. The particular column names and literals are this recording's values; the requirement is that a PostgreSQL-typed projection returns 200 with records, numberOfRecordsUpdated and columnMetadata typed as PostgreSQL reports them.

Related, already owned (not filed here, cited only as corroboration of cause 2's root fault): cluster service=rds-data|ExecuteStatement|status=400->400 at rdsdata-execute-statement-live-cluster-round-trip#task#23, where the emulator answers "no such table: <name>" with x-amzn-errortype BadRequestException while AWS answers the PostgreSQL diagnostic 'ERROR: relation "<name>" does not exist; Position: <n>; SQLState: 42P01' (fixed text: the ERROR:/relation/does not exist/Position/SQLState frame and the 42P01 state; variable parts: the relation name from the request and the position of the token in the submitted SQL).

## Steps to Reproduce
1. Clone this checkout and boot the emulator from it.
2. Replay wire trace set simparity-wire-aws-v1.
3. Cause 1: run fixture rdsdata-execute-statement-field-union-and-error-envelopes to step task#18 — an ExecuteStatement (POST /Execute) with formatRecordsAs=JSON over a table holding a binary column and a boolean-array column. Observe 200 whose formattedRecords renders the binary column as the engine's "\\x"+hex literal instead of its base64 text, and the boolean array elements as 1/0 instead of true/false.
4. Cause 2: run fixture rdsdata-execute-statement-live-cluster-round-trip to step task#10 — an ExecuteStatement selecting PostgreSQL-typed expressions (int4, text, bool, numeric, float8 and a typed NULL) with result metadata requested. Observe 400 with no records, numberOfRecordsUpdated or columnMetadata, where AWS answers 200 with the full ExecuteStatementResponse envelope and one ColumnMetadata per projected column.
5. Compare each response against the recorded AWS response for the same step.
