Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.concurrent-identity-center-config-modification-rejected glue.identity-center-configuration-enforces-managed-access-control glue.create-table-optimizer-nonexistent-table-not-found glue.get-dashboard-url-returns-valid-resource-url glue.get-unfiltered-table-metadata-federation-retryable-error glue.table-optimizer-lifecycle-create-get-delete-404 firehose__glue__kinesis__s3.kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker firehose__glue__kinesis__s3.kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location glue.glue-update-table-with-a-stale-version-id-rejected-and-the-live-table-unchanged glue.job-bookmark-absent-until-run glue.batch-create-partition-arity-mismatch-partial glue.create-usage-profile-invalid-configuration-rejected glue.glue-tsd-get-tables-pagination-follows-next-token-to-every-table
Issue-Key: glue.concurrent-identity-center-config-modification-rejected|run#4|AssertionError|assert existing_code == "EntityNotFoundException", existing_code
Issue-Key: glue.identity-center-configuration-enforces-managed-access-control|run#4|AssertionError|assert code == "EntityNotFoundException", (
Issue-Key: glue.create-table-optimizer-nonexistent-table-not-found|run#15|AssertionError|assert code == "EntityNotFoundException", f"expected EntityNotFoundException, got {code!r}"
Issue-Key: glue.get-dashboard-url-returns-valid-resource-url|run#4|AssertionError|assert code == "OperationNotSupportedException", (
Issue-Key: glue.get-unfiltered-table-metadata-federation-retryable-error|run#11|AssertionError|assert get_code == "FederationSourceRetryableException", (
Issue-Key: glue|CreateTable|NotImplemented
Issue-Key: firehose__glue__kinesis__s3|CreateTable|NotImplemented
Issue-Key: glue|UpdateTable|NotImplemented
Issue-Key: glue.job-bookmark-absent-until-run|run#12|AssertionError|assert reset_code == "EntityNotFoundException", (
Issue-Key: glue.batch-create-partition-arity-mismatch-partial|run#10|AssertionError|assert code == "InvalidInputException", (
Issue-Key: glue.create-usage-profile-invalid-configuration-rejected|run#6|AssertionError|assert code == "InvalidInputException", f"expected InvalidInputException, got {code}"
Issue-Key: glue|GetTables|ValidationException
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Glue: 10 net-new root causes — Identity Center, table optimizer, dashboard URL, unfiltered-metadata, Iceberg CreateTable, UpdateTable+VersionId and ResetJobBookmark answer NotImplemented instead of their modeled errors; BatchCreatePartition/CreateUsageProfile rejections unreachable; GetTables refuses MaxResults=1000

