Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.create-domain-invalid-name-pattern-rejected opensearch.overly-permissive-domain-name-rules opensearch.application-name-invalid-pattern-rejected opensearch.list-migrations-disabled-operation-unsupported-config opensearch.index-invalid-name-rejected opensearch.application-ops-on-missing-application opensearch.attach-data-source-nonexistent-resource-rejected opensearch.list-data-source-attachments-disabled-when-feature-off opensearch.application-duplicate-name-conflict-rejected opensearch.describe-data-source-attachment-disabled-when-feature-off opensearch.e2e-data-source-attachment-consumption-flow opensearch.e2e-data-source-attachment-query-detach-flow opensearch.detach-data-source-not-attached-rejected opensearch.get-migration-unknown-id-error-code opensearch.get-migration-unknown-id-error-shape opensearch.get-migration-cross-account-access-denied-not-not-found opensearch.get-migration-malformed-id-error-semantics opensearch.create-package-duplicate-name-rejected opensearch.service-software-update-lifecycle opensearch.domain-observability-ops-on-missing-domain opensearch.scheduled-action-and-maintenance-ops-on-missing-domain
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.create-domain-invalid-name-pattern-rejected|run#6|AssertionError|assert refusal == ("ValidationException", (
Issue-Key: opensearch.overly-permissive-domain-name-rules|run#3|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: opensearch.application-name-invalid-pattern-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: opensearch.list-migrations-disabled-operation-unsupported-config|run#8|AssertionError|assert got == expected, (
Issue-Key: opensearch.index-invalid-name-rejected|run#7|AssertionError|assert message == _INVALID_UPPER_MESSAGE, f"unexpected CreateIndex refusal message: {message!r}"
Issue-Key: opensearch|DeleteApplication|ResourceNotFoundException
Issue-Key: opensearch.attach-data-source-nonexistent-resource-rejected|run#9|AssertionError|assert message1 == expected, f"missing-application message: expected {expected!r}, got {message1!r}"
Issue-Key: opensearch.list-data-source-attachments-disabled-when-feature-off|run#12|AssertionError|assert got == expected, f"expected {expected!r}, got {got!r}"
Issue-Key: opensearch.application-duplicate-name-conflict-rejected|run#10|AssertionError|assert message == expected, f"duplicate-name message: expected {expected!r}, got {message!r}"
Issue-Key: opensearch.describe-data-source-attachment-disabled-when-feature-off|run#20|AssertionError|assert unattached_message == expected_unattached, (
Issue-Key: opensearch.e2e-data-source-attachment-consumption-flow|run#20|AssertionError|assert detached_message == expected, f"expected message {expected!r}, got {detached_message!r}"
Issue-Key: opensearch.e2e-data-source-attachment-query-detach-flow|run#24|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: opensearch.detach-data-source-not-attached-rejected|run#18|AssertionError|assert pending_message == expected_pending, (
Issue-Key: opensearch.get-migration-unknown-id-error-code|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: opensearch.get-migration-unknown-id-error-shape|run#11|AssertionError|assert message_1 == expected_1, f"expected message {expected_1!r}, got {message_1!r}"
Issue-Key: opensearch.get-migration-cross-account-access-denied-not-not-found|run#6|AssertionError|assert baseline_message == expected, f"expected message {expected!r}, got {baseline_message!r}"
Issue-Key: opensearch.get-migration-malformed-id-error-semantics|run#5|AssertionError|assert malformed_code == "ValidationException", (
Issue-Key: opensearch.create-package-duplicate-name-rejected|run#15|AssertionError|assert refusal == ("BaseException", "Package is already in desired state"), (
Issue-Key: opensearch|StartServiceSoftwareUpdate|ValidationException
Issue-Key: opensearch.domain-observability-ops-on-missing-domain|run#8|AssertionError|assert code == 'BaseException', f'DescribeDomainNodes: expected BaseException, got {code}'
Issue-Key: opensearch.scheduled-action-and-maintenance-ops-on-missing-domain|run#7|AssertionError|assert code == 'BaseException', f'ListDomainMaintenances: expected BaseException, got {code}'
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# opensearch: control-plane refusals carry the wrong error class and non-AWS message text across ten operations (10 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
This issue bundles 10 root causes, all in the opensearch service. All ten are refusal-fidelity defects: the request is correctly refused (or correctly accepted) but the error code and/or the message text is not AWS's. The open issue 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md and 20261025T124157Z-opensearch-six-net-new-control-plane-causes-describedomainhealth.md own a different set of checks in the same operations; the checks below were not marked owned by them. Each cause must be closed before submitting.

