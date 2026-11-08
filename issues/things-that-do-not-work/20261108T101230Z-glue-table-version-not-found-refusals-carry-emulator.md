Issue-Source: trace-sweep-divergence
Issue-Trace: glue-aorigin-table-vs-view-catalog-entries-and-history
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-aorigin-table-vs-view-catalog-entries-and-history glue-database-update-and-table
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetTableVersion|4xx->4xx|$.Message
Issue-Key: glue|BatchDeleteTableVersion|2xx->2xx|$.Errors[*].ErrorDetail.ErrorMessage
Issue-Commit: 3ca9558fed9445543f97a972598d878b9e498dd9

# Glue table-version not-found refusals carry emulator-authored message text

> _Note: this issue was identified against the code base at commit `3ca9558fed9445543f97a972598d878b9e498dd9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTableVersion

## Description
Two Glue table-version lookups refuse with the correct HTTP status and the correct error code but the WRONG message text. Glue gives each of these refusals its own sentence; the emulator substitutes a sentence of its own authorship — generic where Glue names the table, and interpolated-and-reworded where Glue returns fixed text. 2 causes, both in the table-version read/delete paths of the Data Catalog handlers.

Cause 1 of 2: GetTableVersion for a table that the catalog does not hold returns the generic 'Entity Not Found' instead of naming the table.
  AWS: Smithy: GetTableVersion (input GetTableVersionRequest; DatabaseName @required, TableName @required, VersionId optional) models EntityNotFoundException with errorKind client / httpError 400 (defaulted), whose only message member is `Message` (MessageString). The recorded 400 response carries __type 'EntityNotFoundException' and Message of the form 'Table <TableName> not found.' — the fixed text is 'Table ' and ' not found.', and the single variable part is the request's own TableName, reproduced verbatim. This is the same sentence shape the plural read GetTableVersions returns for the same fault (see open issue 20261102T132146Z-glue-entity-lookup-refusals-report-the-wrong-message.md, Cause 3, which records 'Table <tableName> not found.' for GetTableVersions). Beyond the sentence template itself the model says nothing further about the wording: recorded behavior only.
  Emulator: status 400 and __type 'EntityNotFoundException' both match, but Message is the fixed string 'Entity Not Found' with no table name interpolated at all (replay detail: `$.Message: value_mismatch (expected 'Table <tableName> not found.', got 'Entity Not Found')`).
  Why: a client that cannot read the table name out of the refusal cannot tell WHICH entity in a multi-entity request or a batch of version reads was absent, and cannot distinguish 'this table does not exist' from 'this version of an existing table does not exist' — Glue uses a different sentence for each. Code that matches the documented sentence (log scrapers, retry classifiers that only retry on a version miss, tests that assert on the refusal) sees the generic text and takes the wrong branch. It is also the exact inverse of the emulator's GetTable behaviour, which interpolates a name where Glue is generic, so neither message can be relied on in either direction.
  Fix: the GetTableVersion handler's entity-resolution/not-found path — it must build the EntityNotFoundException message from the request's TableName rather than emitting the catalog-wide generic 'Entity Not Found' constant.
  Traces: glue-aorigin-table-vs-view-catalog-entries-and-history#task#9 (representative).

Cause 2 of 2: BatchDeleteTableVersion's per-version error record reports an emulator-authored, value-interpolated ErrorMessage where Glue returns fixed text.
  AWS: Smithy: BatchDeleteTableVersion (DatabaseName @required, TableName @required, VersionIds @required, a list of 0..100) answers 200 with `Errors`, a list of TableVersionError. TableVersionError models TableName, VersionId and ErrorDetail; ErrorDetail models ErrorCode (NameString) and ErrorMessage (DescriptionString, length 0..2048). So the version id and the table name are carried in their OWN modeled members (TableVersionError.VersionId / .TableName) — they are not part of the message. The recorded 200 response carries Errors[0].ErrorDetail.ErrorMessage = 'Version not found.' — entirely fixed text with no interpolation and a terminating period. Model says nothing more about the text: recorded behavior only.
  Emulator: status 200 matches and the surrounding error record matches, but ErrorMessage is 'Table version (<VersionId>) not found' — it interpolates the requested version id into the message (duplicating the structured VersionId member), uses different words, and drops the trailing period (replay detail: `$.Errors[0].ErrorDetail.ErrorMessage: value_mismatch (expected 'Version not found.', got 'Table version (99) not found')`; the '99' is this recording's requested version id filling the emulator's template, not part of the requirement).
  Why: BatchDeleteTableVersion reports failures IN BAND at 200, so a client's only signal is this record. One that matches ErrorDetail.ErrorMessage to classify which versions were skipped never matches, so a partial-failure batch is treated as fully successful or fully failed. The redundant interpolation also means the message is unstable across requests where AWS's is constant, which breaks exact-match assertions and message-keyed metrics.
  Fix: the BatchDeleteTableVersion handler's per-version error construction — emit Glue's fixed ErrorDetail.ErrorMessage for a missing version and let TableVersionError.VersionId/.TableName carry the identity, instead of formatting the id into the message.
  Traces: glue-database-update-and-table#task#13.

Neither step is owned: the replay reports owned_by empty for both clusters, and no stub blocks either operation. The nearest open issue, 20261102T132146Z-glue-entity-lookup-refusals-report-the-wrong-message.md ('Glue entity-lookup refusals report the wrong message text and the wrong error code'), covers GetDatabase/GetTable/GetTableVersions/GetJobRun/GetSchema and does NOT carry either of these fixture steps or either of these operations, so it does not cover them.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. Service glue; both clusters verdict 'mismatch' with matching status and matching error classification, diverging only in message text.

Cause 1 — fixture glue-aorigin-table-vs-view-catalog-entries-and-history, step task#9, operation GetTableVersion, expected status 400 / actual status 400, affected_count 1:
  $.Message: value_mismatch (expected 'Table sim2real-probe-aorigin-tvv-tbl-47fb6af6-v not found.', got 'Entity Not Found')
  Fixed text: 'Table ' ... ' not found.'. Variable part: the request's TableName (here 'sim2real-probe-aorigin-tvv-tbl-47fb6af6-v', a per-run generated name — this recording's value filling the template, not a requirement). $.__type matched ('EntityNotFoundException').

Cause 2 — fixture glue-database-update-and-table, step task#13, operation BatchDeleteTableVersion, expected status 200 / actual status 200, affected_count 1:
  $.Errors[0].ErrorDetail.ErrorMessage: value_mismatch (expected 'Version not found.', got 'Table version (99) not found')
  Fixed text: the whole of 'Version not found.' — AWS interpolates nothing here. The emulator's '99' is the VersionId from this run's request; the structured TableVersionError.VersionId member already carries it.

Model evidence: smithy_query(operation="GetTableVersion,BatchDeleteTableVersion", model_path="services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json") — GetTableVersion errors include EntityNotFoundException (client, httpError 400 defaulted); BatchDeleteTableVersion output is Errors: TableVersionErrors. smithy_query(shape="TableVersionError,ErrorDetail,EntityNotFoundException") — TableVersionError{TableName, VersionId, ErrorDetail}, ErrorDetail{__type, ErrorCode, ErrorMessage(DescriptionString 0..2048)}, EntityNotFoundException{Message, FromFederationSource}.

## Steps to Reproduce
1. Clone this checkout and boot the emulator built from it.
2. Replay fixture glue-aorigin-table-vs-view-catalog-entries-and-history and stop at step task#9 (Cause 1): the fixture builds a database with a table and a view, then calls GetTableVersion naming a table the catalog does not hold. Read the 400 body — $.__type is EntityNotFoundException as expected, but $.Message is the generic 'Entity Not Found' instead of 'Table <TableName> not found.'.
3. Replay fixture glue-database-update-and-table to step task#13 (Cause 2): the fixture creates and updates a table to accumulate versions, then calls BatchDeleteTableVersion with a VersionIds entry that does not exist. Read the 200 body — Errors[0].ErrorDetail.ErrorMessage is 'Table version (<VersionId>) not found' instead of the fixed 'Version not found.'.
4. Re-replay glue-aorigin-table-vs-view-catalog-entries-and-history#task#9 and glue-database-update-and-table#task#13 to confirm a fix; both must match on message text with status and error code unchanged.
