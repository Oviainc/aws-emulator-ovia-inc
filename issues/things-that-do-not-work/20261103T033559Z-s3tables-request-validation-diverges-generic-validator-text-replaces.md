Issue-Source: trace-sweep-divergence
Issue-Trace: s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected s3tables-sct-create-table-input-mistakes s3tables-sct-maintenance-value-mistakes s3tables-sct-namespace-string-and-delete-while-in-use s3tables-sct-sort-order-at-create-via-write-order s3tables-s3t-uppercase-and-hyphenated-table-names-rejected
Issue-Shape-Id: com.amazonaws.s3tables#S3TableBuckets
Issue-Key: s3tables|GetTable|4xx->4xx|$.message
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->4xx|$.message
Issue-Key: s3tables|PutTableMaintenanceConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateNamespace|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateTable|4xx->2xx|$.message
Issue-Key: s3tables|CreateTable|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateTable|4xx->2xx|status
Issue-Key: s3tables|CreateTable|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|CreateTable|4xx->4xx|status
Issue-Key: s3tables|CreateTable|2xx->4xx|$.tableARN
Issue-Key: s3tables|CreateTable|2xx->4xx|$.versionToken
Issue-Key: s3tables|CreateTable|2xx->4xx|status
Issue-Key: s3tables|GetTable|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: s3tables|GetTable|4xx->4xx|status
Issue-Related: 20261103T033559Z-s3tables-conflict-and-not-found-messages-are-emulator.md
Issue-Commit: 56c3e2d5db4619f120a986f75a3622f8f9af8979

# s3tables request validation diverges: generic validator text replaces AWS refusal messages and CreateTable metadata is unchecked

> _Note: this issue was identified against the code base at commit `56c3e2d5db4619f120a986f75a3622f8f9af8979`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTable

## Description
s3tables refuses malformed input with the request framework's own validator prose instead of the service's refusal messages, and one input class (CreateTable metadata) is not validated at all, which then cascades into wrong statuses on the following steps. 7 causes, all in the input-validation layer of the named operations.

Related open issues that do NOT own these clusters: 20261102T055316Z-s3tables-constraint-violation-refusals-are-now-blanket-400.md (constraint violations answering blanket 400 with generic validator text) and 20261102T104943Z-s3tables-8-net-new-root-causes-the-iceberg.md (mentions lookups re-validating names). The replay reports every cluster below as unowned, so they are filed here and those issues are named for cross-reference.

Cause 1 of 7 - GetTable with both tableArn and the bucket/namespace/name triple.
AWS: Smithy: GetTable input has tableBucketARN/namespace/name/tableArn all optional httpQuery members; errors include BadRequestException (httpError 400). Recorded response: 400 with message exactly 'Invalid input: Must provide either table arn alone OR all of (table bucket arn, namespace, table name) together' - fixed text, no request values.
Emulator: 400 with 'Either tableArn, or tableBucketARN, namespace and name together, must be specified.'
Why: a client that keys off the refusal text (or shows it) sees a message the service never produces, and the emulator's sentence describes the 'none supplied' case rather than the mutual-exclusion rule.
Fix: GetTable's input validation / its BadRequestException message.
Traces: s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected#task#5 (representative).

Cause 2 of 7 - CreateTable with an out-of-enum format.
AWS: Smithy: CreateTable.format targets OpenTableFormat (enum, required); errors include BadRequestException (httpError 400). Recorded response: 400 with message exactly 'The specified open table format is not valid'.
Emulator: 400 with the framework's generic constraint prose, "1 validation error detected: Value '<value>' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ICEBERG]".
Why: the message is Smithy-validator boilerplate, not the service's sentence; clients that branch on refusal text mis-handle an ordinary bad-format request.
Fix: CreateTable's format validation must raise the service's BadRequestException text instead of deferring to the generic enum validator.
Traces: s3tables-sct-create-table-input-mistakes#task#3 (representative), s3tables-sct-create-table-input-mistakes#task#4.

