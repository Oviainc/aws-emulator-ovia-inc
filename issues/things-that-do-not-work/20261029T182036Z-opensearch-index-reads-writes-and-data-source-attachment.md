Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.update-index-malformed-mapping-rejected opensearch.update-index-malformed-settings-rejected opensearch.overly-permissive-index-mapping-type-change-rejected opensearch.overly-permissive-update-index-invalid-mapping-rejected opensearch.overly-permissive-update-index-disabled-feature-blocked opensearch.dependency-failure-on-update-index opensearch.list-migrations-most-recent-first-ordering opensearch.migration-listing-recency-ordering opensearch.migration-progress-visibility-poll-window opensearch.representation-migration-timestamp-precision-preserved opensearch.start-migration-invalid-workspace-type-rejected opensearch.attach-data-source-inactive-domain-state opensearch.attach-detach-data-source-concurrent-race-conflict opensearch.data-source-attachment-lifecycle opensearch.attach_data_source_duplicate_rejected opensearch.concurrent-detach-data-source-race-idempotent
Issue-Key: opensearch.update-index-malformed-mapping-rejected|_error_detail#10|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: opensearch.update-index-malformed-settings-rejected|_error_code_and_message#11|AssertionError|raise AssertionError("expected a ClientError, call succeeded")
Issue-Key: opensearch.overly-permissive-index-mapping-type-change-rejected|_mapped_type#9|KeyError|return doc[index_name]["mappings"]["properties"].get(field, {}).get("type")
Issue-Key: opensearch.overly-permissive-update-index-invalid-mapping-rejected|run#16|KeyError|properties = schema[index_name]["mappings"]["properties"]
Issue-Key: opensearch.overly-permissive-update-index-disabled-feature-blocked|run#9|AssertionError|assert update_code == "ValidationException", (
Issue-Key: opensearch.dependency-failure-on-update-index|run#9|AssertionError|assert (refusal_code, refusal_message) == ("ValidationException", (
Issue-Key: opensearch.list-migrations-most-recent-first-ordering|_attach_data_source#4|AssertionError|assert last == "ATTACHED", f"data source attachment landed in {last!r} instead of ATTACHED"
Issue-Key: opensearch.migration-listing-recency-ordering|_attach_data_source#7|AssertionError|assert last == "ATTACHED", f"data source attachment landed in {last!r} instead of ATTACHED"
Issue-Key: opensearch.migration-progress-visibility-poll-window|_attach_data_source#4|AssertionError|assert last == "ATTACHED", f"data source attachment landed in {last!r} instead of ATTACHED"
Issue-Key: opensearch.representation-migration-timestamp-precision-preserved|_attach_data_source#4|AssertionError|assert last == "ATTACHED", f"data source attachment landed in {last!r} instead of ATTACHED"
Issue-Key: opensearch.start-migration-invalid-workspace-type-rejected|_poll_attachment_status#9|AssertionError|raise AssertionError(f"attachment {app_id}/{data_source_arn} entered {last['status']}")
Issue-Key: opensearch.attach-data-source-inactive-domain-state|_assert_domain_still_creating#11|AssertionError|assert status.get("Processing") is True and status.get("Endpoint") is None, (
Issue-Key: opensearch.attach-detach-data-source-concurrent-race-conflict|_assert_domain_still_creating#9|AssertionError|assert status.get("Processing") is True and status.get("Endpoint") is None, (
Issue-Key: opensearch.data-source-attachment-lifecycle|_assert_domain_still_creating#4|AssertionError|assert status.get("Processing") is True and status.get("Endpoint") is None, (
Issue-Key: opensearch.attach_data_source_duplicate_rejected|run#7|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: opensearch.concurrent-detach-data-source-race-idempotent|run#13|AssertionError|assert code == "ConflictException", f"unexpected failure code {code!r} (expected ConflictException)"
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# opensearch: index reads/writes and data-source attachment lifecycle diverge — GetIndex is not keyed by index name, UpdateIndex validates nothing, attachments land FAILED and a new domain never holds a Creating window (7 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIndex

## Description
This issue bundles 7 root causes, all in the opensearch service. Each cause must be closed before submitting.

