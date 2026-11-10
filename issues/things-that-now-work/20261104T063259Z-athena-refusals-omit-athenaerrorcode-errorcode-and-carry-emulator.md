Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-output-location-mistakes
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-output-location-mistakes athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket athena-kfi-traj-athena-query-context-mistakes athena-workgroup-lifecycle-and-query-execution athena-cats-traj-query-execution-polling-reads athena-kfi-traj-athena-reads-the-iceberg-table athena-data-catalog-update-round-trip-and-missing-name-rejected athena-update-data-catalog-contract athena-get-work-group-configuration-and-missing-error athena-service-catalog-metadata-and-workgroup-conflicts athena-cflog-athena-catalog-lookup-mistakes
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|GetQueryExecution|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetQueryExecution|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetQueryResults|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetQueryResults|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetDataCatalog|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetDataCatalog|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetDataCatalog|4xx->4xx|$.Message
Issue-Key: athena|GetWorkGroup|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|GetWorkGroup|4xx->4xx|$.ErrorCode
Issue-Key: athena|GetWorkGroup|4xx->4xx|$.Message
Issue-Key: athena|CreateWorkGroup|4xx->4xx|$.AthenaErrorCode
Issue-Key: athena|CreateWorkGroup|4xx->4xx|$.ErrorCode
Issue-Key: athena|CreateWorkGroup|4xx->4xx|$.Message
Issue-Related: 20261104T063259Z-athena-input-validation-returns-validationexception-without-athenaerrorcode-or.md
Issue-Commit: fb394b2a45eb301a39bf46320599094d7846a70d

# Athena refusals omit AthenaErrorCode/ErrorCode and carry emulator-authored message text (8 causes)

