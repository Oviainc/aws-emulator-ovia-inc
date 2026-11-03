Issue-Source: trace-sweep-divergence
Issue-Trace: glue-import-catalog-to-glue
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-import-catalog-to-glue glue-database-update-and-table glue-database-update-and-table-batch-delete glue-classifier-custom-entity-and-data-quality-ruleset glue-gjwr-traj-hand-written-job-definition-refusals
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetCatalogImportStatus|2xx->2xx|$.ImportStatus.ImportedBy
Issue-Key: glue|GetCatalogImportStatus|2xx->2xx|$.ImportStatus.ImportTime
Issue-Key: glue|UpdateDataQualityRuleset|2xx->2xx|$.Identifier
Issue-Key: glue|CreateJob|4xx->4xx|$.Message
Issue-Commit: 4988eaf067dbbfdfac9330bbeed709d39fc37f35

# Glue emulator-authored values in GetCatalogImportStatus, UpdateDataQualityRuleset and the CreateJob capacity refusal (4 causes)

> _Note: this issue was identified against the code base at commit `4988eaf067dbbfdfac9330bbeed709d39fc37f35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCatalogImportStatus

## Description
Four net-new divergences in service-com-amazonaws-glue-awsglue share one fault: the response value (or refusal message) the emulator emits is one it authored itself rather than the value/text real Glue returns for that path. The HTTP status and error code are right in every case; what diverges is the payload. FOUR causes must be closed before this issue is submitted.

Related open issues cover OTHER paths and do NOT carry these fixture steps' replay keys, so none of them covers these causes: 20261029T201859Z-glue-delete-and-update-mutations-are-never-applied.md (GetDataQualityRuleset's omitted Identifier, not UpdateDataQualityRuleset's wrong one), 20261029T201859Z-glue-responses-omit-modeled-members-whose-value-is.md (classifier/security-configuration members), and the behavioral issue 20261102T114851Z-glue-8-net-new-root-causes-refusal-messages.md, whose Cause 1 names the same CreateJob capacity sentence but is grounded only in behavioral check ids — the wire step below is not bound to it, so a fix verified against that issue alone never re-replays this fixture.

Cause 1 of 4: GetCatalogImportStatus reports the caller as the importer instead of Glue's default sentinel.
AWS: Smithy: GetCatalogImportStatus -> GetCatalogImportStatusResponse.ImportStatus -> CatalogImportStatus{ImportCompleted: Boolean (default false), ImportTime: Timestamp, ImportedBy: NameString 'The name of the person who initiated the migration.'}; the operation declares only InternalServiceException (500) and OperationTimeoutException (400) — the read never fails for a never-migrated account. Recorded behavior only: Glue reports the account's catalog-migration status with ImportedBy set to the fixed service sentinel 'StatusSetByDefault' when the status was not set by a real migration — fixed text with no variable part, in particular NOT a caller or principal identity.
Emulator: 200 with $.ImportStatus.ImportedBy set to the emulator's own principal ARN (recorded: 'arn:aws:iam::000000000000:user/emulator'), i.e. the request's identity interpolated into the field.
Why: a client that distinguishes 'this catalog's import status is a default, no migration ever ran' from 'a principal P ran the migration' reads the second where AWS reports the first, and any assertion on Glue's sentinel fails. The emulated value also leaks the emulator's fixed principal.
Fix: the GetCatalogImportStatus handler / the catalog-import state it reads — the ImportedBy default must be the sentinel, not the request principal.
Traces: glue-import-catalog-to-glue#task#2

Cause 2 of 4: GetCatalogImportStatus omits ImportTime entirely.
AWS: Smithy: CatalogImportStatus.ImportTime (Timestamp) — 'The time that the migration was started.' Recorded behavior: ImportStatus always carries an ImportTime timestamp alongside ImportCompleted/ImportedBy. The REQUIREMENT is that the member is present and carries a timestamp; the particular epoch in the recording (1786091983.0) is a recorded value, not part of the contract.
Emulator: 200 with $.ImportStatus.ImportTime absent from the response body.
Why: a client that reads ImportStatus.ImportTime to decide when (or whether) the catalog migration was last stamped gets a null/absent field, and strongly typed SDK consumers that treat the member as always-populated break. An omitted member is observationally different from any value.
Fix: the GetCatalogImportStatus serializer / the catalog-import state record — populate ImportTime for the default status as well as after ImportCatalogToGlue.
Traces: glue-database-update-and-table#task#8 (also glue-database-update-and-table-batch-delete#task#8)

Cause 3 of 4: UpdateDataQualityRuleset answers with a freshly minted identifier instead of the ruleset's own.
AWS: Smithy: UpdateDataQualityRuleset(Name required, Description, Ruleset) -> UpdateDataQualityRulesetResponse{Name, Description, Ruleset}, errors include EntityNotFoundException and InvalidInputException (both client, 400). Recorded behavior only (the member is not in the modeled output shape): the recorded response carries an Identifier equal to the identifier the ruleset already has — the same value CreateDataQualityRuleset returned for that ruleset earlier in the fixture (captured by the corpus as dq_ruleset_identifier). The requirement is the IDENTITY relation: the value echoes the stored ruleset's identifier, never a per-call value.
Emulator: 200 with $.Identifier set to a newly generated 32-hex digest (recorded: '5b489c2cd40a5f7f8b59982e691b5509') that matches neither the identifier returned at creation nor the one any subsequent read reports.
Why: a client that updates a ruleset and then uses the returned Identifier to read, evaluate or delete it addresses a resource that does not exist; the update response cannot be chained, and it falsely implies the update replaced the resource with a new one.
Fix: the UpdateDataQualityRuleset handler — echo the stored ruleset's identifier from state rather than minting one per call.
Traces: glue-classifier-custom-entity-and-data-quality-ruleset#task#14

Cause 4 of 4: CreateJob's mutually-exclusive-capacity refusal message is emulator-authored.
AWS: Smithy: CreateJob input carries MaxCapacity (NullableDouble), WorkerType (WorkerType enum) and NumberOfWorkers (NullableInteger), and declares InvalidInputException (errorKind client, httpError 400). Setting MaxCapacity together with WorkerType/NumberOfWorkers is refused 400 InvalidInputException with the fixed sentence 'Please do not set Max Capacity if using Worker Type and Number of Workers.' — entirely fixed text, no variable part: spaced-out English member names and a terminating period.
Emulator: the same status and code, but the message 'Please do not set MaxCapacity if using WorkerType and NumberOfWorkers' — the API member spellings concatenated and no trailing period.
Why: an HTTP status, an error code and the error message are all requirements. A client, runbook or log scraper matching Glue's sentence does not recognise the refusal, and because every word boundary differs no substring match rescues it; an operator shown the text sees wording AWS never emits.
Fix: the CreateJob handler's capacity-validation branch, where this message is composed.
Traces: glue-gjwr-traj-hand-written-job-definition-refusals#task#7

## Evidence
Cause 1 — fixture glue-import-catalog-to-glue, step task#2, GetCatalogImportStatus: expected status 200 and $.ImportStatus.ImportedBy == 'StatusSetByDefault'; got status 200 and $.ImportStatus.ImportedBy == 'arn:aws:iam::000000000000:user/emulator' (replay detail: "$.ImportStatus.ImportedBy: value_mismatch"). Fixed text 'StatusSetByDefault'; no variable parts.

Cause 2 — fixture glue-database-update-and-table, step task#8, GetCatalogImportStatus: expected $.ImportStatus.ImportTime to be present with a timestamp (recording shows 1786091983.0 — a recorded value, not a requirement); got the member absent (replay detail: "$.ImportStatus.ImportTime: missing ... got None"). Same cluster: glue-database-update-and-table-batch-delete#task#8.

Cause 3 — fixture glue-classifier-custom-entity-and-data-quality-ruleset, step task#14, UpdateDataQualityRuleset: expected $.Identifier == the identifier captured from the preceding CreateDataQualityRuleset for the same ruleset (corpus variable {{dq_ruleset_identifier}}); got $.Identifier == '5b489c2cd40a5f7f8b59982e691b5509', a value minted for this call. Status 200 on both sides.

Cause 4 — fixture glue-gjwr-traj-hand-written-job-definition-refusals, step task#7, CreateJob: both sides 400 with code InvalidInputException; expected $.Message == 'Please do not set Max Capacity if using Worker Type and Number of Workers.'; got $.Message == 'Please do not set MaxCapacity if using WorkerType and NumberOfWorkers'. Entire message is fixed text on AWS's side.

## Steps to Reproduce
Replay the simparity-wire-aws-v1 wire corpus against the emulator and re-run the four fixture steps named above; or by hand against the emulator's Glue endpoint:
1. GetCatalogImportStatus(CatalogId=<account>) on an account that has run no catalog migration -> read ImportStatus.ImportedBy (expect 'StatusSetByDefault'). (fixture glue-import-catalog-to-glue, task#2)
2. GetCatalogImportStatus(CatalogId=<account>) -> read ImportStatus.ImportTime (expect a timestamp member to be present). (fixture glue-database-update-and-table, task#8)
3. CreateDatabase + CreateTable, then CreateDataQualityRuleset(Name=<r>, Ruleset='Rules = [ ColumnExists "id" ]', TargetTable={DatabaseName,TableName}) and record the returned identifier; then UpdateDataQualityRuleset(Name=<r>, Ruleset=<new DQDL>) and compare the returned Identifier with the recorded one. (fixture glue-classifier-custom-entity-and-data-quality-ruleset, task#14)
4. CreateJob(Name=<j>, Role=<role>, Command={Name:'glueetl',ScriptLocation:...}, GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, MaxCapacity=2.0) -> read Error.Code and Error.Message. (fixture glue-gjwr-traj-hand-written-job-definition-refusals, task#7)
