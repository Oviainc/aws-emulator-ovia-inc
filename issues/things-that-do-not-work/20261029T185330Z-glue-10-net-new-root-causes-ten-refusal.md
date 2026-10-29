Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.get-table-not-found glue.reject-table-in-nonexistent-database glue.database-delete-cascades-to-tables glue.get-dashboard-url-unknown-resource-rejected glue.connection-create-get-delete glue.reject-get-nonexistent-table-version glue.reject-partition-values-count-mismatch glue.reject-partition-on-unpartitioned-table glue.create-session-duplicate-id-rejected glue.create-session-idempotent-mismatch-rejected glue.create-session-idempotent-token-mismatch-rejected glue.session-duplicate-id-rejected glue.catalog-duplicate-create-rejected glue.blueprint-duplicate-name-rejected glue.create-trigger-conditional-without-predicate-fails glue.create-trigger-invalid-cron-fails glue.create-crawler-no-targets-fails
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.get-table-not-found|run#14|AssertionError|assert db_message == expected, f"expected message {expected!r}, got {db_message!r}"
Issue-Key: glue.reject-table-in-nonexistent-database|run#22|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: glue.database-delete-cascades-to-tables|run#17|AssertionError|assert tbl_message == "Entity Not Found", f"unexpected not-found message {tbl_message!r}"
Issue-Key: glue.get-dashboard-url-unknown-resource-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.connection-create-get-delete|run#15|AssertionError|assert delete_msg == "Connection not found", f"unexpected delete_connection message: {delete_msg!r}"
Issue-Key: glue.reject-get-nonexistent-table-version|run#15|AssertionError|assert message == "Version not found.", message
Issue-Key: glue.reject-partition-values-count-mismatch|run#12|AssertionError|assert message == expected_create_message, (
Issue-Key: glue.reject-partition-on-unpartitioned-table|run#16|AssertionError|assert message == "Cannot create partitions in table without partition keys", message
Issue-Key: glue.create-session-duplicate-id-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.create-session-idempotent-mismatch-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.create-session-idempotent-token-mismatch-rejected|run#11|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: glue.session-duplicate-id-rejected|run#10|AssertionError|assert message == f"Session already created, sessionId={session_id}", message
Issue-Key: glue.catalog-duplicate-create-rejected|run#16|AssertionError|assert message == expected_message, (
Issue-Key: glue.blueprint-duplicate-name-rejected|run#9|AssertionError|assert message == expected_message, (
Issue-Key: glue.create-trigger-conditional-without-predicate-fails|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.create-trigger-invalid-cron-fails|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.create-crawler-no-targets-fails|run#14|AssertionError|assert message == "Targets are not defined", f"unexpected CreateCrawler message: {message!r}"
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# Glue: 10 net-new root causes — ten refusal MESSAGES are emulator-authored instead of the text real Glue returns (GetTable/GetDatabase, GetDashboardUrl, DeleteConnection, GetTableVersion, CreatePartition arity, CreateSession duplicate, CreateCatalog duplicate, CreateBlueprint duplicate, CreateTrigger validation, CreateCrawler targets)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTable

## Description
10 causes; all ten must be closed before submitting. In every one the HTTP status and the error CODE already match AWS — only the error MESSAGE diverges, and in most of them the emulator interpolates request- or state-derived values into a message AWS keeps invariant. An error message is part of the contract: clients, SDK helpers and tests branch on it.

Related open issues, named for the fixer: 20261028T122732Z-glue-4-net-new-root-causes-four-refusal.md and 20261029T103029Z-glue-2-net-new-root-causes-getdashboardurl-s.md describe the same GetDashboardUrl defect as Cause 2 below but are grounded only in the checks glue.get-dashboard-url-returns-valid-resource-url and glue.dashboard-url-returns-valid-endpoint, neither of which is the check listed here; 20261028T122732Z's Cause 1 covers the BatchCreatePartition arity message, not the CreatePartition/plain-partition path of Cause 5.

