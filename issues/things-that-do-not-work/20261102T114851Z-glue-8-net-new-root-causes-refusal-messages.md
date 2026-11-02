Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.gjwr-create-job-max-capacity-alongside-workers-rejected glue.gjwr-get-tags-with-other-regions-job-arn-rejected glue.glue-create-table-name-over-255-characters-rejected glue.glue-tsd-create-table-overlong-column-name-rejected glue.glue-tsd-get-tables-backwards-character-range-rejected glue.glue-recreated-table-version-history-restarts-at-zero glue.partition-create-get-delete glue.create-glossary-concurrent-create-race-rejected glue.server-side-encryption-enforced-on-new-tables
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.gjwr-create-job-max-capacity-alongside-workers-rejected|run#6|AssertionError|assert got == (
Issue-Key: glue.gjwr-get-tags-with-other-regions-job-arn-rejected|run#9|AssertionError|assert (code, message) == ("EntityNotFoundException", "Job not found."), (
Issue-Key: glue.glue-create-table-name-over-255-characters-rejected|run#11|AssertionError|assert got == ("ValidationException",
Issue-Key: glue.glue-tsd-create-table-overlong-column-name-rejected|run#11|AssertionError|assert got == ("ValidationException",
Issue-Key: glue.glue-tsd-get-tables-backwards-character-range-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Key: glue.glue-recreated-table-version-history-restarts-at-zero|run#17|AssertionError|assert got == ("EntityNotFoundException", "Version not found."), (
Issue-Key: glue.partition-create-get-delete|run#20|AssertionError|assert update_message == expected_message, (
Issue-Key: glue.create-glossary-concurrent-create-race-rejected|run#10|AssertionError|assert failures[0][1] == expected, f"expected {expected!r}, got {failures[0][1]!r}"
Issue-Key: glue.server-side-encryption-enforced-on-new-tables|run#28|AssertionError|assert re.fullmatch(
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# Glue: 8 net-new root causes — refusal MESSAGES are emulator-authored instead of the text real Glue returns (CreateJob capacity, GetTags, CreateTable validation path, GetTables Expression, GetTableVersion, UpdatePartition, CreateGlossary, GlueEncryptionException)

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Eight distinct refusal paths in service-com-amazonaws-glue-awsglue answer with the correct HTTP status and (except where noted) the correct error code, but with a MESSAGE the emulator authored rather than the message real Glue returns. An HTTP status, an error code AND the error message are requirements, so each of these is a defect in its own right. Related open issues cover OTHER operations' messages (20261029T185330Z-glue-10-net-new-root-causes-ten-refusal.md, 20261029T201859Z-glue-not-found-refusals-carry-emulator-authored-messages.md, 20261030T182934Z-glue-getblueprint-s-entitynotfoundexception-message-is-emulator-authored.md); none of them names the eight operations below, so none covers these causes. EIGHT causes must be closed before this issue is submitted.

Cause 1 of 8: CreateJob — MaxCapacity set together with WorkerType/NumberOfWorkers.
AWS: Smithy: CreateJob input carries MaxCapacity (NullableDouble), WorkerType (WorkerType enum) and NumberOfWorkers (NullableInteger); the two capacity models are mutually exclusive on Glue 2.0+ and the combination is refused with the modeled InvalidInputException (errorKind client, httpError 400). Recorded message, fixed text with no variable parts: "Please do not set Max Capacity if using Worker Type and Number of Workers." (spaced words matching the console wording, terminated by a period).
Emulator: 400 InvalidInputException with "Please do not set MaxCapacity if using WorkerType and NumberOfWorkers" — the API member spellings concatenated, and no trailing period.
Why: a client or runbook that matches Glue's documented sentence (or shows it to an operator) does not recognise the refusal, and the two texts differ in every word boundary, so no substring match rescues it.
Fix: the CreateJob handler's capacity-validation branch, which composes this message.
Traces: glue.gjwr-create-job-max-capacity-alongside-workers-rejected

Cause 2 of 8: GetTags for a job ARN that names another region.
AWS: Smithy: GetTags.ResourceArn is GlueResourceArn (pattern ^arn:aws(-(cn|us-gov|iso(-[bef])?))?:glue:), and the operation declares EntityNotFoundException (client, 400) — 'A specified entity does not exist'. An ARN is resolved only at the endpoint of the region it names, and the refusal message is the fixed sentence "Job not found." with NO variable part: the requested ARN is not echoed.
Emulator: 400 EntityNotFoundException with "Entity not found for resource <the ARN from the request>" (recorded: 'Entity not found for resource arn:aws:glue:us-west-2:000000000000:job/<job-name>') — a request-derived sentence naming the generic word 'resource' instead of the entity type.
Why: callers cannot distinguish 'that job does not exist here' from any other missing Glue entity, and message assertions/log scrapers keyed on Glue's fixed text fail.
Fix: the GetTags handler's not-found branch (message text; the ARN must not be interpolated).
Traces: glue.gjwr-get-tags-with-other-regions-job-arn-rejected

