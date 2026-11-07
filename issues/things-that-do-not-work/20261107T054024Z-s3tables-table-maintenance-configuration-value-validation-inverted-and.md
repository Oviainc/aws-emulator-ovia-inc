Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-sct-maintenance-value-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-sct-maintenance-value-mistakes s3tables-sct-sort-order-at-create-via-write-order s3tables-sct-sort-refused-without-sort-order-then-fallback s3tables-sct-maintenance-on-wrong-identifiers
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->2xx|$
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->2xx|$headers.content-length
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->2xx|status
Issue-Key: s3tables|PutTableMaintenanceConfiguration|2xx->4xx|status
Issue-Key: s3tables|GetTableMaintenanceConfiguration|2xx->2xx|$.configuration.icebergCompaction.settings.icebergCompaction.strategy
Issue-Key: s3tables|GetTableMaintenanceConfiguration|2xx->2xx|$.configuration.icebergCompaction.settings.icebergCompaction.targetFileSizeMB
Issue-Key: s3tables|GetTableMaintenanceConfiguration|4xx->4xx|$.message
Issue-Key: s3tables|GetTableMaintenanceConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|GetTableMaintenanceConfiguration|4xx->4xx|status
Issue-Key: s3tables|GetTableMaintenanceConfiguration|2xx->2xx|$.tableARN
Issue-Commit: 547f35aa751bdfccaa69888f930e73773cf7bea8

# s3tables table maintenance configuration: value validation inverted and refusal/read-back shapes wrong

