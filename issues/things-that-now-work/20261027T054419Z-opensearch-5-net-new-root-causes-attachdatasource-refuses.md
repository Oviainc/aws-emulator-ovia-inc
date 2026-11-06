Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id opensearch.list-data-source-attachments-stable-creation-ordering opensearch.describe-data-source-attachment-not-found-vs-malformed-id-distinct opensearch.domain-config-ops-on-missing-domain opensearch.e2e-data-source-attachment-ingest-search-detach opensearch.update-index-dependency-failure-on-broken-mapping-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|AttachDataSource|ResourceNotFoundException
Issue-Key: opensearch.describe-data-source-attachment-not-found-vs-malformed-id-distinct|run#8|AssertionError|assert missing_message == expected_missing, (
Issue-Key: opensearch.domain-config-ops-on-missing-domain|run#20|AssertionError|assert code == 'ResourceNotFoundException', f'CancelDomainConfigChange: got {code}'
Issue-Key: opensearch.e2e-data-source-attachment-ingest-search-detach|_index_and_search#15|AssertionError|assert status in (200, 201), f"indexing a document answered HTTP {status}: {body!r}"
Issue-Key: opensearch.update-index-dependency-failure-on-broken-mapping-rejected|_error_detail#9|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 1bcc5e8064093cbefca42c0793b5b190f960ef1b

# opensearch: 5 net-new root causes — AttachDataSource refuses a nonexistent-domain ARN, attachment not-found message wording, CancelDomainConfigChange NotImplemented, domain REST data plane unrouted ('es'), UpdateIndex accepts a dangling analyzer

