Issue-Source: trace-sweep-divergence
Issue-Trace: glue-schema-registry-versioning
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-schema-registry-versioning glue-schema-version-lifecycle glue-schema-versions-lifecycle
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|RegisterSchemaVersion|2xx->2xx|$.Status
Issue-Key: glue|RegisterSchemaVersion|4xx->2xx|$.Message
Issue-Key: glue|RegisterSchemaVersion|4xx->2xx|$.__type
Issue-Key: glue|RegisterSchemaVersion|4xx->2xx|status
Issue-Key: glue|GetSchemaVersion|2xx->2xx|$.Status
Issue-Key: glue|ListSchemaVersions|2xx->2xx|$.Schemas[*]
Issue-Key: glue|GetSchemaVersion|4xx->4xx|$.Message
Issue-Commit: 276eddfbab43e2a0f788a4f2dd742943177ed592

# Glue schema-registry version registration never validates or fails a definition, and the version-not-found refusal is mis-worded (5 causes)

> _Note: this issue was identified against the code base at commit `276eddfbab43e2a0f788a4f2dd742943177ed592`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSchemaVersion

## Description
Five net-new root causes in the Glue Schema Registry version path. All five are in the `glue` service; four concern how a registered schema version's validity is judged and reported (RegisterSchemaVersion / GetSchemaVersion / ListSchemaVersions), the fifth is the wording of GetSchemaVersion's not-found refusal. They are filed as one bundle because they all live in the schema-version registration/read path of the glue crate. A fixer must close ALL FIVE before submitting.

Cause 1 of 5: RegisterSchemaVersion reports AVAILABLE for a version that did not pass the schema's validity/compatibility check.
  AWS: Smithy: RegisterSchemaVersion output member `Status` targets `SchemaVersionStatus`, an enum whose values are AVAILABLE, PENDING, FAILURE, DELETING; the operation models InvalidInputException, EntityNotFoundException, AccessDeniedException, ConcurrentModificationException, ResourceNumberLimitExceededException (all client, HTTP 400). Recorded behavior: when the submitted definition is registered but does not pass the schema's check, AWS answers HTTP 200 and the response's `Status` is `FAILURE` — the version exists but is not usable.
  Emulator: HTTP 200 with `Status` = `AVAILABLE`. The emulator has no notion of a registered-but-failed version, so every RegisterSchemaVersion it accepts is reported as AVAILABLE.
  Why: a client that registers a candidate definition and gates deployment on `Status != AVAILABLE` (the documented way to learn that an evolution was rejected) sees every candidate as accepted, and ships a definition the real registry would have marked FAILURE.
  Fix: RegisterSchemaVersion's handler plus the schema-registry state — a version record must carry a per-version SchemaVersionStatus that the validity check sets, not a constant AVAILABLE. The open issue 20261025T022820Z-glue-checkschemaversionvalidity-stub.md covers the separate CheckSchemaVersionValidity operation being a stub; it does not cover RegisterSchemaVersion's own status, and the open 20261025T170315Z-glue-registerschemaversion-stub.md does not either (the operation now answers 200, it is not blocked).
  Traces: glue-schema-registry-versioning#task#7 (expected Status 'FAILURE', got 'AVAILABLE').

Cause 2 of 5: RegisterSchemaVersion accepts an unparseable schema definition with HTTP 200 instead of refusing it with 400 InvalidInputException.
  AWS: Smithy: RegisterSchemaVersion.SchemaDefinition is required, targets SchemaDefinitionString (@length 1..170000, pattern \\S), and the operation models InvalidInputException (client, HTTP 400). Recorded behavior: a definition that is not parseable in the schema's DataFormat is refused with HTTP 400, `__type` = `InvalidInputException`, and Message whose FIXED text is "Schema definition of <DataFormat> data format is invalid: " followed by the parser's diagnostic for the submitted definition — the VARIABLE parts are the schema's DataFormat (AVRO in the recording) and the parser diagnostic text, including the offending token and the line/column taken from the request's own definition. Do not hard-code the recording's token or column.
  Emulator: HTTP 200 with no `__type` and no `Message` — the invalid definition is registered as a new version.
  Why: the definition is never parsed, so a malformed definition is stored. The caller believes evolution succeeded, and the bogus version then pollutes the schema's version counters and listings (corroborated by GetSchema at glue-schema-registry-versioning#task#15 reporting LatestSchemaVersion 3 / NextSchemaVersion 4 where AWS reports 2 / 3 — that read divergence is owned by 20261029T201859Z-glue-reads-report-stale-or-wrong-resource-state.md, but the extra version it reports originates here).
  Fix: RegisterSchemaVersion's input validation — parse SchemaDefinition in the schema's DataFormat before admitting a version, and raise InvalidInputException with AWS's sentence shape on a parse failure.
  Traces: glue-schema-registry-versioning#task#8 (expected 400 InvalidInputException, got 200).

Cause 3 of 5: GetSchemaVersion reports AVAILABLE for a version AWS reports as FAILURE.
  AWS: Smithy: GetSchemaVersion output member `Status` targets SchemaVersionStatus. Recorded behavior: reading back the version registered at Cause 1 returns HTTP 200 with `Status` = `FAILURE`; the status is a persisted property of the version, not a function of the read.
  Emulator: HTTP 200 with `Status` = `AVAILABLE`.
  Why: a client cannot discover after the fact that a stored version is unusable; a reader that filters out non-AVAILABLE versions keeps a version real AWS would exclude.
  Fix: GetSchemaVersion's projection must serialize the version record's stored status (which Cause 1 must start recording) instead of a constant.
  Traces: glue-schema-registry-versioning#task#9 (expected Status 'FAILURE', got 'AVAILABLE').