Cause 3 of 7 - PutTableMaintenanceConfiguration with an out-of-enum maintenance status.
AWS: Smithy: PutTableMaintenanceConfiguration.value is required (TableMaintenanceConfigurationValue, whose status is an enum of enabled/disabled); errors include BadRequestException (httpError 400). Recorded response: 400 with message exactly "The specified status is invalid. Value must be either 'enabled' or 'disabled.'" - fixed text including the two literal allowed values.
Emulator: 400 with "1 validation error detected: Value '<value>' at 'value.status' failed to satisfy constraint: Member must satisfy enum value set: [enabled, disabled]".
Why: same boilerplate-instead-of-service-text defect, on the maintenance configuration path.
Fix: PutTableMaintenanceConfiguration's value validation / message.
Traces: s3tables-sct-maintenance-value-mistakes#task#4 (representative) plus task#5, task#9, task#11, task#12 of the same fixture.

Cause 4 of 7 - PutTableMaintenanceConfiguration with an unknown maintenance type in the URI.
AWS: Smithy: the type member is an httpLabel on /tables/{tableBucketARN}/{namespace}/{name}/maintenance/{type} targeting the TableMaintenanceType enum; errors include BadRequestException (httpError 400). Recorded response: 400 with message 'Invalid table maintenance type: <type>' - fixed prefix 'Invalid table maintenance type: ' followed by the type value taken from the request URI.
Emulator: 400 with "union 'TableMaintenanceSettings' had no member set" and x-amzn-errortype ValidationException, i.e. it reports a deserialization fact about the union body rather than refusing the unknown type.
Why: the client is told its settings union is empty when the real problem is the maintenance type in the path; the error type is also not the service's BadRequestException.
Fix: PutTableMaintenanceConfiguration's path-label validation must reject the unknown type before the body union is interpreted.
Traces: s3tables-sct-maintenance-value-mistakes#task#6.

Cause 5 of 7 - CreateNamespace with namespace sent as a bare string instead of a one-element list.
AWS: Smithy: CreateNamespace.namespace is a required NamespaceList (list, length min 1 max 1); errors include BadRequestException (httpError 400). Recorded response: 400 with message exactly 'Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API reference for constraints on request parameters.'
Emulator: 400 with 'expected array start, got string' and x-amzn-errortype ValidationException - a raw JSON-parser diagnostic.
Why: the body leaks the emulator's deserializer internals and omits the service's documented guidance text; the error type is wrong too.
Fix: CreateNamespace's body deserialization must map a shape error to the service's BadRequestException with its fixed text.
Traces: s3tables-sct-namespace-string-and-delete-while-in-use#task#2.

Cause 6 of 7 - CreateTable does not validate metadata, so an invalid metadata request succeeds and the next steps diverge.
AWS: Smithy: CreateTable.metadata targets TableMetadata; errors include BadRequestException (httpError 400). Recorded response for invalid metadata: 400 with message exactly 'The specified metadata is not valid'.
Emulator: 200 - the table is created. Because that table now exists, the fixture's subsequent step that expects the same 400 instead gets 409 ConflictException ('A table with the name <name> already exists in the namespace <namespace>.'), and the step after it, which expects a successful 200 CreateTable, also gets 409.
Why: a client's malformed metadata is silently accepted, leaving a table in a state AWS would never have created; every later request against that name then takes a conflict path that would not exist on AWS.
Fix: CreateTable's validation must reject invalid metadata (schema/partition/sort-order content) before the table is written to state.
Traces: s3tables-sct-create-table-input-mistakes#task#5 (metadata accepted, 400->200) with s3tables-sct-sort-order-at-create-via-write-order#task#3; cascades s3tables-sct-create-table-input-mistakes#task#6 (400->409, with s3tables-sct-sort-order-at-create-via-write-order#task#4) and s3tables-sct-create-table-input-mistakes#task#7 (200->409, with s3tables-sct-sort-order-at-create-via-write-order#task#5).

