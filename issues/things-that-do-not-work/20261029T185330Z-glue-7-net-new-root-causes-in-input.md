Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.create-database-name-too-long-fails glue.get-materialized-view-refresh-task-run-invalid-id-rejected glue.form-type-invalid-name-pattern-rejected glue.tag-resource-invalid-arn-fails glue.catalog-invalid-name-rejected glue.asset-type-lifecycle-put-get-delete-404 glue.form-type-delete-in-use-rejected glue.form-type-delete-referenced-by-asset-type-rejected glue.glossary-create-duplicate-name-rejected glue.concurrent-glossary-create-conflict-rejected glue.concurrent-glossary-term-create-conflict-rejected glue.concurrent-identity-center-configuration-create-rejected glue.column-statistics-task-settings-duplicate-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.create-database-name-too-long-fails|run#10|AssertionError|assert message == (
Issue-Key: glue.get-materialized-view-refresh-task-run-invalid-id-rejected|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.form-type-invalid-name-pattern-rejected|run#6|AssertionError|assert code == "InvalidInputException", (
Issue-Key: glue.tag-resource-invalid-arn-fails|run#4|AssertionError|assert code == "InvalidInputException", (
Issue-Key: glue.catalog-invalid-name-rejected|run#6|AssertionError|assert msg_invalid == "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name", msg_invalid
Issue-Key: glue.asset-type-lifecycle-put-get-delete-404|run#16|AssertionError|assert message == expected_message, (
Issue-Key: glue.form-type-delete-in-use-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.form-type-delete-referenced-by-asset-type-rejected|run#11|AssertionError|assert not_found_message == expected, f"expected message {expected!r}, got {not_found_message!r}"
Issue-Key: glue.glossary-create-duplicate-name-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.concurrent-glossary-create-conflict-rejected|run#12|AssertionError|assert failures == [already_exists], (
Issue-Key: glue.concurrent-glossary-term-create-conflict-rejected|run#9|AssertionError|assert failures == [already_exists], (
Issue-Key: glue.concurrent-identity-center-configuration-create-rejected|run#12|AssertionError|assert failure_messages[0] == expected[failures[0]], (
Issue-Key: glue.column-statistics-task-settings-duplicate-rejected|run#15|AssertionError|assert message == f"{ctx.account}:{db_name}-{table_a} already exists", message
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Glue: 7 net-new root causes in input validation and error classification — the generic constraint validator replaces Glue's own InvalidInputException refusals, renders the wrong member path and an elided value, and the glossary/asset-type family loses AWS's JSON message envelope

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDatabase

## Description
7 causes; all seven must be closed before submitting. They all concern how a refusal is CLASSIFIED and RENDERED, not whether the call is refused.

Related open issue, named for the fixer: 20261028T122732Z-glue-4-net-new-root-causes-four-refusal.md describes the same CreateGlueIdentityCenterConfiguration message defect as Cause 6 below, but is grounded only in glue.concurrent-identity-center-config-modification-rejected, not in the check listed here.

Cause 1 of 7: the ValidationException constraint message names the wire member path and elides the offending value.
AWS: Smithy: DatabaseInput$Name is NameString (@length 1..255); CreateDatabase and GetDatabase both reject a 300-character name with ValidationException (client, HTTP 400). Real Glue's message is `1 validation error detected: Value '<the full offending value>' at '<path>' failed to satisfy constraint: Member must have length less than or equal to 255`, where <path> is AWS's own request path — `database.name` for CreateDatabase and `name` for GetDatabase — and <the full offending value> is reproduced IN FULL, not summarized.
Emulator: code ValidationException is right, but the path is the Smithy member path `databaseInput.name` and the value is replaced by a summary: `Value 'aaaa…aaa... [300 bytes]' at 'databaseInput.name' failed to satisfy constraint: Member must have length less than or equal to 255`.
Why: a client that parses the validation message to learn which field it got wrong is told a member name AWS never uses, and the offending value is not recoverable from the message.
Fix: the shared constraint-violation message renderer used by the glue crate's request validation — it must map each violated member to AWS's documented request path per operation and emit the value verbatim.
Traces: glue.create-database-name-too-long-fails (asserts both the CreateDatabase and the GetDatabase form).

