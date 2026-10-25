Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.update-ingestion-destination-changes-prefix-persists
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|GetIngestionDestination|ResourceNotFoundException
Issue-Commit: 521a9a87ca33ad668d554c20273a56e6e89cc063

# GetIngestionDestination reports a missing nested chain link as the ingestion instead of the ingestion destination the call targets

> _Note: this issue was identified against the code base at commit `521a9a87ca33ad668d554c20273a56e6e89cc063`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIngestionDestination

## Description
One cause.

Cause 1 of 1: GetIngestionDestination attributes a ResourceNotFoundException to the wrong link of the app bundle -> ingestion -> ingestion destination chain. Failing check: appfabric.update-ingestion-destination-changes-prefix-persists.

AWS: Smithy (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json) models GetIngestionDestination with required httpLabel members appBundleIdentifier, ingestionIdentifier and ingestionDestinationIdentifier, and error ResourceNotFoundException (errorKind client, httpError 404) whose shape requires message, resourceId and resourceType. The model does not state which chain link a refusal names, so for that: recorded behavior only — the recording shows that when the app bundle exists but the supplied ingestionIdentifier and ingestionDestinationIdentifier are both unknown UUIDs, AWS refuses the read as the ingestion DESTINATION being absent: HTTP 404, code ResourceNotFoundException, with the message (example) "The specified ingestion destination does not exist". In other words AWS attributes the not-found to the leaf resource the operation is named for, not to the first ancestor it failed to resolve. Hypothesis: the real service resolves the destination by its own identifier scope rather than walking parents one at a time.

Emulator: the handler walks the chain and refuses at the FIRST link that resolves to nothing, so the same request is refused as the ingestion being absent. The replayed call returned code ResourceNotFoundException (correct) but with message 'The specified ingestion does not exist', i.e. the refusal carries the ingestion's resource attribution (resourceType/resourceId built by the ingestion-not-found helper) rather than the ingestion destination's. The HTTP status (404) and the error code match AWS; only which resource the refusal is ABOUT diverges.

Why: a client that branches on the refusal's resource attribution — the required resourceType/resourceId members of ResourceNotFoundException, or the message a human reads in a log — is told it got the ingestion id wrong when AWS tells it the ingestion destination does not exist. Retry/repair logic that re-resolves the ingestion (re-listing ingestions, re-creating one) chases the wrong level of the chain and never converges, and any test that asserts AWS's not-found attribution for a destination read fails against the emulator.

Fix: belongs in the GetIngestionDestination handler's not-found attribution — the chain-walk refusal path in services/service-com-amazonaws-appfabric-fabricfrontendservice/src/ops/get_ingestion_destination.rs, which selects between the crate::state::ingestion_not_found and crate::state::ingestion_destination_not_found constructors in src/state.rs. The leaf read must report the ingestion destination as absent when a descendant of a resolvable app bundle cannot be resolved. The handler's module doc states the first-absent-link rule as intentional and says the sibling reads (ListIngestionDestinations, GetIngestion) share it, so the sibling reads of this chain should be reviewed for the same attribution rule rather than patched in one place. Note: no open appfabric issue covers this — the nearest ones (20261024T230046Z-appfabric-createingestion-wrong-behavior.md, 20261024T065514Z-appfabric-deleteappbundle-validation.md) concern child-ARN regions and DeleteAppBundle validation, not not-found attribution on reads.

Traces: appfabric.update-ingestion-destination-changes-prefix-persists (the check creates a real app bundle, then calls GetIngestionDestination with a bogus ingestion id and a bogus destination id).

## Evidence
Check: appfabric.update-ingestion-destination-changes-prefix-persists (mismatch).

Request (us-east-1): CreateAppBundle -> real bundle ARN; then GetIngestionDestination(appBundleIdentifier=<real bundle arn>, ingestionIdentifier=<random uuid>, ingestionDestinationIdentifier=<random uuid>).

Expected (recorded AWS): HTTP 404, Error.Code = ResourceNotFoundException, refusal attributed to the ingestion destination; Error.Message example "The specified ingestion destination does not exist".

Actual (emulator): HTTP 404, Error.Code = ResourceNotFoundException (matches), refusal attributed to the ingestion; Error.Message 'The specified ingestion does not exist'.

Assertion: AssertionError: expected AWS's exact not-found message for the ingestion destination, got 'The specified ingestion does not exist'.

Code path: src/ops/get_ingestion_destination.rs lines 188-195 return crate::state::ingestion_not_found(&input.ingestion_identifier) as soon as bundle.resolve_ingestion fails, so the destination branch at lines 196-207 (crate::state::ingestion_destination_not_found) is never reached for this request. The positive control in the same check passed: GetAppBundle still returned the untouched bundle, so the rejected call wrote nothing.

## Steps to Reproduce
1. Point a botocore client at the emulator for service 'appfabric', region us-east-1.
2. appfabric.create_app_bundle(clientToken=<uuid>, tags=[{"key":"env","value":"sim2real"}]) and keep the returned appBundle.arn.
3. appfabric.get_ingestion_destination(appBundleIdentifier=<that arn>, ingestionIdentifier=str(uuid.uuid4()), ingestionDestinationIdentifier=str(uuid.uuid4())).
4. Inspect the ClientError: the code is ResourceNotFoundException (correct) but the refusal is attributed to the ingestion ('The specified ingestion does not exist') instead of the ingestion destination, which is what AWS reports for this request.
5. Clean up with appfabric.delete_app_bundle(appBundleIdentifier=<that arn>).
