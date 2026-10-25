Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# DeleteIngestionDestination answers the read-path not-found message for an unknown app bundle on a mutation path

_Filed: 2026-10-24T16:46:08Z_
_Operation: DeleteIngestionDestination | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AppFabric words an unknown app bundle differently on its read and its write paths: a reporting operation answers "The app bundle ID was not found." while a mutation answers "The specified app bundle ID is not valid" (both HTTP 404 ResourceNotFoundException, neither echoing the caller's identifier). That split is trace-derived and is codified in this crate (src/state.rs `app_bundle_not_found` vs `app_bundle_id_not_valid`) and asserted for the pair GetAppBundle / CreateAppAuthorization in tests.

DeleteIngestionDestination is a mutation, but its bundle-level refusal calls `app_bundle_not_found`, i.e. the READ-path sentence. Every other mutation in the crate (CreateIngestion, CreateAppAuthorization, ConnectAppAuthorization, CreateIngestionDestination, UpdateIngestionDestination, DeleteIngestion, StopIngestion, StartUserAccessTasks) uses `app_bundle_id_not_valid` for the same scenario — DeleteIngestionDestination is the sole outlier, and its own code comment claims it gives "the same answer ... DeleteIngestion give[s]", which it does not.

Expected: DELETE /appbundles/{unknown}/ingestions/{i}/ingestiondestinations/{d} -> HTTP 404 ResourceNotFoundException with the mutation-path sentence "The specified app bundle ID is not valid". Actual: HTTP 404 ResourceNotFoundException with "The app bundle ID was not found."

## Evidence

Spec/trace grounding: the repo's trace-derived issue 20261023T031706Z-appfabric-8-behavioral-divergences (Cause 5) records two independent AWS recordings establishing the two constants — read path "The app bundle ID was not found." (GetAppBundle) and mutation path "The specified app bundle ID is not valid" (CreateAppAuthorization) — and that neither interpolates the identifier.
Handler: src/ops/delete_ingestion_destination.rs lines 190-197 calls `crate::state::app_bundle_not_found(&input.app_bundle_identifier)`; src/ops/delete_ingestion.rs line 180 (the sibling delete one level up) calls `crate::state::app_bundle_id_not_valid(...)` for the identical case, as do all other write paths (grep of app_bundle_not_found / app_bundle_id_not_valid across src/ops shows delete_ingestion_destination.rs as the only mutation using the read-path constructor).
Test coverage: tests/all/ingestion_destination_lifecycle.rs lines 1262-1290 assert only the status (404) and error code (ResourceNotFoundException) for this case — the message is unasserted, so no test pins either wording here, while tests/all/app_authorization_and_ingestion_dependency.rs lines 1238-1281 assert the read/mutation split for the GetAppBundle / CreateAppAuthorization pair.

## Steps to Reproduce

1. POST /appbundles with an empty body -> 201, note appBundle.arn
2. DELETE /appbundles/{a-uuid-no-create-ever-minted}/ingestions/{any-uuid}/ingestiondestinations/{any-uuid}
3. Read Error.Message: the emulator returns "The app bundle ID was not found." (the reporting-path constant)
4. Compare with DELETE /appbundles/{the-same-unknown-uuid}/ingestions/{any-uuid}, which returns the mutation-path constant "The specified app bundle ID is not valid" — AppFabric uses that same mutation wording for the destination delete