Cause 1 of 10: constraint-violation messages echo the model's ANCHORED regex instead of the pattern text AWS quotes.
AWS: Smithy: CreateDomain.DomainName @pattern "^[a-z][a-z0-9\\-]+$", CreateApplication.name @pattern "^[a-z][a-z0-9\\-]+$", ListMigrations/StartMigration applicationId @pattern "^[a-z0-9]{3,30}$". For a violation AWS answers 400 ValidationException whose message is the fixed template "N validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" — and the <pattern> it quotes is the UNANCHORED documented pattern: `[a-z][a-z0-9\-]+` for domainName and for the application name, and `[a-z0-9-]{3,30}` for applicationId. The variable parts are the rejected value, the member name and the count; the rest is fixed text.
Emulator: the same template is emitted but the pattern is rendered as the model's anchored string — `^[a-z][a-z0-9\-]+$` for domainName/name and `^[a-z0-9]{3,30}$` for applicationId.
Why: clients and operators match on or display this message; the quoted pattern differs from the one AWS documents and returns, so message assertions and user-facing diagnostics are wrong.
Fix: the shared constraint-violation message renderer used by the opensearch operations' input validation must emit the documented (unanchored) pattern text, with the applicationId pattern rendered as AWS renders it.
Traces: opensearch.create-domain-invalid-name-pattern-rejected, opensearch.overly-permissive-domain-name-rules, opensearch.application-name-invalid-pattern-rejected, opensearch.list-migrations-disabled-operation-unsupported-config.

Cause 2 of 10: CreateIndex/GetIndex refuse a bad IndexName with hand-written prose instead of the modelled constraint message.
AWS: Smithy: CreateIndex.IndexName @pattern "^(?!\\.\\.?$)[^_ ,:\"+/*\\\\|?#><A-Z-][^ ,:\"+/*\\\\|?#><A-Z]*$", @length 1..255; CreateIndex and GetIndex both model ValidationException (400). AWS refuses with the fixed template "1 validation error detected: Value '<value>' at 'indexName' failed to satisfy constraint: Member must satisfy regular expression pattern: <that pattern>", the value being the caller's index name.
Emulator: ValidationException is raised with an invented prose message — `IndexName must not be '.' or '..', must not begin with '_' or '-', and must carry no upper-case letter, whitespace or any of ,:"+/*\|?#>< : INVALID_UPPER`.
Why: the refusal does not identify the failing member or the pattern in AWS's machine-readable form, so clients that parse validation messages cannot attribute the failure.
Fix: CreateIndex/GetIndex input validation must go through the same modelled constraint-message renderer as the other members.
Traces: opensearch.index-invalid-name-rejected.

Cause 3 of 10: application-not-found refusals use a generic sentence instead of naming the application and account.
AWS: Smithy: DeleteApplication/AttachDataSource/ListDataSourceAttachments model ResourceNotFoundException (409). For an application id that does not exist AWS's message names the resource: "Application <applicationId> not found for account <accountId>" on the AttachDataSource path and "Application <applicationId> not found" on the listing/delete path — fixed text with the requested id (and the caller's account id) substituted.
Emulator: the message is the constant `Application does not exist` with no id and no account.
Why: a caller cannot tell which id of a batch failed, and an operator cannot see whether the lookup was scoped to the right account.
Fix: the application-lookup error construction shared by the application handlers must interpolate the requested id (and account where AWS includes it).
Traces: opensearch.application-ops-on-missing-application, opensearch.attach-data-source-nonexistent-resource-rejected, opensearch.list-data-source-attachments-disabled-when-feature-off.

Cause 4 of 10: the duplicate-application-name conflict message is reworded.
AWS: Smithy: CreateApplication models ConflictException (409). AWS's message is the fixed text "Application name <name> already exists" with the requested name substituted — no trailing period and the words in that order.
Emulator: `Application with name <name> already exists.` — extra "with" and a trailing period.
Why: message-matching clients and idempotency wrappers keyed on the exact text mis-handle the conflict.
Fix: CreateApplication's duplicate-name refusal text in its handler.
Traces: opensearch.application-duplicate-name-conflict-rejected.