Cause 1 of 7: UpdateIndex accepts a malformed mapping and malformed settings.
AWS: Smithy: UpdateIndex(DomainName, IndexName, IndexSchema required, type document) models ValidationException (400) and DependencyFailureException (424). An IndexSchema whose mappings or settings are not a legal index schema is refused 400 ValidationException naming the offending part of the document; the only successful output is Status (IndexStatus enum).
Emulator: the call returns HTTP 200 {"Status": "UPDATED"} for both a malformed mapping and malformed settings — the IndexSchema document is stored without being parsed or validated.
Why: a client believes a schema change landed; the stored schema is a document the engine cannot honour, so the divergence only shows up as broken index behaviour later (and here is invisible because the data plane is not served at all).
Fix: UpdateIndex's input validation — parse and validate the IndexSchema document before applying it.
Traces: opensearch.update-index-malformed-mapping-rejected, opensearch.update-index-malformed-settings-rejected.

Cause 2 of 7: GetIndex returns IndexSchema as a bare {"mappings": ...} object instead of a document keyed by index name.
AWS: Smithy: GetIndex's IndexSchema is a `document`. Recorded behavior: it mirrors the native OpenSearch mapping API response and is keyed by the index name — {"<indexName>": {"mappings": {...}, "settings": {...}}} — which is NOT the bare {"mappings": ...} shape UpdateIndex accepts as input.
Emulator: the returned IndexSchema has no "<indexName>" key, so a client reading IndexSchema[<indexName>]["mappings"]["properties"] raises KeyError: '<indexName>'.
Why: a client cannot read back a mapping at all through the documented path; code written against AWS's response shape crashes rather than mis-reads.
Fix: GetIndex's response serializer must wrap the stored schema under the index name.
Traces: opensearch.overly-permissive-index-mapping-type-change-rejected, opensearch.overly-permissive-update-index-invalid-mapping-rejected.

Cause 3 of 7: UpdateIndex uses ResourceNotFoundException where AWS uses ValidationException for the index-management gate.
AWS: Smithy: UpdateIndex models ValidationException (400), DisabledOperationException (409), DependencyFailureException (424) AND ResourceNotFoundException (409). Recorded behavior: when the domain's engine/software version does not support the index-management APIs, the refusal is ValidationException naming the software-version gate — not a not-found on the index, which AWS has not even looked up yet.
Emulator: both cases answer ResourceNotFoundException with `Resource index <indexName> does not exist`.
Why: the caller is told to create an index when the real problem is that the operation is unavailable for that domain's engine version — a misdiagnosis that sends automation down a create-retry loop.
Fix: UpdateIndex's handler must run the engine/feature-support gate (and emit ValidationException) ahead of the index lookup.
Traces: opensearch.overly-permissive-update-index-disabled-feature-blocked, opensearch.dependency-failure-on-update-index.

Cause 4 of 7: a data-source attachment never reaches ATTACHED — it transitions PENDING -> FAILED.
AWS: Smithy: DataSourceAttachmentStatus enum is PENDING|ATTACHED|FAILED. AttachDataSource returns status PENDING and the attachment converges to ATTACHED for a valid, same-account data source; DescribeDataSourceAttachment/ListDataSourceAttachments then report ATTACHED.
Emulator: the attachment settles in FAILED for valid inputs, so every flow that waits for ATTACHED fails.
Why: no caller can ever consume an attached data source; migration listing/progress and representation flows that depend on an attached source are permanently blocked, and the FAILED status carries no cause the client can act on.
Fix: the attachment state machine in the AttachDataSource handler / data-source attachment state must converge a valid attachment to ATTACHED.
Traces: opensearch.list-migrations-most-recent-first-ordering, opensearch.migration-listing-recency-ordering, opensearch.migration-progress-visibility-poll-window, opensearch.representation-migration-timestamp-precision-preserved, opensearch.start-migration-invalid-workspace-type-rejected.

Cause 5 of 7: a freshly created domain never holds a Creating window — Processing/Endpoint are reported as settled immediately.
AWS: Smithy: CreateDomain returns DomainStatus with Created, Processing, Endpoint and DomainProcessingStatus. Recorded behavior: for a window after CreateDomain the domain reports DomainProcessingStatus='Creating' with Processing=true and NO Endpoint; the endpoint is published only once provisioning completes.
Emulator: within the window the check holds, the domain has already left that state — it reports Processing=false and/or a published Endpoint while still nominally Creating, so inactive-domain behaviour cannot be exercised at all.
Why: any client that polls for readiness, or that depends on AWS refusing an operation while the domain is still Creating, sees a domain that is instantly ready; the inactive-domain refusals AWS enforces never fire.
Fix: the domain lifecycle in the CreateDomain handler / domain state must hold Creating (Processing=true, Endpoint unpublished) until provisioning completes.
Traces: opensearch.attach-data-source-inactive-domain-state, opensearch.attach-detach-data-source-concurrent-race-conflict, opensearch.data-source-attachment-lifecycle.