Cause 1 of 10: a missing DATABASE is reported with a request-derived sentence instead of Glue's constant 'Entity Not Found'.
AWS: Smithy: GetTable, CreateTable and GetDatabase all model EntityNotFoundException (client, HTTP 400). When the named database does not exist (never created, or deleted), real Glue answers EntityNotFoundException with the fixed, invariant message `Entity Not Found` — no database name, no table name, no trailing period, and the same text whichever of the three operations was called.
Emulator: code EntityNotFoundException is right; the message is `Database <database-name> not found.` with the request's DatabaseName interpolated and a trailing period.
Why: the only thing distinguishing this refusal on the wire is the fixed text, so message-matching clients never recognize it, and the message varies per request where AWS's does not.
Fix: the catalog database-lookup miss shared by the GetTable, CreateTable and GetDatabase handlers (services/service-com-amazonaws-glue-awsglue/src/ops/get_table.rs, create_table.rs, get_database.rs) must raise the bare fixed text.
Traces: glue.get-table-not-found, glue.reject-table-in-nonexistent-database, glue.database-delete-cascades-to-tables.

Cause 2 of 10: GetDashboardUrl on a ResourceId naming no session uses GetSession's templated sentence.
AWS: Smithy: GetDashboardUrl takes required ResourceId + ResourceType (GlueResourceType) and models EntityNotFoundException (client, HTTP 400). For ResourceType=SESSION with an id that names no session, real Glue answers EntityNotFoundException with message exactly `Entity not found` — 100% fixed text, no id, no trailing period. (GetSession's own not-found message IS templated — `Session ` + Id + ` not found.` — which is why the two must not share a code path.)
Emulator: EntityNotFoundException with `Session <ResourceId> not found.`
Why: as Cause 1; additionally a caller cannot tell GetDashboardUrl's refusal from GetSession's.
Fix: the GetDashboardUrl handler's session-lookup miss (services/service-com-amazonaws-glue-awsglue/src/ops/get_dashboard_url.rs).
Traces: glue.get-dashboard-url-unknown-resource-rejected.

Cause 3 of 10: DeleteConnection's not-found message appends the connection name.
AWS: Smithy: DeleteConnection models EntityNotFoundException (client, HTTP 400) and takes required ConnectionName. Deleting a connection that does not exist (here: deleting twice) is refused with EntityNotFoundException whose message is the fixed text `Connection not found` — no name, no colon, no period.
Emulator: EntityNotFoundException with `Connection not found: <connection-name>`, the request's ConnectionName appended.
Why: message-matching clients never match, and the text varies per request. Note the already-FIXED issue 20261025T120434Z-glue-getconnection-entitynotfoundexception-message-wording-is-wrong.md corrected GetConnection's wording; DeleteConnection was not corrected with it — this is the delete path, and the call being refused is correct behavior, only its wording is wrong.
Fix: the DeleteConnection handler's lookup miss (services/service-com-amazonaws-glue-awsglue/src/ops/delete_connection.rs).
Traces: glue.connection-create-get-delete.

Cause 4 of 10: GetTableVersion's not-found message says 'Table version' instead of 'Version'.
AWS: Smithy: GetTableVersion models EntityNotFoundException (client, HTTP 400). For a VersionId that does not exist on an existing table, real Glue answers EntityNotFoundException with the fixed text `Version not found.`
Emulator: EntityNotFoundException with `Table version not found.`
Why: different fixed wording, so nothing keyed on the documented sentence matches.
Fix: the GetTableVersion handler's version-lookup miss (services/service-com-amazonaws-glue-awsglue/src/ops/get_table_version.rs).
Traces: glue.reject-get-nonexistent-table-version.

Cause 5 of 10: the partition-arity refusal of the single-partition path is rewritten with request-derived counts.
AWS: Smithy: CreatePartition models InvalidInputException (client, HTTP 400); PartitionInput.Values must carry exactly one value per PartitionKey of the table. When the counts differ — too many values, or values supplied for a table with no partition keys at all — real Glue refuses with InvalidInputException and the fixed, invariant text `The number of partition keys do not match the number of partition values`: no table name, no counts.
Emulator: InvalidInputException with `Partition values must name one value per partition key of the table; the table has <n> partition keys and the input carried <m> values` — emulator prose interpolating both counts.
Why: a client cannot recognize the documented arity refusal, and the message varies per request. (The same wrong text is produced on the BatchCreatePartition path, already filed as Cause 1 of 20261028T122732Z; fixing one shared validator must fix both.)
Fix: the partition-arity validation of the CreatePartition handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_partition.rs), covering both the too-many-values and the zero-partition-key table cases.
Traces: glue.reject-partition-values-count-mismatch, glue.reject-partition-on-unpartitioned-table.