Cause 7 of 7 - GetTable re-validates the table name on lookup, refusing instead of reporting not-found.
AWS: Smithy: GetTable.name targets TableName with pattern ^[0-9a-z_]*$, but the recorded service does NOT turn a pattern violation on this read into a refusal: for an uppercase/hyphenated table name the recorded response is 404 NotFoundException (x-amzn-errortype 'NotFoundException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/') with message exactly 'The specified table does not exist.'
Emulator: 400 with x-amzn-errortype BadRequestException and message 'The specified table name is not valid.'
Why: the client cannot distinguish 'no such table' from 'bad request'; a 400 is non-retryable and signals a malformed call for what AWS treats as an ordinary missing resource.
Fix: GetTable's lookup path must resolve the name against state and answer NotFoundException rather than applying the input pattern as a refusal on a read.
Traces: s3tables-s3t-uppercase-and-hyphenated-table-names-rejected#task#5.

## Evidence
Cause 1 (GetTable, 400->400): expected $.message 'Invalid input: Must provide either table arn alone OR all of (table bucket arn, namespace, table name) together'; actual 'Either tableArn, or tableBucketARN, namespace and name together, must be specified.'
Cause 2 (CreateTable, 400->400): expected 'The specified open table format is not valid'; actual "1 validation error detected: Value 'iceberg' at 'format' failed to satisfy constraint: Member must satisfy enum value set: [ICEBERG]" ('iceberg' is this run's request value; the boilerplate wrapper is the defect).
Cause 3 (PutTableMaintenanceConfiguration, 400->400): expected "The specified status is invalid. Value must be either 'enabled' or 'disabled.'"; actual "1 validation error detected: Value 'ENABLED' at 'value.status' failed to satisfy constraint: Member must satisfy enum value set: [enabled, disabled]".
Cause 4 (PutTableMaintenanceConfiguration, 400->400): expected 'Invalid table maintenance type: icebergUnreferencedFileRemoval' (fixed prefix + the type from the request URI); actual "union 'TableMaintenanceSettings' had no member set", x-amzn-errortype ValidationException instead of BadRequestException.
Cause 5 (CreateNamespace, 400->400): expected 'Invalid input. Consult the S3 Tables and/or Iceberg REST Catalog API reference for constraints on request parameters.'; actual 'expected array start, got string', x-amzn-errortype ValidationException.
Cause 6 (CreateTable): expected status 400 with 'The specified metadata is not valid', actual 200 (no error body at all); the following step expected 400 with the same message and got 409 ConflictException; the step after expected 200 with tableARN/versionToken and got 409 with no body members.
Cause 7 (GetTable): expected status 404, x-amzn-errortype 'NotFoundException:http://internal.amazon.com/coral/com.amazon.s3tablebuckets/', message 'The specified table does not exist.'; actual status 400, x-amzn-errortype 'BadRequestException', message 'The specified table name is not valid.'

## Steps to Reproduce
1. Replay s3tables-s3t-get-table-with-both-the-arn-and-the-triple-rejected step task#5: GET /get-table with tableArn AND tableBucketARN+namespace+name; read the 400 message.
2. Replay s3tables-sct-create-table-input-mistakes step task#3: CreateTable with format outside the OpenTableFormat enum; read the 400 message.
3. Replay s3tables-sct-maintenance-value-mistakes step task#4: PutTableMaintenanceConfiguration with value.status outside {enabled,disabled}; read the 400 message.
4. Replay s3tables-sct-maintenance-value-mistakes step task#6: PUT .../maintenance/<unknown-type>; read the 400 message and x-amzn-errortype.
5. Replay s3tables-sct-namespace-string-and-delete-while-in-use step task#2: CreateNamespace with namespace as a JSON string; read the 400 message.
6. Replay s3tables-sct-create-table-input-mistakes steps task#5, task#6, task#7 in order: the first must be refused 400 'The specified metadata is not valid' (emulator returns 200), after which task#6 and task#7 take conflict paths.
7. Replay s3tables-s3t-uppercase-and-hyphenated-table-names-rejected step task#5: GetTable with an uppercase/hyphenated name; expect 404 NotFoundException, emulator answers 400 BadRequestException.