Cause 4 of 5: ListSchemaVersions' entries do not reflect the registered version set or their statuses.
  AWS: Smithy: ListSchemaVersions output `Schemas` is a list of SchemaVersionListItem {SchemaArn, SchemaVersionId, VersionNumber, Status, CreatedTime}; the operation is @paginated on NextToken/MaxResults. Recorded behavior: after the registrations above, the first listed entry is the schema's VersionNumber 2 carrying Status FAILURE.
  Emulator: the first listed entry is VersionNumber 1 with Status AVAILABLE — neither the entry selected at that position nor its status matches. SchemaVersionId and CreatedTime differ per run and are NOT requirements here; the requirements are the entry's VersionNumber/Status and the set of versions listed. Hypothesis: the emulator both lacks the FAILURE status (Causes 1/3) and orders/selects entries differently from AWS, so the latest version is not the one at index 0.
  Why: a client enumerating a schema's versions to find the newest usable one reads a different version than AWS serves, and never sees a FAILURE version at all.
  Fix: ListSchemaVersions' handler/projection over schema-registry state — list every stored version with its persisted status in AWS's order.
  Traces: glue-schema-registry-versioning#task#11 (expected Schemas[0] VersionNumber 2 / Status 'FAILURE', got VersionNumber 1 / Status 'AVAILABLE').

Cause 5 of 5: GetSchemaVersion's not-found refusal uses an emulator-authored sentence instead of AWS's fixed wording.
  AWS: Smithy: GetSchemaVersion models EntityNotFoundException (client, HTTP 400, doc "A specified entity does not exist") with a single `Message` member. Recorded behavior: a GetSchemaVersion whose SchemaId/SchemaVersionId/SchemaVersionNumber resolves to no stored version is refused with HTTP 400 and the FIXED sentence "Schema is not found" — the message has NO variable parts and NO trailing period.
  Emulator: HTTP 400 with the right error class but Message "Schema version is not found." — different wording and an added trailing period.
  Why: the message is a requirement of the error class; clients and SDK test suites that match on AWS's sentence (or log it for operators) see text AWS never sends, and a fixture asserting the refusal text fails against the emulator.
  Fix: GetSchemaVersion's not-found branch — emit AWS's fixed EntityNotFoundException sentence verbatim.
  Traces: glue-schema-version-lifecycle#task#6 and glue-schema-versions-lifecycle#task#4 (expected 'Schema is not found', got 'Schema version is not found.').

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, 1292 steps, 1034 passed, score 80.0%). The five clusters below carry no `owned_by` and no `stub_path`, i.e. they are net-new:

1. glue-schema-registry-versioning#task#7 — RegisterSchemaVersion, 200 -> 200: $.Status: value_mismatch (expected 'FAILURE', got 'AVAILABLE').
2. glue-schema-registry-versioning#task#8 — RegisterSchemaVersion, expected 400, got 200: $.__type missing (expected 'InvalidInputException', got None); $.Message missing (expected "Schema definition of AVRO data format is invalid: Unrecognized token 'not': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: REDACTED ...; line: 1, column: 4]", got None).
3. glue-schema-registry-versioning#task#9 — GetSchemaVersion, 200 -> 200: $.Status: value_mismatch (expected 'FAILURE', got 'AVAILABLE').
4. glue-schema-registry-versioning#task#11 — ListSchemaVersions, 200 -> 200: $.Schemas[0]: expected {VersionNumber 2, Status 'FAILURE', ...}, got {VersionNumber 1, Status 'AVAILABLE', ...}.
5. glue-schema-version-lifecycle#task#6 (also glue-schema-versions-lifecycle#task#4) — GetSchemaVersion, 400 -> 400: $.Message: value_mismatch (expected 'Schema is not found', got 'Schema version is not found.').

Model evidence (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): SchemaVersionStatus enum = [AVAILABLE, PENDING, FAILURE, DELETING]; RegisterSchemaVersion input {SchemaId required, SchemaDefinition required @length 1..170000 pattern \\S}, output {SchemaVersionId, VersionNumber, Status}, errors include InvalidInputException (client/400) and EntityNotFoundException (client/400); GetSchemaVersion output includes Status; ListSchemaVersions output Schemas: list<SchemaVersionListItem{SchemaArn, SchemaVersionId, VersionNumber, Status, CreatedTime}>, @paginated.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the glue wire fixtures.
2. Cause 1/2/3/4 — replay fixture `glue-schema-registry-versioning`: create a registry and schema, then RegisterSchemaVersion with a definition that fails the schema's check (step task#7) and observe Status AVAILABLE instead of FAILURE; RegisterSchemaVersion with an unparseable AVRO definition (step task#8) and observe HTTP 200 instead of 400 InvalidInputException; GetSchemaVersion on the failed version (step task#9) and observe Status AVAILABLE; ListSchemaVersions for the schema (step task#11) and observe Schemas[0] = VersionNumber 1 / AVAILABLE instead of VersionNumber 2 / FAILURE.
3. Cause 5 — replay fixture `glue-schema-version-lifecycle` step task#6 (or `glue-schema-versions-lifecycle` step task#4): GetSchemaVersion for a version that does not resolve and observe Message "Schema version is not found." instead of AWS's "Schema is not found".
4. Re-replay all of fixtures glue-schema-registry-versioning, glue-schema-version-lifecycle and glue-schema-versions-lifecycle to confirm a fix.