Cause 6 of 7: AttachDataSource's error precedence puts validation ahead of the resource lookup.
AWS: Smithy: AttachDataSource models both ValidationException (400) and ResourceNotFoundException (409). Recorded behavior: for a duplicate attach whose referenced resource does not exist, AWS answers ResourceNotFoundException — the existence of the referenced resource is decided before the duplicate/validation verdict.
Emulator: the same call answers ValidationException.
Why: the client is told its input is malformed when the resource is simply absent, inverting the retry decision (fix-the-request vs create-the-resource).
Fix: the ordering of lookup and validation in the AttachDataSource handler.
Traces: opensearch.attach_data_source_duplicate_rejected.

Cause 7 of 7: a concurrent DetachDataSource race is refused ResourceNotFoundException instead of ConflictException.
AWS: Smithy: DetachDataSource models ConflictException (409) and ResourceNotFoundException (409). Recorded behavior: when two detaches race on the same attachment, the loser is refused ConflictException (the attachment is in use / mid-transition), not ResourceNotFoundException.
Emulator: the losing call gets ResourceNotFoundException because the attachment record is already purged from state.
Why: a concurrent caller cannot tell a serialization conflict (retry) from a genuinely absent attachment (do not retry); idempotent detach wrappers mis-handle the race.
Fix: the DetachDataSource handler / attachment state must serialize concurrent detaches and keep the record long enough to answer ConflictException.
Traces: opensearch.concurrent-detach-data-source-race-idempotent.

## Evidence
Cause 1 — expected a ClientError (ValidationException); actual HTTP 200 `{'Status': 'UPDATED'}` for the malformed mapping, and `expected a ClientError, call succeeded` for the malformed settings.
Cause 2 — expected IndexSchema == {"<index>": {"mappings": ..., "settings": ...}}; actual `KeyError: 'sim2real-mapping-idx'` at `doc[index_name]["mappings"]["properties"]`, and `KeyError: 'sim2real-widgets'`.
Cause 3 — `expected ValidationException for UpdateIndex on an engine that doesn't support index management, got ResourceNotFoundException`; `expected ValidationException with AWS's exact software-version-gate message, got ('ResourceNotFoundException', 'Resource index sim2real-depfail-idx does not exist')`.
Cause 4 — expected status ATTACHED; actual `data source attachment landed in 'FAILED' instead of ATTACHED` (4 checks) and `attachment sfi9sv1baaaa/arn:aws:aoss:us-west-2:...:collection/... entered FAILED`.
Cause 5 — expected Processing=true and no Endpoint during the Creating window; actual `domain sim2real-od-18ee213a2f19 left its creating window while the check held it: Processing=True, Endpoint='search-sim2real-od-...es.amazonaws.com'` and `Processing=False, Endpoint='search-sim2r-osadom-...'`.
Cause 6 — `expected ResourceNotFoundException, got ValidationException`.
Cause 7 — `unexpected failure code 'ResourceNotFoundException' (expected ConflictException)`.

## Steps to Reproduce
1. Cause 1: create a domain, CreateIndex a simple index, then UpdateIndex(DomainName=<d>, IndexName=<i>, IndexSchema={"mappings": <malformed>}) and again with malformed settings.
2. Cause 2: CreateIndex(IndexSchema={"mappings": {"properties": {"myfield": {"type": "text"}}}}) then GetIndex(DomainName=<d>, IndexName=<i>) and read response["IndexSchema"][<i>].
3. Cause 3: UpdateIndex on a domain whose engine version does not support index management, and UpdateIndex naming an index that was never created.
4. Cause 4: CreateApplication, AttachDataSource with a valid same-account domain ARN, then poll DescribeDataSourceAttachment until terminal — observe FAILED.
5. Cause 5: CreateDomain and immediately DescribeDomain in a loop — the domain never reports Processing=true with no Endpoint.
6. Cause 6: AttachDataSource twice with a dataSourceArn naming a resource that does not exist.
7. Cause 7: issue two concurrent DetachDataSource calls for the same attachment and inspect the loser's error code.