Cause 3 of 8: CreateTable constraint-validation messages are rooted at the wrong member path.
AWS: Smithy: CreateTable.TableInput -> TableInput; TableInput.Name targets NameString (@length min 1 max 255); a column name is also capped at 255 on this path. The refusal is ValidationException and the message is the standard framework sentence "1 validation error detected: Value '<value>' at '<path>' failed to satisfy constraint: Member must have length less than or equal to 255", where <path> is rooted at the segment 'table' — recorded as 'table.name' for the table name and 'table.storageDescriptor.columns.1.member.name' for the column name. Fixed text: everything except <value> and <path>; variable parts: the offending value from the request and the index inside the columns list.
Emulator: the fixed text, the value and the constraint are right, but the path is rooted at 'tableInput' — 'tableInput.name' and 'tableInput.storageDescriptor.columns.1.member.name'.
Why: the path is the machine-readable part of a validation message — it is what tooling parses to point an operator at the offending field — and every path the emulator emits for this input names a member path that AWS never produces.
Fix: the CreateTable input validation layer, where the root segment of the validation path is derived from the input member name instead of Glue's 'table'. Applies to every nested path under the table input, not only the two recorded here.
Traces: glue.glue-create-table-name-over-255-characters-rejected, glue.glue-tsd-create-table-overlong-column-name-rejected

Cause 4 of 8: GetTables with an Expression that is not a valid pattern.
AWS: Smithy: GetTables.Expression targets FilterString (length 0-2048) and the operation declares InvalidInputException (client, 400). Recorded behavior only: Glue compiles the expression with the Java regex engine and relays that engine's syntax description as the message — for a backwards character class range the message is "Illegal character range near index <i>\n<expression>\n<i spaces>^", where the fixed text is 'Illegal character range near index ' and the caret line layout, and the variable parts are the index of the range's high bound (one past the '-') and the expression from the request.
Emulator: the code is right (InvalidInputException) but the message is the Rust regex crate's diagnostic, including an emulator-internal rewritten pattern: 'Expression "t[b-a]" is not a valid regular expression: regex parse error:\n    ^(?:t[b-a])$\n          ^^^\nerror: invalid character class range, the start must be <= the end'.
Why: the message leaks the emulator's own anchoring/rewriting of the caller's expression (^(?:...)$) and an engine name AWS never mentions, so a caller cannot locate the fault at the index AWS reports, and the text identifies the implementation.
Fix: the GetTables Expression compilation/validation path — translate the pattern failure into Glue's Java-style syntax message instead of forwarding the regex engine's error.
Traces: glue.glue-tsd-get-tables-backwards-character-range-rejected

Cause 5 of 8: GetTableVersion for a version that no longer exists.
AWS: Smithy: GetTableVersion declares EntityNotFoundException (client, 400). DeleteTable drops the table's version history, so a version id of the deleted table is refused with the fixed message "Version not found." (no variable parts).
Emulator: 400 EntityNotFoundException with "Table version not found."
Why: the code matches but the message does not, so callers asserting on or displaying Glue's text diverge; the message is a requirement of the refusal.
Fix: the GetTableVersion handler's not-found branch message.
Traces: glue.glue-recreated-table-version-history-restarts-at-zero

Cause 6 of 8: UpdatePartition for a partition that was deleted.
AWS: Smithy: UpdatePartition requires DatabaseName, TableName, PartitionValueList (BoundedPartitionValueList) and PartitionInput, and declares EntityNotFoundException (client, 400). The refusal names the partition it could not find: message "Partition [<partition values>] not found" — fixed text 'Partition [' / '] not found' with the requested partition value list inside the brackets (recorded with a single-key partition, so a one-element list).
Emulator: 400 EntityNotFoundException with the fixed, value-free sentence "Cannot find partition."
Why: with multiple partitions in flight the caller cannot tell WHICH partition the refusal is about, and the wording does not match AWS's.
Fix: the UpdatePartition handler's not-found branch — compose the message from the request's PartitionValueList.
Traces: glue.partition-create-get-delete