Cause 5 of 10: the data-source-attachment not-found message is reworded.
AWS: Smithy: DescribeDataSourceAttachment models ResourceNotFoundException (409). AWS's message is the fixed text "Data source attachment not found for application <id> and data source <dataSourceArn>", with the application id and the requested ARN substituted.
Emulator: `Data source <dataSourceArn> is not attached to application <id>.` — different sentence, reversed order, trailing period.
Why: clients that distinguish "never attached" from other not-found causes by message text break.
Fix: DescribeDataSourceAttachment's not-found refusal text in its handler.
Traces: opensearch.describe-data-source-attachment-disabled-when-feature-off, opensearch.e2e-data-source-attachment-consumption-flow, opensearch.e2e-data-source-attachment-query-detach-flow.

Cause 6 of 10: the detach-while-PENDING refusal is reworded.
AWS: Smithy: DetachDataSource models ConflictException (409); DataSourceAttachmentStatus enum is PENDING|ATTACHED|FAILED. AWS's message is the fixed text "Data source <dataSourceArn> attachment to application <id> is still PENDING." with the ARN and id substituted.
Emulator: `Data source <arn> has a PENDING attachment to application <id>. Wait for the attachment to complete before detaching the data source.` — a different sentence plus an added instruction.
Why: the refusal is not the one AWS words, so text-matching retry logic does not recognise the transient conflict.
Fix: DetachDataSource's in-flight-attachment refusal text in its handler.
Traces: opensearch.detach-data-source-not-attached-rejected.

Cause 7 of 10: GetMigration's not-found and malformed-id semantics are wrong in both class and text.
AWS: Smithy: GetMigration(migrationId) models ValidationException (400), ResourceNotFoundException (409) and AccessDeniedException (403). Recorded behavior: for a syntactically invalid migrationId AWS refuses with ValidationException, not ResourceNotFoundException; for a well-formed but unknown id — including one owned by another account — the refusal is phrased against the APPLICATION, with the fixed text "Application <applicationId-prefix> not found" (the variable part is the id segment taken from the request), never a message naming the migration id.
Emulator: every unknown id answers ResourceNotFoundException with `Migration not found: <migrationId>`, and a malformed id also answers ResourceNotFoundException instead of ValidationException.
Why: a client cannot tell a malformed request (never retryable, fix the input) from a missing resource, and cross-account probing is answered with a message that confirms the id shape AWS deliberately does not confirm.
Fix: GetMigration's handler — id syntax validation ahead of the lookup, and the not-found error construction.
Traces: opensearch.get-migration-unknown-id-error-code, opensearch.get-migration-unknown-id-error-shape, opensearch.get-migration-cross-account-access-denied-not-not-found, opensearch.get-migration-malformed-id-error-semantics.

Cause 8 of 10: the duplicate-package refusal message is not AWS's.
AWS: Smithy: CreatePackage models ResourceAlreadyExistsException (409) and the PackageName @pattern "^[a-z][a-z0-9\\-]+$". AWS's duplicate-name message names the package in AWS's own wording; recorded behavior shows the emulator's text is not it.
Emulator: `Package already exists: <packageName>` (code ResourceAlreadyExistsException).
Why: callers that key idempotent create-or-reuse logic on the message text do not match.
Fix: CreatePackage's duplicate-name refusal text in its handler.
Traces: opensearch.create-package-duplicate-name-rejected.

Cause 9 of 10: StartServiceSoftwareUpdate's refusal message for a domain with nothing to update is not AWS's.
AWS: Smithy: StartServiceSoftwareUpdate models BaseException (400, defaulted), ValidationException (400) and ResourceNotFoundException (409). Recorded behavior: a domain whose ServiceSoftwareOptions report UpdateAvailable=false is refused with the fixed message "The domain is not available for update." — no domain name is interpolated.
Emulator: `There is no software update available for domain <domainName>.`
Why: the message differs from AWS's wording and leaks a different rule than the one AWS states, so clients that branch on the refusal text treat an unavailable domain as an absent update.
Fix: StartServiceSoftwareUpdate's refusal text in its handler.
Traces: opensearch.service-software-update-lifecycle.