> _Note: this issue was identified against the code base at commit `1bcc5e8064093cbefca42c0793b5b190f960ef1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AttachDataSource, DescribeDataSourceAttachment, CancelDomainConfigChange, UpdateIndex

## Description
Five independent root causes in the OpenSearch emulator, all grounded in failing behavioral checks of service `opensearch`. They are numbered below; a fixer must close ALL FIVE before submitting.

=== Cause 1 of 5: AttachDataSource refuses a well-formed domain ARN whose domain does not exist, instead of accepting the attachment as PENDING ===
Failing checks: opensearch.attach-data-source-accepts-nonexistent-arn-without-attaching, opensearch.attach-data-source-duplicate-attachment-reuses-attachment-id, opensearch.list-data-source-attachments-stable-creation-ordering

AWS: Smithy model (com.amazonaws.opensearch#AttachDataSource) — input `dataSourceArn` is `ARN` with `@length(20..2048)` and `@pattern ".*"`; the boundary constrains only syntax, never existence. The operation's own documentation states the lifecycle: "The data source must be an Amazon OpenSearch Service domain. If both the application and the data source are active, the attachment completes immediately with a status of ATTACHED. Otherwise, the operation returns PENDING and completes the attachment automatically once both become active. If the attachment cannot be completed, its status becomes FAILED. This operation is idempotent: If the data source is already attached or pending, the operation returns the existing attachment." So a syntactically valid `arn:aws:es:<region>:<account>:domain/<name>` naming a domain that does not exist is ACCEPTED: HTTP 200 with `status` = `PENDING`, a minted `attachmentId`, the attachment visible in ListDataSourceAttachments, and it never reaches `ATTACHED` (it stays `PENDING` or turns `FAILED`). ResourceNotFoundException IS modeled for this operation (HTTP 409) but recorded behavior applies it to the application `id`, not to the data source ARN.

Emulator: HTTP 409 `ResourceNotFoundException` with message `Data source not found: <dataSourceArn>` — fixed text `"Data source not found: "` followed by the request's `dataSourceArn` value. No attachment is created, so every flow that builds attachments without provisioning a domain aborts on the very first AttachDataSource call.

Why: a client that attaches a data source whose domain is still being created, was deleted, or lives in another account gets a hard 409 instead of a PENDING attachment it can poll. The documented idempotency and the PENDING→ATTACHED/FAILED settling path are unreachable, and ListDataSourceAttachments can never be populated by this route, so attachment ordering/pagination behavior is likewise unobservable.

Fix: the AttachDataSource handler's data-source existence gate — the `store.domain_of_arn(&input.data_source_arn).ok_or_else(|| ... ResourceNotFoundException(data_source_not_found(...)))` lookup must not refuse. A well-formed ARN naming no live domain yields an attachment record owed to the clock (`PENDING`, settling to `FAILED` when it cannot complete), minted and stored exactly like the live-domain case so the idempotent repeat returns the same `attachmentId`.

Traces: see Traces section.

=== Cause 2 of 5: DescribeDataSourceAttachment's ResourceNotFoundException message is reworded ===
Failing check: opensearch.describe-data-source-attachment-not-found-vs-malformed-id-distinct

AWS: Smithy model (com.amazonaws.opensearch#DescribeDataSourceAttachment) documents "Throws a ResourceNotFoundException if no attachment record exists for the specified application and data source combination"; ResourceNotFoundException is modeled, and ValidationException (HTTP 400) is the distinct code for input that breaks `id`'s pattern. Recorded behavior for the not-found class: code `ResourceNotFoundException`, message `Data source attachment not found for application <id> and data source <dataSourceArn>`. Fixed text: `"Data source attachment not found for application "` and `" and data source "`; variable parts: the request's `id` and its `dataSourceArn`, taken verbatim from the request.

Emulator: the status and the error code are correct, but the message reads `Data source <dataSourceArn> is not attached to application <id>.` — different fixed text, reversed operand order, and a trailing period AWS does not emit.

Why: callers and SDK/CLI users that match or surface the refusal text (and the same text is asserted after DetachDataSource, Cause 4's flow) see wording real AWS never produces, so message-based handling and golden-output comparisons diverge.

Fix: the shared not-found message builder for attachments (`attachment_not_found`) used by the DescribeDataSourceAttachment handler. Note the same template must be produced on the post-detach read path.

Traces: see Traces section. (The check's second half — ValidationException for a malformed `id` — was never reached because the first assertion failed, so that leg is unverified here.)

=== Cause 3 of 5: CancelDomainConfigChange on an unknown domain answers NotImplemented instead of ResourceNotFoundException ===
Failing check: opensearch.domain-config-ops-on-missing-domain

AWS: Smithy model (com.amazonaws.opensearch#CancelDomainConfigChange) models exactly BaseException (400), DisabledOperationException (409), InternalException (500), ResourceNotFoundException (409) and ValidationException (400). `NotImplemented` is NOT a modeled error of this service. Recorded behavior: a syntactically valid `DomainName` (matching `^[a-z][a-z0-9\-]+$`, length 3..28) naming no domain is refused `ResourceNotFoundException` with message `Domain not found: <DomainName>` — fixed text `"Domain not found: "` plus the request's `DomainName`. DescribeDomain, DeleteDomain, DescribeDomainConfig and UpdateDomainConfig all pass this same contract in the emulator; CancelDomainConfigChange is the one that does not.

Emulator: the call returns the scaffold channel's unmodeled `NotImplemented` error, so the whole domain-scoped not-found sweep fails at that step.

Why: an unmodeled error code is not in any SDK's error factory for this service, so clients cannot catch it; a caller probing whether a config change can be cancelled on a missing/just-deleted domain gets a shape AWS never returns instead of the 409 ResourceNotFoundException it handles.

Fix: the CancelDomainConfigChange handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/cancel_domain_config_change.rs) — resolve `DomainName` against the domain store and return the modeled ResourceNotFoundException for an unknown name. Open issue 20261025T020249Z-opensearch-control-plane-startdomainmaintenance-and-canceldomainconfigchange-answer-notimplemented.md describes the same operation answering NotImplemented but the suite did not mark this check as owned by it; if that issue is being fixed, this check belongs in its verification set.

Traces: see Traces section.

=== Cause 4 of 5: a domain's published Endpoint routes to no service — the OpenSearch REST data plane is unrouted ('es') ===
Failing check: opensearch.e2e-data-source-attachment-ingest-search-detach

AWS: a domain reported ACTIVE by DescribeDomain publishes `DomainStatus.Endpoint`, and that endpoint serves the OpenSearch REST API under SigV4 signing name `es`. Recorded behavior: a SigV4('es')-signed `PUT https://<Endpoint>/<index>/_doc/1?refresh=true` with a JSON body answers HTTP 200 or 201, and the subsequent `POST /<index>/_search` answers HTTP 200 with the document in `hits.total`.

