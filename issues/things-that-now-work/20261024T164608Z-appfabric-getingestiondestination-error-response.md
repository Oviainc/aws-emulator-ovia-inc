Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2a22026c0f737b6187455fb636429eae77d015a7

# GetIngestionDestination reports a missing app bundle or ingestion as a missing destination instead of naming the absent link

_Filed: 2026-10-24T16:46:08Z_
_Operation: GetIngestionDestination | Class: error-response_

> _Note: this issue was identified against the code base at commit `2a22026c0f737b6187455fb636429eae77d015a7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

AppFabric's not-found refusals name the KIND of resource that is actually absent — the repo's own trace-derived rule records this (GetAppBundle on a dead bundle: "The app bundle ID was not found."; ListIngestionDestinations with an ingestionIdentifier that does not exist inside a real bundle: HTTP 404 ResourceNotFoundException with "The specified ingestion does not exist").

GetIngestionDestination walks the same three-level chain (bundle -> ingestion -> destination) but collapses every unresolved link into one single message: the handler builds a `not_found` closure that always constructs `ingestion_destination_not_found(...)` ("The specified ingestion destination does not exist") and returns it for a missing app bundle, a missing ingestion and a missing destination alike.

Expected (per the recorded sibling read on the same chain): a bundle identifier naming nothing stored is refused with the app-bundle read-path sentence ("The app bundle ID was not found."), an ingestion identifier naming nothing stored is refused with "The specified ingestion does not exist", and only an absent destination is refused with the destination sentence. Actual: all three answer HTTP 404 ResourceNotFoundException with the destination sentence (and `resourceType`/`resourceId` describing a destination), so a client cannot tell which level of the chain it got wrong, and the message does not match AWS for two of the three cases.

Note the emulator already words this correctly on the sibling listing operation (list_ingestion_destinations.rs uses `app_bundle_not_found` for the bundle and `ingestion_not_found` for the ingestion), so the divergence is isolated to this reader.

## Evidence

Spec/trace grounding: the model documents ResourceNotFoundException as "The specified resource does not exist." for all of these ops; the repo's trace-derived issue 20261023T031706Z-appfabric-8-behavioral-divergences (Cause 2) records AWS answering ListIngestionDestinations for an unknown ingestionIdentifier with HTTP 404 ResourceNotFoundException, message "The specified ingestion does not exist", and (Cause 5) pins AppFabric's constant per-resource-kind read-path wording "The app bundle ID was not found." src/state.rs lines 1254-1312 codify that rule (`app_bundle_not_found`, `ingestion_not_found`, `ingestion_destination_not_found`).
Handler: src/ops/get_ingestion_destination.rs lines 178-195 — one `not_found` closure returning `crate::state::ingestion_destination_not_found(&input.ingestion_destination_identifier)` is used for the failed bundle lookup (line 186), the failed ingestion lookup (line 189) and the failed destination lookup (line 194).
Contrast in the same crate: src/ops/list_ingestion_destinations.rs lines 189-204 uses `app_bundle_not_found` then `ingestion_not_found` for exactly the same two parents.
Test coverage: tests/all/ingestion_destination_lifecycle.rs `a_read_reports_the_destination_as_absent_whichever_link_is_missing` (lines 689-774) asserts the current collapsed behavior (code only plus the destination resourceType), so that test pins the divergence and must be reworked with the fix; no test asserts the per-level AWS wording for this operation.

## Steps to Reproduce

1. POST /appbundles with an empty body -> 201, note appBundle.arn (call it B)
2. POST /appbundles/{B}/appauthorizations (app=SLACK, authType=apiKey, credential.apiKeyCredential.apiKey=..., tenant={tenantIdentifier:t-1,tenantDisplayName:t-1}) then POST .../connect so an ingestion can be created
3. POST /appbundles/{B}/ingestions (app=SLACK, tenantId=t-1, ingestionType=auditLog) -> 201, note ingestion.arn (call it I)
4. GET /appbundles/{B}/ingestions/{random-uuid}/ingestiondestinations/{random-uuid} -> emulator answers 404 with message "The specified ingestion destination does not exist"; AWS names the absent ingestion ("The specified ingestion does not exist"), as the recorded ListIngestionDestinations call on the same chain shows
5. GET /appbundles/{random-uuid}/ingestions/{I}/ingestiondestinations/{random-uuid} -> emulator answers the same destination message; AWS reports the app bundle read-path sentence "The app bundle ID was not found."