> _Note: this issue was identified against the code base at commit `547f35aa751bdfccaa69888f930e73773cf7bea8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTableMaintenanceConfiguration

## Description
Seven distinct net-new wire causes in the s3tables table-maintenance surface (Put/GetTableMaintenanceConfiguration), all replayed from the wire corpus and none of them carried by an open issue's replay key. NOTE: this issue carries 7 SEPARATE causes — all seven must be closed before submitting.

=== Cause 1 of 7: PutTableMaintenanceConfiguration accepts an out-of-range icebergCompaction targetFileSizeMB ===
AWS: Smithy: PutTableMaintenanceConfiguration PUT /tables/{tableBucketARN}/{namespace}/{name}/maintenance/{type}, success code 204 with an empty body; input member `value` -> TableMaintenanceConfigurationValue{status: MaintenanceStatus, settings: TableMaintenanceSettings}; IcebergCompactionSettings.targetFileSizeMB is a PositiveInteger (modeled range 1..2147483647) so the model does NOT express the service limit; modeled error BadRequestException (httpError 400, "The request is invalid or malformed."). The limit itself is recorded behavior only: a targetFileSizeMB outside the supported window is refused with HTTP 400 and the message "The specified compaction file size is invalid. Minimum value: 64 MB, maximum value: 512 MB." That message is entirely fixed text — the two numbers are the service's constant bounds, not values from the request — and the response carries an x-amzn-errortype header and a non-zero content-length.
Emulator: the same put SUCCEEDS — HTTP 204 with an empty JSON body ({}), no x-amzn-errortype and no content-length — so the compaction file-size window is never checked and the out-of-range value is persisted.
Why: a client that relies on AWS rejecting an unsupported compaction file size gets a silent accept; the table is left configured with a size real S3 Tables would never store, and negative-path tests written against AWS (expecting BadRequestException with that message) pass against the emulator and fail in production.
Fix: the validation of PutTableMaintenanceConfiguration in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_maintenance_configuration.rs) — range-check icebergCompaction.targetFileSizeMB against the service window and refuse with BadRequestException before any state write.
Traces: s3tables-sct-maintenance-value-mistakes#task#10 (cluster also covers s3tables-sct-sort-order-at-create-via-write-order#task#9, s3tables-sct-sort-refused-without-sort-order-then-fallback#task#4 and #task#6).

=== Cause 2 of 7: a maintenance put AWS accepts is refused with 400 ===
AWS: Smithy: PutTableMaintenanceConfiguration, http code 204, no response body; a well-formed icebergCompaction value whose status is a MaintenanceStatus member (enabled|disabled) and whose targetFileSizeMB is inside the supported window is accepted with HTTP 204 and stored.
Emulator: the same put answers HTTP 400 instead of 204, so a configuration AWS stores is rejected outright.
Why: the inverse of Cause 1 — the emulator's acceptance window for a maintenance value does not match the service's, so a legitimate configuration cannot be written at all. A client sees a hard refusal for input AWS accepts, and every later read of that table's maintenance configuration reflects a value that was never written (which is what Cause 3 observes).
Fix: the same handler/validation pair as Cause 1 (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_maintenance_configuration.rs) — accept the full modeled value space, applying only the service's documented refusals.
Traces: s3tables-sct-maintenance-value-mistakes#task#7.

=== Cause 3 of 7: GetTableMaintenanceConfiguration reads back the invalid configuration a refused put should never have stored ===
AWS: Smithy: GetTableMaintenanceConfiguration GET /tables/{tableBucketARN}/{namespace}/{name}/maintenance, code 200, required output members `tableARN` and `configuration` (TableMaintenanceConfiguration map keyed by TableMaintenanceType); IcebergCompactionStrategy enum values are auto | binpack | sort | z-order. Recorded behavior: after a sequence of maintenance puts in which the out-of-window one is refused, the read-back reports the last ACCEPTED configuration — configuration.icebergCompaction.settings.icebergCompaction.strategy 'auto' with targetFileSizeMB 512 — i.e. a refused put makes no state change.
Emulator: the read-back reports strategy 'binpack' and targetFileSizeMB 16 — the values of the put AWS refuses, proving the refused request mutated state.
Why: a refusal with side effects is worse than a wrong status: the client is told nothing changed (or, per Cause 2, that a valid change failed) yet the table's compaction behavior is now driven by a configuration AWS would not accept, and any read-modify-write on the configuration carries it forward.
Fix: the PutTableMaintenanceConfiguration handler's state write (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_maintenance_configuration.rs) plus the projection in get_table_maintenance_configuration.rs — validate fully before touching state so a refused put leaves the stored value untouched.
Traces: s3tables-sct-maintenance-value-mistakes#task#14 (cluster also covers s3tables-sct-sort-refused-without-sort-order-then-fallback#task#10).

=== Cause 4 of 7: a refused `sort` compaction put still changes the stored strategy ===
AWS: Smithy: GetTableMaintenanceConfiguration output configuration.icebergCompaction.settings.icebergCompaction.strategy, IcebergCompactionStrategy enum (auto|binpack|sort|z-order). Recorded behavior: enabling the `sort` strategy on a table whose metadata carries no sort order is refused, and the following read-back still reports the untouched default strategy 'auto'.
Emulator: the read-back reports strategy 'sort', so the refused (here, silently accepted) put persisted.
Why: the client believes the table is still on the default compaction strategy while the emulator has it on one the table metadata cannot support; a test that asserts the post-refusal state is unchanged passes on AWS and fails on the emulator, and vice versa.
Fix: same component as Cause 3 — reject before mutating in services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/put_table_maintenance_configuration.rs. Open issue 20261106T132944Z-s3tables-2-net-new-root-causes-puttablemaintenanceconfiguration-accepts describes the missing sort-order precondition from the BEHAVIORAL corpus, but it does not carry this fixture step's replay key, so this wire read-back divergence is unrun by any filed issue; closing that cause should be re-verified against the trace below.
Traces: s3tables-sct-sort-refused-without-sort-order-then-fallback#task#5.

=== Cause 5 of 7: a maintenance read on a non-existent TABLE reports the BUCKET as missing ===
AWS: Smithy: GetTableMaintenanceConfiguration, modeled error NotFoundException (httpError 404, "The request was rejected because the specified resource could not be found."). Recorded behavior: with a valid, existing table bucket ARN but a table name that does not exist, the refusal is HTTP 404 with message "The specified table does not exist." — fixed text naming the missing resource KIND, with no variable parts.
Emulator: the same request answers HTTP 404 but with message "The specified bucket does not exist.", naming the wrong resource kind.
Why: the message is the only thing distinguishing a missing table from a missing bucket at the same status; a client (or an operator reading logs) retries or recreates the bucket instead of the table, and error-mapping code keyed on the message misroutes the failure.
Fix: the GetTableMaintenanceConfiguration handler's lookup/refusal path in the s3tables crate (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_maintenance_configuration.rs) — resolve bucket then namespace then table, and emit the message for the level that actually failed.
Traces: s3tables-sct-maintenance-on-wrong-identifiers#task#6.

=== Cause 6 of 7: a table-bucket ARN from another Region is answered 404 not-found instead of a 400 Region mismatch ===
AWS: Smithy: GetTableMaintenanceConfiguration, input tableBucketARN pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63})$ (so a foreign-Region ARN is pattern-valid), modeled errors BadRequestException (400) and NotFoundException (404). Recorded behavior: an ARN whose Region differs from the endpoint's Region is refused with HTTP 400 and the message "The provided Region does not match the Region of the resource." — entirely fixed text — carrying an x-amzn-errortype header for the 400 error class, NOT a not-found.
Emulator: the request answers HTTP 404 with x-amzn-errortype NotFoundException and message "The specified bucket does not exist."
Why: Region mismatch is a caller-addressing bug, and AWS says so with a 400 that tells the caller to re-point the client; the emulator's 404 tells the caller the resource is gone, so cross-Region misconfiguration is misdiagnosed as a deleted bucket and retry/creation logic takes the wrong branch.
Fix: ARN parsing and Region validation shared by the s3tables lookup handlers (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/get_table_maintenance_configuration.rs and the common ARN resolution it calls) — compare the ARN's Region against the serving Region and refuse with BadRequestException before the existence check.
Traces: s3tables-sct-maintenance-on-wrong-identifiers#task#7.

=== Cause 7 of 7: the tableARN echoed by GetTableMaintenanceConfiguration does not match the ARN CreateTable handed out ===
AWS: Smithy: GetTableMaintenanceConfiguration required output member `tableARN` (TableARN, len 1..2048, pattern ^(arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}/table/[a-zA-Z0-9-_]{1,255})$). Recorded behavior: the ARN read back here is identical to the tableARN the table's own CreateTable response returned — the fixture binds that value and compares it, so the requirement is ARN identity across operations for one table, not any particular literal ARN.
Emulator: the comparison diverges because the bound CreateTable ARN is never established on this path — the preceding CreateTable in this fixture answers 409 ConflictException instead of 200 with a tableARN, so the emulator serves a maintenance read for a table whose create never succeeded and the two ARNs cannot agree.
Why: a client that stores the ARN from CreateTable and uses it to key maintenance state cannot correlate the two responses; here it also shows the create/read pair disagreeing about whether the table exists at all.
Fix: the CreateTable handler and its metadata validation (services/service-com-amazonaws-s3tables-s3tablebuckets/src/ops/create_table.rs) so the create on this path succeeds and mints one stable tableARN, and the get_table_maintenance_configuration.rs projection so it echoes exactly that stored ARN. Open issue 20261103T033559Z-s3tables-request-validation-diverges-generic-validator-text-replaces covers the upstream CreateTable refusal but does not carry this step's replay key.
Traces: s3tables-sct-sort-order-at-create-via-write-order#task#7.

## Evidence
Wire replay of simparity-wire-aws-v1 against the emulator built from this checkout: 564 steps, 388 passed, 171 mismatched, score 68.8%. The seven clusters below are s3tables mismatches with no stub_path and no owning issue.

Cause 1 — s3tables-sct-maintenance-value-mistakes#task#10, PutTableMaintenanceConfiguration: status expected 400, got 204; $: empty_body (expected {'message': 'The specified compaction file size is invalid. Minimum value: 64 MB, maximum value: 512 MB.'}, got {}); $headers.x-amzn-errortype missing; $headers.content-length missing. 4 steps affected.

Cause 2 — s3tables-sct-maintenance-value-mistakes#task#7, PutTableMaintenanceConfiguration: status expected 204, got 400.

Cause 3 — s3tables-sct-maintenance-value-mistakes#task#14, GetTableMaintenanceConfiguration: $.configuration.icebergCompaction.settings.icebergCompaction.strategy expected 'auto', got 'binpack'; .targetFileSizeMB expected 512, got 16. 2 steps affected.

Cause 4 — s3tables-sct-sort-refused-without-sort-order-then-fallback#task#5, GetTableMaintenanceConfiguration: $.configuration.icebergCompaction.settings.icebergCompaction.strategy expected 'auto', got 'sort'.

Cause 5 — s3tables-sct-maintenance-on-wrong-identifiers#task#6, GetTableMaintenanceConfiguration: $.message expected 'The specified table does not exist.', got 'The specified bucket does not exist.' (both HTTP 404).

Cause 6 — s3tables-sct-maintenance-on-wrong-identifiers#task#7, GetTableMaintenanceConfiguration: status expected 400, got 404; $.message expected 'The provided Region does not match the Region of the resource.', got 'The specified bucket does not exist.'; $headers.x-amzn-errortype got 'NotFoundException' where a 400-class error type was recorded.

Cause 7 — s3tables-sct-sort-order-at-create-via-write-order#task#7, GetTableMaintenanceConfiguration: $.tableARN expected the ARN bound from the fixture's CreateTable response, got a freshly minted ARN; the CreateTable at #task#5 of the same fixture answered 409 instead of 200.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus, or by hand against the s3tables endpoint:

Causes 1-3 (fixture s3tables-sct-maintenance-value-mistakes):
1. CreateTableBucket -> arn; CreateNamespace(tableBucketARN=arn, namespace=[<ns>]); CreateTable(..., format='ICEBERG').
2. PutTableMaintenanceConfiguration(type='icebergCompaction', value={'status':'enabled','settings':{'icebergCompaction':{'strategy':'auto','targetFileSizeMB':512}}}) — AWS 204, emulator 400 (Cause 2).
3. PutTableMaintenanceConfiguration with targetFileSizeMB below the 64 MB minimum (e.g. 16) — AWS 400 BadRequestException 'The specified compaction file size is invalid. Minimum value: 64 MB, maximum value: 512 MB.', emulator 204 empty body (Cause 1).
4. GetTableMaintenanceConfiguration on the same table — AWS reports strategy 'auto' / targetFileSizeMB 512 (the refused put changed nothing), emulator reports 'binpack' / 16 (Cause 3).

Cause 4 (fixture s3tables-sct-sort-refused-without-sort-order-then-fallback):
5. On a table whose metadata carries no sort order, PutTableMaintenanceConfiguration(type='icebergCompaction', value={'status':'enabled','settings':{'icebergCompaction':{'strategy':'sort'}}}), then GetTableMaintenanceConfiguration — AWS still reports strategy 'auto', emulator reports 'sort'.

Causes 5-6 (fixture s3tables-sct-maintenance-on-wrong-identifiers):
6. GetTableMaintenanceConfiguration with an existing table bucket ARN and a table name that does not exist — AWS 404 'The specified table does not exist.', emulator 404 'The specified bucket does not exist.'
7. GetTableMaintenanceConfiguration with a pattern-valid table bucket ARN whose Region is not the endpoint's Region — AWS 400 'The provided Region does not match the Region of the resource.', emulator 404 NotFoundException 'The specified bucket does not exist.'

Cause 7 (fixture s3tables-sct-sort-order-at-create-via-write-order):
8. CreateTable on a fresh name — AWS 200 with tableARN (emulator answers 409 at this step), then GetTableMaintenanceConfiguration on that table and compare its tableARN with the one CreateTable returned: they must be identical.