Emulator: the signed request to the published Endpoint answers HTTP 404 with body `{"__type": "UnknownService", "message": "no emulated service is registered for 'es'"}` — the endpoint the control plane advertises is not routed to any emulated service, so no document can be indexed or searched.

Why: every end-to-end flow that writes and reads a document through an attached domain is impossible; a client follows the Endpoint the emulator itself handed back and gets an UnknownService 404 that real AWS never returns for a live domain, and the "detaching the application view leaves the domain's data intact" contract cannot be observed at all.

Fix: the service-registration/routing layer for the domain endpoint (the `es` sigv4 name / domain-endpoint host) plus the domain REST data-plane implementation it must dispatch to. Open issues 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md and 20261026T044500Z-opensearch-package-association-e2e-flows-need-the-domain-data-plane-and.md describe the same missing data plane from other checks; the suite did not mark this check as owned by either, so it is filed here and should be added to their verification sets.

Traces: see Traces section.

=== Cause 5 of 5: UpdateIndex accepts a mapping that references an analyzer never configured on the domain ===
Failing check: opensearch.update-index-dependency-failure-on-broken-mapping-rejected

AWS: Smithy model (com.amazonaws.opensearch#UpdateIndex) — `IndexSchema` is a required `document`, and the operation models ValidationException (HTTP 400), DependencyFailureException (424), ResourceNotFoundException (409) and others. Recorded behavior: an UpdateIndex whose mapping gives a field an `analyzer` name that was never defined on the domain (no package ever backed it) is refused `ValidationException` with message `analyzer [<analyzer-name>] has not been configured in mappings` — fixed text `"analyzer ["`, `"] has not been configured in mappings"`; the variable part is the analyzer name taken from the request's IndexSchema. The rejected mapping must leave no trace: neither the analyzer name nor the new field appears in a following GetIndex, and a subsequent valid UpdateIndex still returns `Status` = `UPDATED`. (This is distinct from DependencyFailureException, which AWS uses for a package-backed analyzer whose backing file becomes unavailable AFTER association.)

Emulator: the call SUCCEEDS — HTTP 200 with body `{"Status": "UPDATED"}` — so the dangling analyzer reference is written into the index schema and no refusal is produced.

Why: an over-permissive accept hides a real misconfiguration: a client developing mappings against the emulator believes a mapping referencing an undefined analyzer is valid, then gets a 400 ValidationException against real AWS. Worse, the emulator's index state now contains a schema AWS would never have stored, so later reads of that index diverge too.

Fix: validation in the UpdateIndex handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_index.rs) — walk the submitted mapping's `properties.*.analyzer` references against the analyzers configured on the domain and refuse the modeled ValidationException with the message template above before any part of the schema is persisted.

Traces: see Traces section.

## Evidence
Cause 1 — expected: AttachDataSource(id=<app>, dataSourceArn=arn:aws:es:<region>:<account>:domain/<nonexistent>) -> HTTP 200, status 'PENDING', an attachmentId; actual: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the AttachDataSource operation: Data source not found: arn:aws:es:us-west-2:000000000000:domain/sim2real-nope-043c60d7f06e  (identical refusal in the duplicate-attachment check for .../sim2real-attach-dup-f4029c57 and in the listing-ordering check for .../sim2real-src0-b4b9e817b2a6). Handler: src/ops/attach_data_source.rs, the domain_of_arn(...)/data_source_not_found(...) gate (~lines 262-273).