Cause 2 of 7: the pattern-constraint message includes the regex anchors AWS omits.
AWS: Smithy: GetMaterializedViewRefreshTaskRun's run-id member carries a @pattern; real Glue reports a violation as `1 validation error detected: Value '<value>' at 'materializedViewRefreshTaskRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}` — the pattern is quoted WITHOUT the leading ^ and trailing $.
Emulator: same code and path, but the pattern is quoted with the anchors: `^[0-9a-fA-F]{8}-...-[0-9a-fA-F]{12}$`.
Why: the message is the contract a client reads the expected format from; the anchored form is text AWS never emits, and clients that compare the advertised pattern to their own do not match.
Fix: the same constraint-violation renderer as Cause 1 must strip the anchors when quoting a @pattern.
Traces: glue.get-materialized-view-refresh-task-run-invalid-id-rejected.

Cause 3 of 7: generic ValidationException is returned where Glue raises InvalidInputException.
AWS: Smithy: PutFormType models InvalidInputException (client, HTTP 400) and its Name has @pattern `^(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$`-style form-type naming; TagResource models InvalidInputException as well. A form-type Name that violates the name pattern, and a TagResource ResourceArn that is not a well-formed Glue ARN, are both refused by real Glue with code `InvalidInputException` — Glue validates these in the handler and does not surface them as the generic ValidationException.
Emulator: both are refused with code `ValidationException`.
Why: the error CODE is what SDK error-type branching and retry classification use; a client catching Glue's InvalidInputException sees an unmodeled-for-this-case ValidationException instead and takes the wrong branch.
Fix: move these two validations out of the generic request validator into the PutFormType and TagResource handlers so they raise InvalidInputException (services/service-com-amazonaws-glue-awsglue/src/ops/put_form_type.rs, tag_resource.rs).
Traces: glue.form-type-invalid-name-pattern-rejected (Name='lowercase…' violating ^[A-Z]\w*$), glue.tag-resource-invalid-arn-fails (malformed ARN).

Cause 4 of 7: CreateCatalog's invalid-name refusal is the generic pattern message instead of Glue's sentence.
AWS: Smithy: CreateCatalog models InvalidInputException (client, HTTP 400). A catalog Name containing a character Glue forbids (here a '.') is refused with the fixed Glue sentence `Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name` — not a constraint-violation report.
Emulator: the generic validator answers first with `1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!(.*[.\/\\]|aws:)).*$`.
Why: the caller is given a regex instead of the rule AWS states, and nothing keyed on Glue's sentence matches.
Fix: the CreateCatalog handler's name validation (services/service-com-amazonaws-glue-awsglue/src/ops/create_catalog.rs) must own this rejection and emit the fixed sentence.
Traces: glue.catalog-invalid-name-rejected.

Cause 5 of 7: the glossary / glossary-term / asset-type / form-type family drops AWS's JSON message envelope and rewrites the sentence.
AWS: recorded behavior for this Glue business-metadata family: the error payload's Message is itself a JSON document. A missing asset type is refused with EntityNotFoundException whose Message is the string `{"Message":"Asset type <name> not found."}` (fixed envelope `{"Message":"Asset type ` + the requested name + ` not found."}`); a duplicate glossary name is refused with AlreadyExistsException whose Message is `{"Message":"A glossary with name '<name>' already exists."}`; a duplicate glossary term likewise carries the term-specific sentence inside the same envelope.
Emulator: the codes are right but every message is a bare sentence with no envelope and different wording: `Asset type <name> not found`, `Glossary already exists.`, `Glossary term already exists.`
Why: clients of this family parse Message as JSON to read the service's sentence; a bare string makes that parse fail outright, and the wording differs on top of it. The duplicate-glossary divergence is also what makes both concurrency checks fail even though the emulator serializes the race correctly (exactly one winner).
Fix: the error-rendering path shared by the glossary / glossary-term / asset / asset-type / form-type handlers of the glue crate must emit AWS's JSON-enveloped message, with the requested name interpolated.
Traces: glue.asset-type-lifecycle-put-get-delete-404, glue.form-type-delete-in-use-rejected, glue.form-type-delete-referenced-by-asset-type-rejected, glue.glossary-create-duplicate-name-rejected, glue.concurrent-glossary-create-conflict-rejected, glue.concurrent-glossary-term-create-conflict-rejected.

Cause 6 of 7: CreateGlueIdentityCenterConfiguration's duplicate refusal omits the ApplicationArn.
AWS: Smithy: CreateGlueIdentityCenterConfiguration models AlreadyExistsException and ConcurrentModificationException (both client, HTTP 400) and its output carries ApplicationArn. Recorded behavior: a caller losing the race for the account singleton is refused with AlreadyExistsException whose message is fixed text `Glue Idc Application already exists. ApplicationArn: ` + the ApplicationArn of the configuration that already exists (taken from emulator state).
Emulator: AlreadyExistsException with `Glue Identity Center configuration already exists.` — different fixed text, no ApplicationArn.
Why: the caller cannot learn which application owns the singleton, and the documented sentence never matches.
Fix: the duplicate/concurrency path of the CreateGlueIdentityCenterConfiguration handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_glue_identity_center_configuration.rs).
Traces: glue.concurrent-identity-center-configuration-create-rejected.

Cause 7 of 7: CreateColumnStatisticsTaskSettings' duplicate refusal message is emulator-authored.
AWS: Smithy: CreateColumnStatisticsTaskSettings models AlreadyExistsException (client, HTTP 400) and is keyed on DatabaseName+TableName. A second create for the same table is refused with AlreadyExistsException whose message is the fixed text real Glue returns for an existing settings record, naming the table the settings belong to — not the emulator's generic sentence.
Emulator: AlreadyExistsException with `Column statistics task settings already exist.`
Why: a client cannot recognize the duplicate-settings condition, nor learn which table it refers to.
Fix: the duplicate path of the CreateColumnStatisticsTaskSettings handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_column_statistics_task_settings.rs).
Traces: glue.column-statistics-task-settings-duplicate-rejected.

## Evidence
Cause 1 — glue.create-database-name-too-long-fails (300-char name):
  expected: ValidationException / "1 validation error detected: Value '<full 300-char value>' at 'database.name' failed to satisfy constraint: Member must have length less than or equal to 255"
            (and 'name' as the path for the same value through GetDatabase)
  actual:   ValidationException / "1 validation error detected: Value 'aaaa…aaa... [300 bytes]' at 'databaseInput.name' failed to satisfy constraint: Member must have length less than or equal to 255"
Cause 2 — glue.get-materialized-view-refresh-task-run-invalid-id-rejected:
  expected pattern quoted as [0-9a-fA-F]{8}-…-[0-9a-fA-F]{12}; actual quoted as ^[0-9a-fA-F]{8}-…-[0-9a-fA-F]{12}$
Cause 3 — glue.form-type-invalid-name-pattern-rejected: expected InvalidInputException, got ValidationException
        glue.tag-resource-invalid-arn-fails: expected InvalidInputException for malformed ARN, got ValidationException
Cause 4 — glue.catalog-invalid-name-rejected:
  expected: 'Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name'
  actual:   "1 validation error detected: Value '<name>.invalid' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!(.*[.\/\\]|aws:)).*$"
Cause 5 — glue.asset-type-lifecycle-put-get-delete-404: expected '{"Message":"Asset type <id> not found."}', got 'Asset type <id> not found'
        glue.form-type-delete-in-use-rejected / -referenced-by-asset-type-rejected: same envelope divergence
        glue.glossary-create-duplicate-name-rejected: expected '{"Message":"A glossary with name \'<name>\' already exists."}', got 'Glossary already exists.'
        glue.concurrent-glossary-create-conflict-rejected: exactly one racer won (correct), but the loser's message was 'Glossary already exists.'
        glue.concurrent-glossary-term-create-conflict-rejected: loser's message 'Glossary term already exists.'
Cause 6 — glue.concurrent-identity-center-configuration-create-rejected: actual 'Glue Identity Center configuration already exists.'
Cause 7 — glue.column-statistics-task-settings-duplicate-rejected: actual 'Column statistics task settings already exist.'

## Steps to Reproduce
Cause 1: CreateDatabase(DatabaseInput={'Name': 'a'*300}) -> ValidationException whose Message quotes the full 300-char value at path 'database.name'; GetDatabase(Name='a'*300) -> same, at path 'name'.
Cause 2: GetMaterializedViewRefreshTaskRun(...RunId='not-a-valid-uuid') -> ValidationException whose quoted pattern has no ^ or $.
Cause 3: PutFormType(Name='lowercasename', Schema=...) -> InvalidInputException (not ValidationException); TagResource(ResourceArn='not-an-arn', TagsToAdd={'k':'v'}) -> InvalidInputException.
Cause 4: CreateCatalog(Name='<name>.invalid', CatalogInput={...}) -> InvalidInputException / 'Only lowercase letters, numbers, underscores and hyphens are allowed for catalog name'.
Cause 5: GetAssetType(Identifier=<unknown>) and DeleteFormType(Name=<referenced by an asset type>) -> Message must be the JSON document '{"Message":"Asset type <name> not found."}'; CreateGlossary(Name=G) twice, and two concurrent CreateGlossary(Name=G) -> the loser's Message must be '{"Message":"A glossary with name \'G\' already exists."}'; same for CreateGlossaryTerm.
Cause 6: fire concurrent CreateGlueIdentityCenterConfiguration(InstanceArn=<sso instance>) calls -> each loser's Message must be 'Glue Idc Application already exists. ApplicationArn: <winner ApplicationArn>'.
Cause 7: CreateColumnStatisticsTaskSettings(DatabaseName=D, TableName=T, Role=R, Schedule=...) twice -> the second must be AlreadyExistsException with AWS's settings-already-exist text naming the table.