> _Note: this issue was identified against the code base at commit `e9591ea0ffea53e5939624d35a733297d20386ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGlueIdentityCenterConfiguration

## Description
This issue bundles 10 net-new root causes in the Glue control plane. Every one of them is a call that real AWS answers with a modeled Glue error (or a success) and that this emulator answers with the scaffold's `NotImplemented` error code (body message of the form 'operation <Op> is scaffolded but not implemented in this emulator'), or — in the last cause — with an unmodeled refusal of a request AWS accepts. Each cause must be closed before this issue is done. Where an already-filed stub issue names the same operation it is called out inside the cause; those issues say 'implement the operation', while the causes below additionally state the error class and state effect the operation owes.

Cause 1 of 10: the Glue Identity Center configuration operations are stubs, so the account-singleton's 'not configured' state cannot be observed.
AWS: Smithy com.amazonaws.glue#GetGlueIdentityCenterConfiguration takes no input, returns ApplicationArn/InstanceArn/Scopes/UserBackgroundSessionsEnabled and models EntityNotFoundException, ConcurrentModificationException, AccessDeniedException, InvalidInputException, OperationTimeoutException (all httpError 400) and InternalServiceException (500). On an account with no configuration the recorded behavior is a 400 EntityNotFoundException (recorded example message: 'Idc application not found. Need to create glue application first.'); racing CreateGlueIdentityCenterConfiguration calls leave exactly one 200 and refuse the rest with ConcurrentModificationException or AlreadyExistsException; DeleteGlueIdentityCenterConfiguration returns the account to the EntityNotFound state.
Emulator: the first probe already fails — GetGlueIdentityCenterConfiguration answers error code 'NotImplemented' instead of EntityNotFoundException, so neither the create race nor the delete half can be reached.
Why: an SDK maps the Glue error code onto a modeled exception class, so a client that catches EntityNotFoundException to mean 'not configured yet' sees an unmodeled/unknown error instead and cannot branch at all; the whole singleton lifecycle is unusable.
Fix: implement the Get/Create/Delete GlueIdentityCenterConfiguration handlers in services/service-com-amazonaws-glue-awsglue/src/ops/ over an account-singleton item in the Glue state, with the serialized-create behavior described above.
Traces: glue.concurrent-identity-center-config-modification-rejected, glue.identity-center-configuration-enforces-managed-access-control.

Cause 2 of 10: CreateTableOptimizer/GetTableOptimizer are stubs, so a table optimizer on a table that does not exist is not refused as not-found.
AWS: Smithy CreateTableOptimizer (CatalogId, DatabaseName, TableName, Type, TableOptimizerConfiguration all required) models EntityNotFoundException, AlreadyExistsException, AccessDeniedException, InvalidInputException, ThrottlingException, ValidationException; GetTableOptimizer models EntityNotFoundException. The recorded behavior for a TableName absent from the catalog is a 400 EntityNotFoundException from CreateTableOptimizer, with no optimizer persisted, so the follow-up GetTableOptimizer is EntityNotFoundException too.
Emulator: both calls answer error code 'NotImplemented'.
Why: a caller cannot distinguish 'the table is missing' from 'the service is broken', and cannot verify that a refused create left nothing behind.
Fix: implement create_table_optimizer.rs / get_table_optimizer.rs (and the delete/list siblings) against the catalog's table state, validating table existence before persisting.
Traces: glue.create-table-optimizer-nonexistent-table-not-found.

Cause 3 of 10: GetDashboardUrl is a stub, so neither of its two refusal classes exists.
AWS: Smithy GetDashboardUrl (ResourceId, ResourceType required; output Url required, sensitive) models OperationNotSupportedException, EntityNotFoundException, AccessDeniedException, InvalidInputException and InternalServiceException. Recorded: ResourceType=JOB is refused outright with 400 OperationNotSupportedException regardless of whether the job exists (recorded example message 'GetDashboardUrl operation is not supported for JOB'), and an unknown SESSION id is refused with 400 EntityNotFoundException rather than being handed a URL.
Emulator: error code 'NotImplemented' for both.
Why: the operation's whole contract is which resource types it supports; a client sees an unmodeled error for a case AWS classifies precisely, and an emulator that later returns a URL for an unknown session would be worse still.
Fix: implement get_dashboard_url.rs with the per-ResourceType dispatch (JOB unsupported, SESSION looked up in session state).
Traces: glue.get-dashboard-url-returns-valid-resource-url.

Cause 4 of 10: GetUnfilteredTableMetadata is a stub, so the federated-source retryable error is never produced.
AWS: Smithy GetUnfilteredTableMetadata models FederationSourceRetryableException and FederationSourceException alongside EntityNotFoundException, PermissionTypeMismatchException, InvalidInputException and GlueEncryptionException (all 400). Recorded: against a federated database whose source was accepted at registration but is unreachable at read time, the call fails with 400 FederationSourceRetryableException — the code that tells a client to retry rather than give up.
Emulator: error code 'NotImplemented'.
Why: federation error handling inverts — retryable failures become an unmodeled permanent-looking error, so a client's retry path is never exercised.
Fix: implement get_unfiltered_table_metadata.rs, including propagation of a federated catalog's source failure as FederationSourceRetryableException.
Traces: glue.get-unfiltered-table-metadata-federation-retryable-error.

Cause 5 of 10: CreateTable with OpenTableFormatInput (the Iceberg table marker) is an unimplemented partial stub, so no open-table-format table can exist in the catalog.
AWS: Smithy CreateTable carries the optional member OpenTableFormatInput (IcebergInput: MetadataOperation, Version) beside TableInput; with it, Glue creates the table and mints the Iceberg metadata, so the table reads back with its metadata_location table parameter and can be used as a table-optimizer or Firehose Iceberg destination.
Emulator: CreateTable answers error code 'NotImplemented' with 'operation CreateTable with OpenTableFormatInput is scaffolded but not implemented in this emulator' — note that plain CreateTable works, so this is a partial stub on one request shape, not a missing operation.
Why: every Iceberg-based flow is unreachable: the table optimizer lifecycle, Firehose Iceberg destination validation and the metadata-location assertions all stop at table creation, and a client cannot even create the fixture it needs.
Fix: the OpenTableFormatInput branch of services/service-com-amazonaws-glue-awsglue/src/ops/create_table.rs, plus the Iceberg metadata it must record in the table's parameters/state.
Traces: glue.table-optimizer-lifecycle-create-get-delete-404, firehose__glue__kinesis__s3.kds-create-delivery-stream-accepts-a-glue-table-with-no-iceberg-marker, firehose__glue__kinesis__s3.kds-create-delivery-stream-rejects-a-destination-database-that-does-not-exist, firehose__glue__kinesis__s3.kds-iceberg-input-mints-a-metadata-json-and-a-metadata-location. (Service attribution: those three check ids carry a compound token, but the failing call is glue:CreateTable and the fix is in the Glue handler, so they are filed here under glue.)

Cause 6 of 10: UpdateTable with VersionId is an unimplemented partial stub, so optimistic-concurrency updates cannot be made at all and a stale VersionId is never refused.
AWS: Smithy UpdateTable carries VersionId (VersionString) and models ConcurrentModificationException among its errors. Recorded: an UpdateTable whose VersionId is the current version applies and advances the version pointer; one whose VersionId is no longer current (or never existed) is refused with 400 ConcurrentModificationException (recorded example message 'Update table failed due to concurrent modifications.') — not VersionMismatchException — and the refusal leaves the live table, the version pointer and the archived version history untouched.
Emulator: UpdateTable answers error code 'NotImplemented' with 'operation UpdateTable with VersionId is scaffolded but not implemented in this emulator', so even the accepted, current-version update fails.
Why: the only API-level concurrency control on a Glue table is unusable; a client cannot write safely, and cannot test that a lost race writes nothing.
Fix: the VersionId branch of update_table.rs — compare against the current version in the table state, refuse with ConcurrentModificationException when stale, and make the refusal write nothing (no version bump, no archived version).
Traces: glue.glue-update-table-with-a-stale-version-id-rejected-and-the-live-table-unchanged.

Cause 7 of 10: ResetJobBookmark is a stub, so a never-run job's bookmark is neither absent nor resettable.
AWS: Smithy ResetJobBookmark (JobName required, RunId optional; output JobBookmarkEntry) models EntityNotFoundException, InvalidInputException, OperationTimeoutException and InternalServiceException. Recorded: a job that has never run has no bookmark, and ResetJobBookmark against it is refused with 400 EntityNotFoundException.
Emulator: error code 'NotImplemented'.
Why: the bookmark-absent-until-first-run invariant cannot be observed, and a client's not-found branch gets an unmodeled code.
Fix: implement reset_job_bookmark.rs (and the GetJobBookmark sibling) over per-job bookmark state, with EntityNotFoundException when no bookmark entry exists.
Traces: glue.job-bookmark-absent-until-run.

Cause 8 of 10: BatchCreatePartition cannot refuse a whole request whose partition values do not match the table's partition keys.
AWS: Smithy BatchCreatePartition takes PartitionInputList (0..100) and returns per-entry Errors, but models InvalidInputException (400) for a malformed request. Recorded: when a PartitionInput carries fewer values than the table has partition keys, the WHOLE request is refused with 400 InvalidInputException — the arity mismatch is a request-level rejection, not a per-entry error, so the response has no Errors list and no partition is created.
Emulator: BatchCreatePartition answers error code 'NotImplemented' (Errors=None). The already-filed stub issue for BatchCreatePartition (services/service-com-amazonaws-glue-awsglue/src/ops/batch_create_partition.rs) covers standing the operation up; this cause additionally fixes WHERE the arity check belongs — request-level InvalidInputException, not a per-entry error.
Why: a client that implements the documented partial-success contract would, once the stub is replaced by a naive per-entry implementation, see an Errors list where AWS refuses the whole call.
Fix: batch_create_partition.rs — validate each PartitionInput's value arity against the table's PartitionKeys before any entry is applied and refuse the request with InvalidInputException.
Traces: glue.batch-create-partition-arity-mismatch-partial.

Cause 9 of 10: CreateUsageProfile cannot refuse an invalid Configuration.
AWS: Smithy CreateUsageProfile (Name, Configuration required) models InvalidInputException, AlreadyExistsException, OperationNotSupportedException, ResourceNumberLimitExceededException and OperationTimeoutException (all 400). Recorded: a ProfileConfiguration that is not internally valid is refused with 400 InvalidInputException and no profile is created.
Emulator: error code 'NotImplemented'. The already-filed stub issue for CreateUsageProfile covers standing the operation up; this cause states the validation and its error class that the implementation owes.
Why: a client validating its profile definitions gets an unmodeled error instead of the field-level rejection, and cannot tell an invalid configuration from a missing feature.
Fix: create_usage_profile.rs — validate ProfileConfiguration and refuse with InvalidInputException before persisting.
Traces: glue.create-usage-profile-invalid-configuration-rejected.

Cause 10 of 10: GetTables refuses MaxResults=1000 with an unmodeled ValidationException where AWS accepts the call.
AWS: recorded behavior — GetTables(DatabaseName, MaxResults=1000) over a five-table database succeeds and returns all five tables in one page with no NextToken. (Smithy does declare GetTablesRequest.MaxResults as CatalogGetterPageSize range 1..100, but the recording shows the service not enforcing that bound; and ValidationException is NOT in GetTables' modeled error set, which is EntityNotFoundException, InvalidInputException, GlueEncryptionException, FederationSource[Retryable]Exception, OperationTimeoutException, InternalServiceException. So even for a value AWS did refuse, the owed code is InvalidInputException.)
Emulator: 'An error occurred (ValidationException) when calling the GetTables operation: 1 validation error detected: Value \'1000\' at \'maxResults\' failed to satisfy constraint: Member must have value less than or equal to 100'.
Why: the common 'one big page' listing call fails outright, and when a page size really is out of range the caller receives a cross-service ValidationException that no Glue SDK error class matches.
Fix: the input validation in services/service-com-amazonaws-glue-awsglue/src/ops/get_tables.rs — accept the larger page size as the service does, and route any Glue input rejection through Glue's InvalidInputException rather than the generic ValidationException constructor.
Traces: glue.glue-tsd-get-tables-pagination-follows-next-token-to-every-table.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against this checkout (20/223 checks passed).

Cause 1 — glue.identity-center-configuration-enforces-managed-access-control: "AssertionError: expected no Glue Identity Center configuration before the check, got 'NotImplemented'"; glue.concurrent-identity-center-config-modification-rejected: same probe, "AssertionError: NotImplemented" at `assert existing_code == 'EntityNotFoundException'`. Expected code EntityNotFoundException (400).
Cause 2 — glue.create-table-optimizer-nonexistent-table-not-found: "AssertionError: expected EntityNotFoundException, got 'NotImplemented'" from create_table_optimizer(CatalogId, DatabaseName, TableName=<absent>, Type='compaction').
Cause 3 — glue.get-dashboard-url-returns-valid-resource-url: "AssertionError: expected OperationNotSupportedException for ResourceType=JOB, got 'NotImplemented'".
Cause 4 — glue.get-unfiltered-table-metadata-federation-retryable-error: "AssertionError: expected FederationSourceRetryableException from GetUnfilteredTableMetadata against an accepted-but-unreachable federated database, got 'NotImplemented'".
Cause 5 — glue.table-optimizer-lifecycle-create-get-delete-404 and the three firehose__glue__kinesis__s3 kds-* checks: "botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateTable operation: operation CreateTable with OpenTableFormatInput is scaffolded but not implemented in this emulator".
Cause 6 — glue.glue-update-table-with-a-stale-version-id-rejected-and-the-live-table-unchanged: "(NotImplemented) when calling the UpdateTable operation: operation UpdateTable with VersionId is scaffolded but not implemented in this emulator" — raised on the FIRST, legitimate update whose VersionId is current; expected: that update applies, and the later stale-VersionId update is refused 400 ConcurrentModificationException with the table, version pointer and version history unchanged.
Cause 7 — glue.job-bookmark-absent-until-run: "AssertionError: ResetJobBookmark on a never-run job must 404, got NotImplemented".
Cause 8 — glue.batch-create-partition-arity-mismatch-partial: "AssertionError: too few values: the whole request must be refused with InvalidInputException, got 'NotImplemented' (Errors=None)".
Cause 9 — glue.create-usage-profile-invalid-configuration-rejected: "AssertionError: expected InvalidInputException, got NotImplemented".
Cause 10 — glue.glue-tsd-get-tables-pagination-follows-next-token-to-every-table: "botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetTables operation: 1 validation error detected: Value '1000' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100". The MaxResults=2 paging arm of the same check (pages 2,2,1 with NextToken only on non-final pages) was reached and is not part of this cause.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. Cause 1: glue:GetGlueIdentityCenterConfiguration on a fresh account — expect 400 EntityNotFoundException, observe NotImplemented.
3. Cause 2: CreateDatabase, then glue:CreateTableOptimizer with a TableName that does not exist in it — expect 400 EntityNotFoundException.
4. Cause 3: glue:GetDashboardUrl(ResourceId=<any>, ResourceType='JOB') — expect 400 OperationNotSupportedException; then ResourceType='SESSION' with an unknown id — expect 400 EntityNotFoundException.
5. Cause 4: register a federated catalog whose source is unreachable, then glue:GetUnfilteredTableMetadata against a database in it — expect 400 FederationSourceRetryableException.
6. Cause 5: glue:CreateTable with OpenTableFormatInput={'IcebergInput': {'MetadataOperation': 'CREATE'}} — expect 200 and an Iceberg table readable with its metadata location.
7. Cause 6: CreateTable, GetTable for VersionId, UpdateTable with that VersionId (must apply), then UpdateTable again with the now-stale VersionId — expect 400 ConcurrentModificationException and no state change.
8. Cause 7: CreateJob, then glue:ResetJobBookmark(JobName=<that job>) before any run — expect 400 EntityNotFoundException.
9. Cause 8: create a table with two partition keys, then glue:BatchCreatePartition with a PartitionInput carrying one value — expect the whole request refused 400 InvalidInputException with no Errors list.
10. Cause 9: glue:CreateUsageProfile with an invalid Configuration — expect 400 InvalidInputException.
11. Cause 10: create five tables in a database, then glue:GetTables(DatabaseName=..., MaxResults=1000) — expect one page of five tables and no NextToken.