Cause 2 — AssertionError: expected message 'Data source attachment not found for application sfi9sv1baaaa and data source arn:aws:es:us-west-2:000000000000:domain/sim2real-ds-340504844c8f', got 'Data source arn:aws:es:us-west-2:000000000000:domain/sim2real-ds-340504844c8f is not attached to application sfi9sv1baaaa.'  (code ResourceNotFoundException was correct). Message built by attachment_not_found(...) in src/state.rs, called from src/ops/describe_data_source_attachment.rs:192. The ids/ARNs shown are this run's values; the requirement is the fixed text with the request's id and dataSourceArn substituted.

Cause 3 — AssertionError: CancelDomainConfigChange: got NotImplemented  (expected code 'ResourceNotFoundException' with message 'Domain not found: <DomainName>'). The same check's DescribeDomain, DeleteDomain, DescribeDomainConfig and UpdateDomainConfig legs passed on that exact contract immediately before.

Cause 4 — AssertionError: indexing a document answered HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}  (expected HTTP 200 or 201 for a SigV4('es')-signed PUT /<index>/_doc/1?refresh=true against DomainStatus.Endpoint of an ACTIVE domain).

Cause 5 — AssertionError: expected a rejection, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'Status': 'UPDATED'}  (expected HTTP 400 ValidationException, message 'analyzer [sim2real_missing_analyzer] has not been configured in mappings', and the rejected field absent from a following GetIndex).

## Steps to Reproduce
Cause 1: CreateApplication and wait for status ACTIVE. Call AttachDataSource(id=<appId>, dataSourceArn="arn:aws:es:<region>:<account>:domain/<a-name-that-does-not-exist>"). Expect HTTP 200 with status PENDING and an attachmentId; observe HTTP 409 ResourceNotFoundException "Data source not found: <arn>". Repeat the identical call to confirm the documented idempotency (same attachmentId) and call ListDataSourceAttachments(id=<appId>) to confirm exactly one record.

Cause 2: CreateApplication, wait ACTIVE, then DescribeDataSourceAttachment(id=<appId>, dataSourceArn="arn:aws:es:<region>:<account>:domain/<never-attached>"). Expect code ResourceNotFoundException with message "Data source attachment not found for application <appId> and data source <arn>"; observe "Data source <arn> is not attached to application <appId>."

Cause 3: Pick a DomainName matching ^[a-z][a-z0-9-]+$ (3..28 chars) that was never created. Call CancelDomainConfigChange(DomainName=<missing>, DryRun=true). Expect ResourceNotFoundException "Domain not found: <missing>"; observe NotImplemented. (DescribeDomain/DeleteDomain/DescribeDomainConfig/UpdateDomainConfig on the same name already answer correctly.)

Cause 4: CreateDomain (t3.small.search x1, gp3 10 GiB), poll DescribeDomain until Created && !Processing && Endpoint is published. SigV4-sign with signing name "es" and send PUT https://<Endpoint>/<index>/_doc/1?refresh=true with body {"val":"x"}. Expect HTTP 200/201; observe HTTP 404 {"__type":"UnknownService","message":"no emulated service is registered for 'es'"}.

Cause 5: CreateDomain and wait active. CreateIndex(DomainName=<d>, IndexName=<i>, IndexSchema={"mappings":{"properties":{"title":{"type":"text"}}}}). Then UpdateIndex(DomainName=<d>, IndexName=<i>, IndexSchema={"mappings":{"properties":{"body":{"type":"text","analyzer":"<name-never-configured>"}}}}). Expect HTTP 400 ValidationException "analyzer [<name-never-configured>] has not been configured in mappings" and GetIndex unchanged; observe HTTP 200 {"Status":"UPDATED"}.