> _Note: this issue was identified against the code base at commit `fb394b2a45eb301a39bf46320599094d7846a70d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Athena's own (non-constraint) refusals reach the right HTTP status and exception type but lose the two members every Athena client branches on — AthenaErrorCode and ErrorCode — and several carry sentences the emulator composed instead of AWS's text. Smithy: InvalidRequestException {AthenaErrorCode: ErrorCode @length 1..256, Message}, httpError 400; AWS serialises both AthenaErrorCode and the ErrorCode alias in the body. 8 causes.

Cause 1 of 8: StartQueryExecution with no resolvable output location.
AWS: 400 InvalidRequestException, AthenaErrorCode='INVALID_INPUT', ErrorCode='INVALID_INPUT', and the wholly fixed Message 'No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings.' (no request-derived part; the doubled space after 'for' is AWS's own text).
Emulator: both code members absent and Message rewritten as 'No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input.'
Why: a client matching on INVALID_INPUT or on AWS's remediation text neither branches nor surfaces the documented fix.
Fix: the athena crate's StartQueryExecution output-location validation and the Athena error serializer.
Traces: athena-cats-traj-output-location-mistakes#task#3, athena-cflog-athena-ddl-route-hyphen-name-and-absent-bucket#task#6, athena-kfi-traj-athena-query-context-mistakes#task#5, athena-workgroup-lifecycle-and-query-execution#task#12.

Cause 2 of 8: GetQueryExecution for an unknown QueryExecutionId.
AWS: 400 InvalidRequestException with AthenaErrorCode='QUERY_EXECUTION_NOT_FOUND' and ErrorCode='QUERY_EXECUTION_NOT_FOUND' (the Message already matches).
Emulator: both members absent.
Why: 'no such execution' is indistinguishable from any other bad request, so a poller cannot stop on a permanent miss.
Fix: the Athena error serializer and the GetQueryExecution not-found path.
Traces: athena-cats-traj-query-execution-polling-reads#task#6, athena-workgroup-lifecycle-and-query-execution#task#8.

Cause 3 of 8: GetQueryResults against a query not in a readable state.
AWS: 400 InvalidRequestException with AthenaErrorCode='INVALID_QUERY_EXECUTION_STATE' and ErrorCode='INVALID_QUERY_EXECUTION_STATE'.
Emulator: both members absent.
Why: a client cannot tell 'poll again / query failed' from a malformed request, so retry logic has nothing to key on.
Fix: the Athena error serializer and the GetQueryResults state check.
Traces: athena-kfi-traj-athena-reads-the-iceberg-table#task#8, athena-workgroup-lifecycle-and-query-execution#task#9.

Cause 4 of 8: GetDataCatalog for a catalog that does not exist.
AWS: 400 InvalidRequestException, AthenaErrorCode='INVALID_INPUT', ErrorCode='INVALID_INPUT', Message of the fixed form 'DataCatalog <name> was not found.' — fixed text 'DataCatalog ' + the Name from the request + ' was not found.'
Emulator: both code members absent and the verb rewritten: 'DataCatalog <name> is not found.'
Why: text-matching clients and recorded-response assertions fail on a message that differs from AWS's wording.
Fix: the athena crate's GetDataCatalog not-found path and the Athena error serializer.
Traces: athena-data-catalog-update-round-trip-and-missing-name-rejected#task#6, athena-update-data-catalog-contract#task#6.

Cause 5 of 8: GetWorkGroup for a workgroup that does not exist.
AWS: 400 InvalidRequestException, AthenaErrorCode='INVALID_INPUT', ErrorCode='INVALID_INPUT', and the wholly fixed Message 'WorkGroup is not found.' — AWS interpolates no workgroup name.
Emulator: both code members absent and the message is request-derived, inserting the requested name: 'WorkGroup <name> is not found.'
Why: the message is not AWS's; a client (or a recorded-response comparison) that expects the fixed sentence never matches.
Fix: the athena crate's GetWorkGroup not-found path and the Athena error serializer.
Traces: athena-get-work-group-configuration-and-missing-error#task#3.

Cause 6 of 8: CreateWorkGroup for a name that already exists.
AWS: 400 InvalidRequestException, AthenaErrorCode='INVALID_INPUT', ErrorCode='INVALID_INPUT', and the fixed Message 'WorkGroup is already created' — no name, no trailing period.
Emulator: both code members absent and the message replaced with the request-derived 'WorkGroup <name> already exists.'
Why: an idempotent create-if-absent flow that keys on AWS's conflict text misreads the conflict.
Fix: the athena crate's CreateWorkGroup conflict path and the Athena error serializer.
Traces: athena-service-catalog-metadata-and-workgroup-conflicts#task#4.

Cause 7 of 8: GetTableMetadata for a table that is not in the catalog.
AWS: Smithy binds MetadataException (httpError 400, Message only) for a metastore miss on GetTableMetadata; the recording's $.Message is masked in the fixture (recorded behavior only — its exact text is not asserted), but it is not the emulator's sentence.
Emulator: composes its own request-derived message, 'Table does not exist: <database>.<table>', built from the request's DatabaseName and TableName.
Why: the refusal reason is phrased by the emulator rather than taken from the metastore contract, so message-matching clients diverge.
Fix: the athena crate's GetTableMetadata not-found path (message text from the recorded contract, not composed from the request).
Traces: athena-aorigin-athena-metadata-table-vs-view#task#7, athena-cflog-athena-catalog-lookup-mistakes#task#4, athena-glue-metadata-exploration#task#7.

Cause 8 of 8: ListTableMetadata for a database that is not in the catalog.
AWS: same MetadataException binding; the recorded $.Message is masked in the fixture (recorded behavior only) and differs from the emulator's.
Emulator: composes 'Database does not exist: <database>' from the request's DatabaseName.
Why: as cause 7, on the list path.
Fix: the athena crate's ListTableMetadata not-found path.
Traces: athena-cflog-athena-catalog-lookup-mistakes#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All eight causes agree on status (400 both sides); the divergence is in the error body.
Cause 1 — athena-cats-traj-output-location-mistakes#task#3, StartQueryExecution: expected $.AthenaErrorCode='INVALID_INPUT', $.ErrorCode='INVALID_INPUT', $.Message='No output location provided. You did not provide an output location for  your query results. Either specify an S3 bucket location or enable Athena managed query results in your workgroup settings.'; actual both codes missing and Message='No output location provided. An output location is required either through the Workgroup result configuration setting or as an API input.' (4 steps).
Cause 2 — athena-cats-traj-query-execution-polling-reads#task#6, GetQueryExecution: expected $.AthenaErrorCode=$.ErrorCode='QUERY_EXECUTION_NOT_FOUND'; actual both missing (2 steps).
Cause 3 — athena-kfi-traj-athena-reads-the-iceberg-table#task#8, GetQueryResults: expected $.AthenaErrorCode=$.ErrorCode='INVALID_QUERY_EXECUTION_STATE'; actual both missing (2 steps).
Cause 4 — athena-data-catalog-update-round-trip-and-missing-name-rejected#task#6, GetDataCatalog: expected codes 'INVALID_INPUT' and Message='DataCatalog <name> was not found.'; actual codes missing, Message='DataCatalog <name> is not found.' (2 steps).
Cause 5 — athena-get-work-group-configuration-and-missing-error#task#3, GetWorkGroup: expected codes 'INVALID_INPUT' and Message='WorkGroup is not found.'; actual codes missing, Message='WorkGroup sim2real-athena-wg-missing-b5cba6ae94 is not found.' (the requested name interpolated into text AWS leaves fixed).
Cause 6 — athena-service-catalog-metadata-and-workgroup-conflicts#task#4, CreateWorkGroup: expected codes 'INVALID_INPUT' and Message='WorkGroup is already created'; actual codes missing, Message='WorkGroup sim2real-probe-athena-svc-catalog-wg-9ede7909 already exists.'
Cause 7 — athena-aorigin-athena-metadata-table-vs-view#task#7, GetTableMetadata: $.Message diverges from the recorded (masked) value; actual 'Table does not exist: sim2real-probe-aorigin-ath-db-e6a78422.sim2real-probe-aorigin-ath-tbl-e6a78422' (3 steps).
Cause 8 — athena-cflog-athena-catalog-lookup-mistakes#task#5, ListTableMetadata: $.Message diverges from the recorded (masked) value; actual 'Database does not exist: sim2real_probe_cflog_none_f3f8c76f'.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Inspect the listed fixture#step pairs, representative athena-cats-traj-output-location-mistakes#task#3.
4. Minimal manual repro: StartQueryExecution with no ResultConfiguration.OutputLocation and a workgroup that sets none; GetQueryExecution with an unknown id; GetQueryResults for a query that is not in a readable state; GetDataCatalog and GetWorkGroup for absent names; CreateWorkGroup twice with the same name; GetTableMetadata/ListTableMetadata for an absent table/database. Each response must carry AthenaErrorCode and ErrorCode and AWS's message text (fixed for the output-location, GetWorkGroup and CreateWorkGroup cases).