Cause 6 of 10: CreateSession's duplicate/idempotency refusal uses a generic 'already exists' sentence.
AWS: Smithy: CreateSession takes required Id (NameString, @length 1..255) and models AlreadyExistsException and IdempotentParameterMismatchException (both client, HTTP 400). A CreateSession naming an Id that already exists — whether the second call is identical, carries a different definition, or carries a mismatched idempotency token — is refused with AlreadyExistsException whose message is fixed text `Session already created, sessionId=` + the Id taken verbatim from the request (no trailing period).
Emulator: AlreadyExistsException with `Session <id> already exists.` — different fixed wording and a trailing period.
Why: four distinct inputs produce a sentence AWS never emits, so a caller cannot recognize the duplicate-session condition.
Fix: the CreateSession handler's duplicate-id path (services/service-com-amazonaws-glue-awsglue/src/ops/create_session.rs).
Traces: glue.create-session-duplicate-id-rejected, glue.create-session-idempotent-mismatch-rejected, glue.create-session-idempotent-token-mismatch-rejected, glue.session-duplicate-id-rejected.

Cause 7 of 10: CreateCatalog's duplicate-name message carries a trailing period AWS does not emit.
AWS: Smithy: CreateCatalog models AlreadyExistsException (client, HTTP 400). A second CreateCatalog for an existing catalog name is refused with AlreadyExistsException whose message is exactly `Catalog already exists` — fixed text, no name, NO trailing period.
Emulator: AlreadyExistsException with `Catalog already exists.`
Why: an exact-match client (the only safe way to key on a message) does not match; one character of fixed text is still fixed text.
Fix: the CreateCatalog handler's duplicate-name path (services/service-com-amazonaws-glue-awsglue/src/ops/create_catalog.rs).
Traces: glue.catalog-duplicate-create-rejected.

Cause 8 of 10: CreateBlueprint's duplicate-name message is the wrong sentence.
AWS: Smithy: CreateBlueprint models AlreadyExistsException (client, HTTP 400) and takes required Name. A repeat create of an existing blueprint name is refused with AlreadyExistsException whose message is the fixed text `Blueprint with given name already exists`.
Emulator: AlreadyExistsException with `Blueprint already exists.`
Why: as above — the documented sentence never appears.
Fix: the CreateBlueprint handler's duplicate-name path (services/service-com-amazonaws-glue-awsglue/src/ops/create_blueprint.rs).
Traces: glue.blueprint-duplicate-name-rejected.

Cause 9 of 10: CreateTrigger's two input-validation refusals are emulator prose.
AWS: Smithy: CreateTrigger takes required Name/Type (TriggerType)/Actions plus optional Schedule and Predicate, and models InvalidInputException (client, HTTP 400). A CONDITIONAL trigger created without a Predicate is refused with InvalidInputException whose message is the fixed text `Predicate cannot be null or empty`. A SCHEDULED trigger whose Schedule is not a cron expression is refused with InvalidInputException whose message is fixed text `The schedule ` + the Schedule value taken verbatim from the request + ` is invalid.`
Emulator: the codes are right but the messages are `Predicate is required when the trigger type is CONDITIONAL` and `Schedule <value> is not a cron expression of the form cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)` — emulator-authored prose in both cases.
Why: two separate validation branches of one operation emit sentences AWS never produces, so neither refusal is recognizable to a client.
Fix: the input validation of the CreateTrigger handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_trigger.rs).
Traces: glue.create-trigger-conditional-without-predicate-fails, glue.create-trigger-invalid-cron-fails.

Cause 10 of 10: CreateCrawler's empty-Targets refusal is emulator prose.
AWS: Smithy: CreateCrawler takes required Targets (CrawlerTargets) and models InvalidInputException (client, HTTP 400). A CreateCrawler whose Targets names no crawl target at all is refused with InvalidInputException whose message is the fixed text `Targets are not defined`.
Emulator: InvalidInputException with `Targets must specify at least one crawl target`.
Why: the documented sentence never appears, so the empty-targets case cannot be distinguished from any other InvalidInputException.
Fix: the Targets validation of the CreateCrawler handler (services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs).
Traces: glue.create-crawler-no-targets-fails.

## Evidence
Cause 1 — glue.get-table-not-found: expected EntityNotFoundException / 'Entity Not Found'; actual 'Database <DatabaseName> not found.'
        (replay: AssertionError: expected message 'Entity Not Found', got 'Database sim2real-behav-db-missing-01a16fb1a821 not found.' — the suffix is a per-run name)
        glue.reject-table-in-nonexistent-database: same expected/actual on CreateTable.
        glue.database-delete-cascades-to-tables: same expected/actual after DeleteDatabase.