Cause 10 of 10: the observability and maintenance-listing reads on an unknown domain use ResourceNotFoundException where AWS uses the generic BaseException envelope.
AWS: Smithy: DescribeDomainHealth, DescribeDomainNodes, DescribeDomainChangeProgress and ListDomainMaintenances all bind BaseException (httpError 400, defaulted) ALONGSIDE ResourceNotFoundException (409). Recorded behavior: for a syntactically valid domain name that does not exist these four answer the BaseException envelope — DescribeDomainHealth/DescribeDomainNodes with the fixed text "Domain not found: <domainName>", DescribeDomainChangeProgress with the fixed text "No progress information found", ListDomainMaintenances with "Domain not found: <domainName>" — while the sibling ListScheduledActions genuinely answers ResourceNotFoundException with "Domain not found: <domainName>".
Emulator: all of them answer ResourceNotFoundException, so the error class is wrong for four operations (and the asymmetry with ListScheduledActions is lost).
Why: clients retry/branch on the error code; a modelled ResourceNotFoundException is handled differently from the unmodelled BaseException envelope, and SDK error-type matching (botocore exception classes) resolves to the wrong class.
Fix: the not-found refusal construction in the DescribeDomainHealth, DescribeDomainNodes, DescribeDomainChangeProgress and ListDomainMaintenances handlers.
Traces: opensearch.domain-observability-ops-on-missing-domain, opensearch.scheduled-action-and-maintenance-ops-on-missing-domain.

## Evidence
Cause 1 — expected message `1 validation error detected: Value 'Invalid_Domain_123' at 'domainName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9\-]+`; actual `... pattern: ^[a-z][a-z0-9\-]+$`. Same for 'Sim2Real-Upper' at 'domainName', for 'Bad_SIM2REAL_BADNAME_0A2' at 'name', and for applicationId where expected `[a-z0-9-]{3,30}` vs actual `^[a-z0-9]{3,30}$`.
Cause 2 — expected `1 validation error detected: Value 'INVALID_UPPER' at 'indexName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!\.\.?$)[^_ ,:"+/*\\|?#><A-Z-][^ ,:"+/*\\|?#><A-Z]*$`; actual `IndexName must not be '.' or '..', must not begin with '_' or '-', and must carry no upper-case letter, whitespace or any of ,:"+/*\|?#>< : INVALID_UPPER`.
Cause 3 — expected `Application sim2real00missingapp not found for account 000000000000` / `Application <id> not found`; actual `Application does not exist` (code ResourceNotFoundException, correct).
Cause 4 — expected `Application <name> already exists`; actual `Application with name <name> already exists.`
Cause 5 — expected `Data source attachment not found for application <id> and data source <arn>`; actual `Data source <arn> is not attached to application <id>.`
Cause 6 — expected `Data source <arn> attachment to application <id> is still PENDING.`; actual `Data source <arn> has a PENDING attachment to application <id>. Wait for the attachment to complete before detaching the data source.`
Cause 7 — expected code ValidationException for a malformed migrationId, got ResourceNotFoundException; expected message `Application <id> not found`, got `Migration not found: <migrationId>`.
Cause 8 — expected AWS's exact duplicate-name message, got ('ResourceAlreadyExistsException', 'Package already exists: <name>').
Cause 9 — expected `The domain is not available for update.`, got `There is no software update available for domain <domainName>.`
Cause 10 — `DescribeDomainNodes: expected BaseException, got ResourceNotFoundException`; `ListDomainMaintenances: expected BaseException, got ResourceNotFoundException`.

## Steps to Reproduce
1. Cause 1: CreateDomain(DomainName='Invalid_Domain_123'); CreateApplication(name='Bad_SIM2REAL_BADNAME_0A2'); ListMigrations(applicationId='THIS-IS-NOT-VALID!!') — read each ValidationException message and compare the quoted pattern.
2. Cause 2: create a domain, wait Active, CreateIndex(DomainName=<d>, IndexName='INVALID_UPPER', IndexSchema={"mappings":{"properties":{}},"settings":{}}) and then GetIndex with the same name.
3. Cause 3: DeleteApplication(id=<never-created id>); AttachDataSource(id=<never-created id>, dataSourceArn=<domain arn>); ListDataSourceAttachments(id=<never-created id>).
4. Cause 4: CreateApplication(name=<n>) twice.
5. Cause 5/6: create an application, AttachDataSource a domain ARN (status PENDING), then DescribeDataSourceAttachment for an unattached ARN and DetachDataSource for the PENDING one.
6. Cause 7: GetMigration(migrationId='not a uuid') and GetMigration(migrationId=<well-formed unknown id>).
7. Cause 8: CreatePackage with the same PackageName twice.
8. Cause 9: create a domain whose ServiceSoftwareOptions.UpdateAvailable is false, then StartServiceSoftwareUpdate(DomainName=<d>).
9. Cause 10: with a syntactically valid but nonexistent DomainName call DescribeDomainHealth, DescribeDomainNodes, DescribeDomainChangeProgress, ListDomainMaintenances and ListScheduledActions and compare error codes.