Cause 7 of 8: CreateGlossary for a name that already exists (duplicate / concurrent race loser).
AWS: Smithy: CreateGlossary (POST /glossaries) declares AlreadyExistsException (client, 400) — 'A resource to be created or added already exists.' Recorded behavior: the refusal's message is the JSON error body itself, '{"Message":"A glossary with name \'<name>\' already exists."}' — fixed text apart from <name>, which is the Name from the request.
Emulator: the code is right (AlreadyExistsException, and exactly one of two concurrent creates wins) but the message is the emulator-authored "Glossary already exists.", which neither carries the JSON envelope nor names the glossary.
Why: a client that parses this message as JSON (which AWS's shape of it invites) fails outright, and the refusal does not identify the conflicting name.
Fix: the CreateGlossary handler's duplicate-name branch message.
Traces: glue.create-glossary-concurrent-create-race-rejected

Cause 8 of 8: GetTable while the catalog's SSE-KMS CMK is disabled — GlueEncryptionException does not relay the KMS refusal.
AWS: Smithy: GlueEncryptionException (client, 400) — 'An encryption operation failed.' Recorded behavior: with catalog encryption at rest set to SSE-KMS, a read whose CMK is disabled fails with GlueEncryptionException whose message is the relayed KMS error, '<key ARN> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <id>; Proxy: null)'. Fixed text: ' is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: ' / '; Proxy: null)'. Variable parts: the CMK ARN from catalog state and the per-call KMS request id (a per-run value — only its presence and shape are required, never a literal id).
Emulator: correct code, but the message is truncated to '<key ARN> is disabled.' — the relayed Service/Status Code/Error Code/Request ID suffix is absent.
Why: operators and clients diagnose this from the relayed KMS error code (DisabledException) carried in the message; without it the Glue failure cannot be attributed to the key's state.
Fix: the catalog-encryption path behind GetTable (the GlueEncryptionException construction) — relay the underlying KMS error rather than only its leading sentence.
Traces: glue.server-side-encryption-enforced-on-new-tables

## Evidence
Cause 1 — expected ('InvalidInputException', 'Please do not set Max Capacity if using Worker Type and Number of Workers.'); got ('InvalidInputException', 'Please do not set MaxCapacity if using WorkerType and NumberOfWorkers').
Cause 2 — expected ('EntityNotFoundException', 'Job not found.'); got ('EntityNotFoundException', 'Entity not found for resource arn:aws:glue:us-west-2:000000000000:job/sim2real-gjwr-arn-<id>').
Cause 3 — expected ('ValidationException', "1 validation error detected: Value '<256 chars>' at 'table.name' failed to satisfy constraint: Member must have length less than or equal to 255"); got the same sentence with "at 'tableInput.name'". Column arm: expected "at 'table.storageDescriptor.columns.1.member.name'"; got "at 'tableInput.storageDescriptor.columns.1.member.name'".
Cause 4 — expected message 'Illegal character range near index 4\nt[b-a]\n    ^'; got 'Expression "t[b-a]" is not a valid regular expression: regex parse error:\n    ^(?:t[b-a])$\n          ^^^\nerror: invalid character class range, the start must be <= the end'.
Cause 5 — expected ('EntityNotFoundException', 'Version not found.'); got ('EntityNotFoundException', 'Table version not found.').
Cause 6 — expected UpdatePartition message 'Partition [2023] not found'; got 'Cannot find partition.'.
Cause 7 — expected '{"Message":"A glossary with name \'sim2real-glue-glossary-race-<id>\' already exists."}'; got 'Glossary already exists.'.
Cause 8 — expected the GlueEncryptionException message to match '<key ARN> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <id>; Proxy: null)'; got 'arn:aws:kms:us-west-2:000000000000:key/<uuid> is disabled.'.

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks against the emulator.
1. CreateJob(Name, Role, GlueVersion='4.0', WorkerType='G.1X', NumberOfWorkers=2, MaxCapacity=2.0) -> read Error.Message.
2. From the us-east-1 endpoint, GetTags(ResourceArn='arn:aws:glue:us-west-2:<acct>:job/<name>') for a job that exists in neither region.
3. CreateTable with a 256-character TableInput.Name; then CreateTable with a 256-character second column name; read Error.Message in both.
4. GetTables(DatabaseName=<db>, Expression='t[b-a]').
5. CreateTable -> DeleteTable -> CreateTable under the same name, then GetTableVersion(VersionId='1') of the dropped history.
6. CreatePartition -> DeletePartition -> UpdatePartition for the same PartitionValueList.
7. Two concurrent CreateGlossary calls with the same Name; read the loser's Error.Message.
8. kms:CreateKey, PutDataCatalogEncryptionSettings(EncryptionAtRest={CatalogEncryptionMode:'SSE-KMS', SseAwsKmsKeyId:<key>}), create a table, kms:DisableKey, then GetTable and read the GlueEncryptionException message.