Cause 2 — glue.get-dashboard-url-unknown-resource-rejected: expected EntityNotFoundException / 'Entity not found'; actual 'Session <ResourceId> not found.'
Cause 3 — glue.connection-create-get-delete: expected EntityNotFoundException / 'Connection not found'; actual 'Connection not found: <ConnectionName>'
Cause 4 — glue.reject-get-nonexistent-table-version: expected EntityNotFoundException / 'Version not found.'; actual 'Table version not found.'
Cause 5 — glue.reject-partition-values-count-mismatch: expected InvalidInputException / 'The number of partition keys do not match the number of partition values'; actual 'Partition values must name one value per partition key of the table; the table has 2 partition keys and the input carried 3 values'
        glue.reject-partition-on-unpartitioned-table: actual '... the table has 0 partition keys and the input carried 1 values'
Cause 6 — glue.create-session-duplicate-id-rejected: expected AlreadyExistsException / 'Session already created, sessionId=<Id>'; actual 'Session <Id> already exists.' (identical divergence in the three sibling checks)
Cause 7 — glue.catalog-duplicate-create-rejected: expected AlreadyExistsException / 'Catalog already exists'; actual 'Catalog already exists.'
Cause 8 — glue.blueprint-duplicate-name-rejected: expected AlreadyExistsException / 'Blueprint with given name already exists'; actual 'Blueprint already exists.'
Cause 9 — glue.create-trigger-conditional-without-predicate-fails: expected InvalidInputException / 'Predicate cannot be null or empty'; actual 'Predicate is required when the trigger type is CONDITIONAL'
        glue.create-trigger-invalid-cron-fails: expected InvalidInputException / 'The schedule <Schedule> is invalid.'; actual 'Schedule <Schedule> is not a cron expression of the form cron(<minutes> <hours> <day-of-month> <month> <day-of-week> <year>)'
Cause 10 — glue.create-crawler-no-targets-fails: expected InvalidInputException / 'Targets are not defined'; actual 'Targets must specify at least one crawl target'

In every cause the status (400) and the error code already match; names/ids shown above are per-run values that fill the template, not part of the requirement.

## Steps to Reproduce
Cause 1: GetTable(DatabaseName=<never created>, Name=anything), CreateTable(DatabaseName=<never created>, ...), and GetDatabase(Name=<deleted database>) -> each must be EntityNotFoundException with Message exactly 'Entity Not Found'.
Cause 2: GetDashboardUrl(ResourceId=<id naming no session>, ResourceType='SESSION') -> EntityNotFoundException / Message exactly 'Entity not found'.
Cause 3: CreateConnection(C); DeleteConnection(C); DeleteConnection(C) again -> EntityNotFoundException / Message exactly 'Connection not found'.
Cause 4: CreateDatabase+CreateTable, then GetTableVersion(DatabaseName=D, TableName=T, VersionId='999') -> EntityNotFoundException / Message exactly 'Version not found.'
Cause 5: CreateTable with PartitionKeys [{yr},{mo}], then CreatePartition with Values ['2024','01','extra'] -> InvalidInputException / 'The number of partition keys do not match the number of partition values'; repeat against a table with no PartitionKeys using Values ['x'] -> same message.
Cause 6: CreateSession(Id=S, ...); repeat it identically, with a changed definition, and with a different idempotency token -> each must be AlreadyExistsException / 'Session already created, sessionId=S'.
Cause 7: CreateCatalog(Name=C, ...) twice -> AlreadyExistsException / 'Catalog already exists' (no trailing period).
Cause 8: CreateBlueprint(Name=B, BlueprintLocation=<s3 zip>) twice -> AlreadyExistsException / 'Blueprint with given name already exists'.
Cause 9: CreateTrigger(Type='CONDITIONAL', Actions=[{JobName:J}]) with no Predicate -> InvalidInputException / 'Predicate cannot be null or empty'; CreateTrigger(Type='SCHEDULED', Schedule='every-so-often', Actions=[{JobName:J}]) -> InvalidInputException / 'The schedule every-so-often is invalid.'
Cause 10: CreateCrawler(Name=C, Role=<role>, DatabaseName=D, Targets={}) -> InvalidInputException / 'Targets are not defined'.
