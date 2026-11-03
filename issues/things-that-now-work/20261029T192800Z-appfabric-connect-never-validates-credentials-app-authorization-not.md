Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed appfabric.get-app-authorization-nonexistent-not-found appfabric.delete-app-authorization-removes-resource appfabric.delete-app-bundle-with-contents-rejected appfabric.start-ingestion-without-connected-authorization-rejected
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed|run#14|AssertionError|assert final_status == "ConnectionValidationFailed", (
Issue-Key: appfabric.get-app-authorization-nonexistent-not-found|run#11|AssertionError|assert message == "The app authorization was not found.", (
Issue-Key: appfabric.delete-app-authorization-removes-resource|run#10|AssertionError|assert message == "The app authorization was not found.", (
Issue-Key: appfabric.delete-app-bundle-with-contents-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: appfabric.start-ingestion-without-connected-authorization-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf

# AppFabric: connect never validates credentials, app-authorization not-found and CreateIngestion/DeleteAppBundle refusals use the wrong code and wording (4 causes)

> _Note: this issue was identified against the code base at commit `2216fbd2f00c4c2b6b2036585ad12df5e43cbdbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConnectAppAuthorization, GetAppAuthorization, UpdateAppAuthorization, DeleteAppBundle, CreateIngestion

## Description
Four independent root causes in the AppFabric crate, all found by replaying the behavioral suite against the running emulator (19/24 appfabric checks pass, these 5 fail). A fixer must close ALL FOUR before submitting.

--------------------------------------------------
Cause 1 of 4: ConnectAppAuthorization connects an oauth2 authorization whose credential/auth code can never validate
--------------------------------------------------
AWS: ConnectAppAuthorization performs the provider handshake, so an oauth2 authorization whose clientId/clientSecret and authRequest.code are not real can never reach Connected. AWS ends in exactly one of two terminal outcomes: (a) a synchronous refusal HTTP 400 ValidationException whose message has the fixed text "The specified oauth2 app authorization configuration is not valid. The error returned by the application is: " followed by the provider's own error text (for Slack, the variable tail is the provider response, e.g. the token-API error body — provider text and body are values, not part of the template), after which the record stays PendingConnect or moves to ConnectionValidationFailed; or (b) HTTP 200 followed by the record transitioning to status ConnectionValidationFailed. Smithy: ConnectAppAuthorization errors include ValidationException (httpError 400, members message/reason required); AppAuthorizationStatus enum = [PendingConnect, Connected, ConnectionValidationFailed, TokenAutoRotationFailed] — ConnectionValidationFailed exists precisely for this outcome. Recorded behavior pins the two acceptable terminal outcomes.
Emulator: the call is answered 200 and the authorization's status becomes 'Connected' unconditionally, and stays there: the replay polled GetAppAuthorization until status left PendingConnect and observed 'Connected'. The handler writes status = APP_AUTHORIZATION_STATUS_CONNECTED with no validation branch at all (connect_app_authorization.rs; see also the already-open issue 20261029T093023Z-appfabric-connectappauthorization-wrong-behavior.md, which describes the updatedAt re-stamp in that same unconditional write but NOT the missing validation outcome — that issue does not cover this cause).
Why: ConnectionValidationFailed is unreachable in the emulator, so every client that tests its connect-failure path — a console/SDK flow that polls for ConnectionValidationFailed and re-prompts the user for credentials, or any error-handling test built on it — sees a false success. Worse, downstream state is wrong too: the authorization is now a usable, Connected authorization, so CreateIngestion and ingestion start succeed against an authorization that in AWS never connected.
Fix: ConnectAppAuthorization's handler and the app-authorization state machine — the connect path needs a validation outcome that either refuses with ValidationException or writes status = ConnectionValidationFailed instead of unconditionally writing Connected.
Traces: appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed (AssertionError: expected ConnectionValidationFailed, got 'Connected').

--------------------------------------------------
Cause 2 of 4: the app-authorization ResourceNotFoundException message is the emulator's own wording, not AWS's
--------------------------------------------------
AWS: a read or mutation naming an app authorization that does not exist in the named bundle is refused HTTP 404 ResourceNotFoundException with message exactly "The app authorization was not found." — that whole string is FIXED text, with no request or state values interpolated into it (the identifier is carried in the required resourceId/resourceType members of ResourceNotFoundException, not in the message). Smithy: ResourceNotFoundException httpError 404, members message/resourceId/resourceType all @required; it is a modeled error of GetAppAuthorization and UpdateAppAuthorization.
Emulator: the code is right (ResourceNotFoundException) but the message is "The specified app authorization does not exist" — different fixed text, and missing the trailing period. Observed both for a bogus appAuthorizationIdentifier in a live bundle and for the identifier of a just-deleted authorization, which indicates one shared not-found constructor in the app-authorization lookup rather than two per-operation strings.
Why: AppFabric's error message is a requirement of the error class. Clients and operator tooling that match on or display the message (log scrapers, user-facing "not found" surfaces, and parity tests that assert the documented string) diverge from AWS even though the status and code match.
Fix: the app-authorization not-found refusal built by the shared identifier resolution in src/state.rs and consumed by the GetAppAuthorization / UpdateAppAuthorization / DeleteAppAuthorization handlers — it must emit AWS's fixed text. Check every sibling resource level for the same hand-written wording while fixing it.
Traces: appfabric.get-app-authorization-nonexistent-not-found, appfabric.delete-app-authorization-removes-resource (both AssertionError: expected message 'The app authorization was not found.', got 'The specified app authorization does not exist').

--------------------------------------------------
Cause 3 of 4: DeleteAppBundle on a non-empty bundle is refused as ConflictException instead of ValidationException
--------------------------------------------------
AWS: DeleteAppBundle on a bundle that still contains an app authorization is refused HTTP 400 ValidationException with message exactly "The specified app bundle cannot be deleted." (all fixed text — no request or state values), and the call has no effect: the bundle and its contents survive. Once the contents are deleted the same DeleteAppBundle call succeeds. Smithy: DeleteAppBundle models BOTH ConflictException (409) and ValidationException (400), so the model permits either shape and the recorded behavior is what selects ValidationException here; ValidationException additionally requires a `reason` member.
Emulator: the refusal is classified as 'ConflictException' (HTTP 409), so the status, the code and the error's member set (resourceId/resourceType instead of reason/fieldList) are all wrong. The replay never got as far as checking the message.
Why: a client that distinguishes a retryable/cleanup-needed conflict from a client validation error takes the wrong branch — 409 ConflictException reads as "state will settle, retry", while AWS's 400 ValidationException means "the request can never succeed as written". Any SDK retry policy or teardown loop keyed on that distinction loops or gives up wrongly.
Fix: DeleteAppBundle's handler non-empty-bundle guard — it must raise ValidationException (400, with the required `reason`) carrying AWS's fixed message, not ConflictException.
Traces: appfabric.delete-app-bundle-with-contents-rejected (AssertionError: expected ValidationException, got 'ConflictException').

--------------------------------------------------
Cause 4 of 4: CreateIngestion's unconnected-authorization message says "is not connected" where AWS says "was not connected"
--------------------------------------------------
AWS: CreateIngestion for an app/tenant whose app authorization has not completed the connect handshake is refused HTTP 400 ValidationException with the message "The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion." — entirely fixed text; the app and tenant are NOT interpolated (AWS says "the specified application and tenant" generically). Smithy: CreateIngestion models ValidationException (400); recorded behavior pins the wording.
Emulator: the status and code are right, but the first sentence reads "...and tenant is not connected." — "is" where AWS's fixed text has "was". The second sentence matches.
Why: the message is part of the error-class contract; a client or test matching AWS's documented string fails against the emulator even though it correctly refused the call.
Fix: the unconnected-authorization validation in CreateIngestion's handler — correct the fixed text of the refusal message.
Traces: appfabric.start-ingestion-without-connected-authorization-rejected (AssertionError: expected message '...was not connected. ...', got '...is not connected. ...'). Note this check's id says start-ingestion but the failing call is CreateIngestion.

## Evidence
All five failures come from the behavioral suite replay against the booted emulator (appfabric 19/24, 5 mismatched, 0 errored).

Cause 1 — appfabric.connect-app-authorization-invalid-apikey-transitions-to-validation-failed:
  check_appfabric_connect_app_authorization_invalid_apikey_transitions_to_validation_failed.py:153
  assert final_status == "ConnectionValidationFailed"
  AssertionError: expected ConnectionValidationFailed, got 'Connected'
  Expected: ConnectAppAuthorization with credential {oauth2Credential: bogus clientId/clientSecret} and authRequest {code: bogus, redirectUri: ...} either refuses 400 ValidationException ("The specified oauth2 app authorization configuration is not valid. The error returned by the application is: <provider error text>") leaving the record PendingConnect/ConnectionValidationFailed, or answers 200 and the record converges to ConnectionValidationFailed.
  Actual: 200, and GetAppAuthorization reports status 'Connected' (polled to a non-PendingConnect terminal value).

Cause 2 — appfabric.get-app-authorization-nonexistent-not-found:
  check_...get_app_authorization_nonexistent_not_found.py:128
  AssertionError: expected message 'The app authorization was not found.', got 'The specified app authorization does not exist'
  Request: GetAppAuthorization(appBundleIdentifier=<live bundle arn>, appAuthorizationIdentifier=<fresh uuid4>). Code ResourceNotFoundException matched; message did not.
  Same divergence from the deleted-identifier direction — appfabric.delete-app-authorization-removes-resource, check_...delete_app_authorization_removes_resource.py:123, identical expected/actual strings, after DeleteAppAuthorization succeeded (the check also asserts the same message for UpdateAppAuthorization on the dead id).
  Fixed text per AWS: "The app authorization was not found." (whole string, including the final period). No request/state values belong in it.

Cause 3 — appfabric.delete-app-bundle-with-contents-rejected:
  check_...delete_app_bundle_with_contents_rejected.py:148
  assert code == "ValidationException"
  AssertionError: expected ValidationException, got 'ConflictException'
  Sequence: CreateAppBundle -> CreateAppAuthorization(SLACK, oauth2) -> poll ListAppAuthorizations until the authorization is visible -> DeleteAppBundle(bundle).
  Expected: 400 ValidationException, message "The specified app bundle cannot be deleted." (all fixed text), bundle unaffected; then after DeleteAppAuthorization the same DeleteAppBundle succeeds and GetAppBundle refuses 404 ResourceNotFoundException "The app bundle ID was not found."
  Actual: refused as ConflictException (modeled httpError 409), so the assertion failed before the message/reason could be compared.

Cause 4 — appfabric.start-ingestion-without-connected-authorization-rejected:
  check_...start_ingestion_without_connected_authorization_rejected.py:147
  AssertionError: expected message 'The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion.', got 'The app authorization for the specified application and tenant is not connected. Connect the app authorization for the application and tenant before creating an ingestion.'
  Request: CreateIngestion(appBundleIdentifier=<bundle>, app="SLACK", tenantId=<tenant>, ingestionType="auditLog") while the authorization is PendingConnect (verified by an independent GetAppAuthorization both before and after). Code ValidationException matched; only the fixed text of the first sentence diverges ("is" vs "was").

Model context used (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  ConnectAppAuthorization errors: AccessDenied(403), InternalServer(500), ResourceNotFound(404), Throttling(429), Validation(400); AppAuthorizationStatus = PendingConnect|Connected|ConnectionValidationFailed|TokenAutoRotationFailed.
  DeleteAppBundle errors: AccessDenied(403), Conflict(409), InternalServer(500), Throttling(429), Validation(400); success 204.
  GetAppAuthorization errors include ResourceNotFoundException(404) with required message/resourceId/resourceType.
  CreateIngestion errors include ValidationException(400) with required message/reason.
  Relevant sources: src/ops/connect_app_authorization.rs (unconditional status=Connected write), src/ops/delete_app_bundle.rs, src/ops/create_ingestion.rs, src/ops/get_app_authorization.rs, src/ops/update_app_authorization.rs, src/state.rs (shared identifier resolution / not-found construction).

## Steps to Reproduce
Cause 1 (ConnectAppAuthorization never fails validation):
1. CreateAppBundle.
2. CreateAppAuthorization(app=SLACK, authType=oauth2, credential.oauth2Credential={clientId:"definitely-not-a-real-client-id", clientSecret:"definitely-not-a-real-client-secret"}, tenant={...}); confirm status == PendingConnect.
3. POST /appbundles/{bundle}/appauthorizations/{auth}/connect with authRequest={code:"definitely-not-a-real-oauth-code", redirectUri:"https://us-east-1.console.aws.amazon.com/appfabric/oauth2"}.
4. Poll GetAppAuthorization until status != PendingConnect. Emulator: 'Connected'. AWS: either the connect call itself was refused 400 ValidationException (and status stayed PendingConnect / became ConnectionValidationFailed), or status becomes ConnectionValidationFailed.

Cause 2 (not-found message):
1. CreateAppBundle, CreateAppAuthorization in it.
2. GET /appbundles/{bundle}/appauthorizations/{a-random-uuid}. Observe 404 ResourceNotFoundException with message "The specified app authorization does not exist"; AWS: "The app authorization was not found."
3. Repeat with a real authorization that was just deleted: DeleteAppAuthorization, then GetAppAuthorization and UpdateAppAuthorization on that identifier — same wrong message on both.

Cause 3 (DeleteAppBundle on a non-empty bundle):
1. CreateAppBundle; CreateAppAuthorization(app=SLACK, authType=oauth2) in it.
2. ListAppAuthorizations until the new authorization is visible.
3. DELETE /appbundles/{bundle}. Emulator: 409 ConflictException. AWS: 400 ValidationException, message "The specified app bundle cannot be deleted.", bundle untouched.
4. DeleteAppAuthorization, then DELETE /appbundles/{bundle} again — must now succeed (204), and GetAppBundle must refuse 404 ResourceNotFoundException "The app bundle ID was not found."

Cause 4 (CreateIngestion message):
1. CreateAppBundle; CreateAppAuthorization(app=SLACK, authType=oauth2) and leave it PendingConnect (never connect it).
2. CreateIngestion(app="SLACK", tenantId=<the same tenantIdentifier>, ingestionType="auditLog").
3. Observe 400 ValidationException with "...application and tenant is not connected..."; AWS's fixed text is "...application and tenant was not connected...". Also confirm no ingestion was created and the authorization is still PendingConnect (both hold in the emulator).
