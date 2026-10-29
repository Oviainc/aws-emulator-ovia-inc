Issue-Source: trace-sweep-divergence
Issue-Trace: appfabric-app-authorization-connect-apikey
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appfabric-app-authorization-connect-apikey appfabric-ingestion-and-destination-error-paths appfabric-app-authorization-oauth2-lifecycle appfabric-resource-not-found-and-conflict-errors
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|CreateAppAuthorization|2xx->4xx|$.appAuthorization
Issue-Key: appfabric|CreateAppAuthorization|2xx->4xx|status
Issue-Key: appfabric|CreateIngestion|4xx->4xx|$.message
Issue-Key: appfabric|CreateAppAuthorization|4xx->4xx|$.message
Issue-Key: appfabric|CreateAppAuthorization|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appfabric|CreateAppAuthorization|4xx->4xx|status
Issue-Key: appfabric|GetAppAuthorization|4xx->4xx|$.message
Issue-Key: appfabric|DeleteAppBundle|4xx->4xx|$.message
Issue-Key: appfabric|DeleteAppBundle|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appfabric|DeleteAppBundle|4xx->4xx|status
Issue-Key: appfabric|DeleteAppAuthorization|4xx->2xx|$
Issue-Key: appfabric|DeleteAppAuthorization|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appfabric|DeleteAppAuthorization|4xx->2xx|status
Issue-Commit: 3660361421cece15e150541af9cef0f01b201b94

# AppFabric refusal paths use the wrong error class, status and message across app-authorization, app-bundle and ingestion operations

> _Note: this issue was identified against the code base at commit `3660361421cece15e150541af9cef0f01b201b94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppAuthorization, CreateIngestion, GetAppAuthorization, DeleteAppBundle, DeleteAppAuthorization

## Description
Six independent root causes on AppFabric's refusal paths, all found by replaying the wire corpus against the booted emulator: a supported application is refused as unsupported, two refusals are classified as ConflictException where AWS answers ValidationException, two messages are the emulator's own wording, and one refusal is not raised at all. A fixer must close ALL SIX before submitting.

Related open issue: 20261029T192800Z-appfabric-connect-never-validates-credentials-app-authorization-not.md (behavioral corpus) describes causes 4 and 5 below from the behavioral side, but it carries no wire replay key, so it does not cover these fixture steps; the wording it pins for cause 2 also differs from what the wire recording shows. Close both views together.

--------------------------------------------------
Cause 1 of 6: CreateAppAuthorization refuses PAGERDUTY as an unsupported application
--------------------------------------------------
AWS: CreateAppAuthorization$app targets String255 with no @enum, so the set of accepted applications is the SERVICE's, not the boundary validator's, and AppFabric accepts PagerDuty. Recorded behavior: CreateAppAuthorization with app="PAGERDUTY", authType="apiKey", an apiKeyCredential and a PagerDuty tenant answers HTTP 201 with the modeled appAuthorization (app=PAGERDUTY, authType=apiKey, status=PendingConnect, persona=admin, tenant echoed from the request, createdAt/updatedAt stamped). Smithy: CreateAppAuthorization http 201; app required String255 len 1..255; AuthType enum = [oauth2, apiKey]; ValidationException httpError 400.
Emulator: the call is refused HTTP 400 ValidationException with message "app PAGERDUTY is not supported" (fixed text "app " + the requested app value + " is not supported" - the app name is a request value, the rest is the emulator's own invented template). The cause is a hard-coded allowlist: src/state.rs SUPPORTED_APPS lists only SLACK, ASANA, JIRA, M365, M365AUDITLOGS, ZOOM, ZENDESK, OKTA, GOOGLE, DROPBOX, SMARTSHEET, CISCO, and is_supported_app rejects anything else; create_app_authorization.rs calls it before any other request-body rule.
Why: every PagerDuty flow is unreachable, and the failure cascades - the authorization never exists, so the bundle has no authorization for its tenant and all downstream ingestion / user-access calls in the same session diverge too (this is what blocks appfabric-ingestion-and-destination-error-paths#setup#2 and feeds causes 2 below and the user-access-task issue). More generally the allowlist is a snapshot of one documentation sentence, so any application AWS has added since is refused.
Fix: the supported-application set in src/state.rs (SUPPORTED_APPS / is_supported_app), consumed by src/ops/create_app_authorization.rs and src/ops/create_ingestion.rs - it must accept the applications AppFabric actually supports, PAGERDUTY among them, rather than a frozen twelve-entry list.
Traces: appfabric-app-authorization-connect-apikey#task#2 (expected 201, got 400); the same cluster covers appfabric-ingestion-and-destination-error-paths#setup#2.

--------------------------------------------------
Cause 2 of 6: CreateIngestion reports the unsupported-app refusal instead of the modeled not-connected refusal
--------------------------------------------------
AWS: CreateIngestion for an app/tenant whose app authorization has not completed the connect handshake is refused HTTP 400 ValidationException with the message "The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion." - that whole string is fixed text; the application and tenant are NOT interpolated (AWS says "the specified application and tenant" generically). Smithy: CreateIngestion models ValidationException (400), app is String255 with no @enum.
Emulator: the status (400) and class are right but the message is "app PAGERDUTY is not supported" - the emulator refuses the call on its own application allowlist (cause 1) before it ever evaluates the authorization precondition, so the caller is told the application is invalid instead of being told to connect the authorization.
Why: a client cannot act on the refusal. AWS's message is actionable ("connect the authorization") and the emulator's names a condition that is not true, so retry/remediation logic and any test matching the documented string both go wrong.
Fix: src/ops/create_ingestion.rs - once the allowlist of cause 1 accepts the application, this path must reach the unconnected-authorization guard and emit AWS's fixed text. Verify the precondition ordering (application validity, then authorization existence, then connected state) rather than only widening the allowlist.
Traces: appfabric-ingestion-and-destination-error-paths#task#3 (400/400, message only).

--------------------------------------------------
Cause 3 of 6: a duplicate CreateAppAuthorization is refused as ConflictException (409) instead of ValidationException (400)
--------------------------------------------------
AWS: a second CreateAppAuthorization for a tenant that already has an authorization of that authType is refused HTTP 400 ValidationException with message "An AppAuthorization already exists for this tenant and this authType." - entirely fixed text, with no request or state values interpolated, and x-amzn-errortype naming ValidationException (AWS appends its Coral namespace to the header value; the code itself is what a client reads). Smithy: CreateAppAuthorization models BOTH ConflictException (409) and ValidationException (400), so the model permits either shape and the recorded behavior is what selects ValidationException here; ValidationException additionally carries the required `reason` member. Hypothesis: AWS's own wording implies the uniqueness key is (bundle, tenant, authType) rather than (bundle, app, tenant) - worth confirming while fixing, but the status, code and message above are the requirement.
Emulator: the refusal is ConflictException at HTTP 409 with message "An app authorization already exists for this application and tenant" and a bare x-amzn-errortype of "ConflictException", so the status, the class, the member set (resourceId/resourceType instead of reason/fieldList) and the fixed text are all wrong. Built by src/state.rs app_authorization_already_exists and raised by the duplicate guard in src/ops/create_app_authorization.rs (the (app, tenant) lookup before the insert).
Why: 409 ConflictException reads to an SDK retry policy as "transient, the state will settle, retry", while AWS's 400 ValidationException means "this request can never succeed as written". A caller doing create-if-absent loops or retries therefore loops or gives up wrongly, and nothing matching AWS's message sees it.
Fix: CreateAppAuthorization's duplicate guard in src/ops/create_app_authorization.rs plus the constructor in src/state.rs - raise ValidationException (400, with the required `reason`) carrying AWS's fixed text.
Traces: appfabric-app-authorization-oauth2-lifecycle#task#6 (expected 400 ValidationException, got 409 ConflictException).

--------------------------------------------------
Cause 4 of 6: the app-authorization not-found message is the emulator's own wording
--------------------------------------------------
AWS: naming an app authorization that does not exist in the named bundle is refused HTTP 404 ResourceNotFoundException with message exactly "The app authorization was not found." - the whole string is fixed text including the final period, with no request or state values in it (the identifier travels in the required resourceId/resourceType members, not in the message). Smithy: ResourceNotFoundException httpError 404, members message/resourceId/resourceType all @required; it is a modeled error of GetAppAuthorization, UpdateAppAuthorization and DeleteAppAuthorization.
Emulator: status 404 and class ResourceNotFoundException are right, but the message is "The specified app authorization does not exist" - different fixed text and no trailing period. It comes from one shared constructor in the app-authorization lookup in src/state.rs, so every operation on that chain reports it.
Why: for AppFabric the error message is part of the error-class contract; operator tooling, user-facing "not found" surfaces and parity tests that match AWS's documented string all diverge even though status and code agree.
Fix: the shared app-authorization not-found constructor in src/state.rs, consumed by the Get/Update/Delete app-authorization handlers - emit AWS's fixed text. Audit the sibling resource levels for the same hand-written wording while there.
Traces: appfabric-resource-not-found-and-conflict-errors#task#3 (404/404, message only). Same wording is pinned from the behavioral side by open issue 20261029T192800Z-appfabric-connect-never-validates-credentials-app-authorization-not.md cause 2, which carries no wire key.

--------------------------------------------------
Cause 5 of 6: DeleteAppBundle on a non-empty bundle is refused as ConflictException (409) instead of ValidationException (400)
--------------------------------------------------
AWS: DeleteAppBundle on a bundle that still holds an app authorization is refused HTTP 400 ValidationException with message exactly "The specified app bundle cannot be deleted." (all fixed text, no request or state values), x-amzn-errortype naming ValidationException, and the call has no effect - the bundle and its contents survive and the same call succeeds (204) once the contents are gone. Smithy: DeleteAppBundle success 204, models BOTH ConflictException (409) and ValidationException (400), so the recorded behavior is what selects ValidationException.
Emulator: the refusal is ConflictException at HTTP 409 with message "You must delete all associated app authorizations before you can delete an app bundle." and a bare x-amzn-errortype of "ConflictException" - wrong status, wrong class, wrong member set (resourceId/resourceType instead of the required `reason`) and wrong fixed text.
Why: same retry-semantics inversion as cause 3 - a teardown loop keyed on 409-means-retry spins against a condition that will never clear on its own, and a client distinguishing a client-validation error from a state conflict takes the wrong branch.
Fix: the non-empty-bundle guard in src/ops/delete_app_bundle.rs - raise ValidationException (400, with `reason`) carrying AWS's fixed text instead of ConflictException.
Traces: appfabric-resource-not-found-and-conflict-errors#task#5 (expected 400 ValidationException, got 409 ConflictException). Also pinned behaviorally by open issue 20261029T192800Z-appfabric-connect-never-validates-credentials-app-authorization-not.md cause 3, which carries no wire key.

--------------------------------------------------
Cause 6 of 6: DeleteAppAuthorization on an identifier the bundle does not hold answers 204 instead of 404
--------------------------------------------------
AWS: DeleteAppAuthorization naming an app authorization the bundle does not hold is refused HTTP 404 ResourceNotFoundException with message "The app authorization was not found." (the same fixed text as cause 4) and x-amzn-errortype naming ResourceNotFoundException. Smithy: DeleteAppAuthorization is @idempotent with success 204 AND models ResourceNotFoundException (404); recorded behavior shows AWS does raise it, so @idempotent does not license a silent success here.
Emulator: the call answers HTTP 204 with an empty body and no x-amzn-errortype header. src/ops/delete_app_authorization.rs treats an unresolved target as already-absent, writes an "already absent" audit record and returns Ok (see the handler's own comment reasoning from @idempotent, lines ~193-207).
Why: a client cannot distinguish "I deleted it" from "it was never there". Teardown and reconciliation code that relies on the 404 to detect a double delete, a wrong bundle scope or a typo'd identifier silently believes a non-existent resource was removed - and the step that follows a successful delete in any sequence now takes the wrong branch.
Fix: src/ops/delete_app_authorization.rs - when the identifier resolves to no stored authorization, raise the modeled ResourceNotFoundException (404) carrying the shared not-found message of cause 4, instead of returning the empty 204.
Traces: appfabric-resource-not-found-and-conflict-errors#task#6 (expected 404 ResourceNotFoundException, got 204 empty body).

## Evidence
All six clusters come from replaying trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, 53 steps, 35 passed, 14 mismatched, 4 blocked, score 66%). None of them carried a stub_path or an owned_by owner.

Cause 1 - appfabric-app-authorization-connect-apikey#task#2, CreateAppAuthorization:
  status: expected 201, got 400
  $.appAuthorization: missing (expected the created authorization {app: 'PAGERDUTY', authType: 'apiKey', persona: 'admin', status: 'PendingConnect', tenant: {...}, appBundleArn: ..., createdAt/updatedAt: ...}, got None)
  Emulator body message: "app PAGERDUTY is not supported"
  Cluster also covers appfabric-ingestion-and-destination-error-paths#setup#2 (affected_count 2).
  Source: src/state.rs lines 131-148 (SUPPORTED_APPS, is_supported_app - PAGERDUTY absent); src/ops/create_app_authorization.rs lines 257-262 (the guard), src/state.rs lines 1799-1801 (app_not_supported).

Cause 2 - appfabric-ingestion-and-destination-error-paths#task#3, CreateIngestion, status 400/400:
  $.message: expected 'The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion.'
            got 'app PAGERDUTY is not supported'

Cause 3 - appfabric-app-authorization-oauth2-lifecycle#task#6, CreateAppAuthorization (repeat of task#2's tenant):
  status: expected 400, got 409
  $.message: expected 'An AppAuthorization already exists for this tenant and this authType.'
            got 'An app authorization already exists for this application and tenant'
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.fabricfrontendservice/', got 'ConflictException'
  Source: src/ops/create_app_authorization.rs lines 337-349; src/state.rs lines 1810-1813 (app_authorization_already_exists).

Cause 4 - appfabric-resource-not-found-and-conflict-errors#task#3, GetAppAuthorization, status 404/404:
  $.message: expected 'The app authorization was not found.', got 'The specified app authorization does not exist'

Cause 5 - appfabric-resource-not-found-and-conflict-errors#task#5, DeleteAppBundle:
  status: expected 400, got 409
  $.message: expected 'The specified app bundle cannot be deleted.'
            got 'You must delete all associated app authorizations before you can delete an app bundle.'
  $headers.x-amzn-errortype: expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.fabricfrontendservice/', got 'ConflictException'

Cause 6 - appfabric-resource-not-found-and-conflict-errors#task#6, DeleteAppAuthorization:
  status: expected 404, got 204
  $: empty_body (expected {'message': 'The app authorization was not found.'}, got {})
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:...', got None)
  Source: src/ops/delete_app_authorization.rs lines 193-207 (unresolved target -> audit "already absent" -> Ok(DeleteAppAuthorizationOutput {})).

Model evidence (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  CreateAppAuthorization: http 201 POST /appbundles/{appBundleIdentifier}/appauthorizations; app = String255 required (no @enum); errors AccessDenied(403), Conflict(409), InternalServer(500), ResourceNotFound(404), ServiceQuotaExceeded(402), Throttling(429), Validation(400).
  CreateIngestion: http 201; errors include Validation(400), Conflict(409); no ResourceNotFoundException.
  DeleteAppBundle: http 204; errors AccessDenied(403), Conflict(409), InternalServer(500), Throttling(429), Validation(400).
  DeleteAppAuthorization: @idempotent, http 204; errors include ResourceNotFound(404) and Validation(400).
  GetAppAuthorization: http 200; errors include ResourceNotFound(404); ResourceNotFoundException requires message/resourceId/resourceType.
  AuthType enum = [oauth2, apiKey].

## Steps to Reproduce
Cause 1 (supported app refused):
1. CreateAppBundle.
2. POST /appbundles/{bundle}/appauthorizations with app="PAGERDUTY", authType="apiKey", credential.apiKeyCredential={apiKey: "..."}, tenant={tenantIdentifier: "<something>.pagerduty.com", tenantDisplayName: "..."}.
3. Emulator: 400 ValidationException "app PAGERDUTY is not supported". AWS: 201 with the appAuthorization (status PendingConnect, persona admin).

Cause 2 (CreateIngestion message):
1. With the PagerDuty authorization created but not connected, POST /appbundles/{bundle}/ingestions with app="PAGERDUTY", tenantId=<the tenantIdentifier>, ingestionType="auditLog".
2. Emulator: 400 with "app PAGERDUTY is not supported". AWS: 400 ValidationException "The app authorization for the specified application and tenant was not connected. Connect the app authorization for the application and tenant before creating an ingestion."

Cause 3 (duplicate create):
1. CreateAppBundle; CreateAppAuthorization(app=SLACK, authType=oauth2, tenant=T) - succeeds 201.
2. Repeat the same CreateAppAuthorization for tenant T (no clientToken).
3. Emulator: 409 ConflictException "An app authorization already exists for this application and tenant". AWS: 400 ValidationException "An AppAuthorization already exists for this tenant and this authType.", x-amzn-errortype ValidationException.

Cause 4 (not-found wording):
1. CreateAppBundle; GET /appbundles/{bundle}/appauthorizations/{a-random-uuid}.
2. Emulator: 404 with "The specified app authorization does not exist". AWS: 404 with "The app authorization was not found."

Cause 5 (DeleteAppBundle on a non-empty bundle):
1. CreateAppBundle; CreateAppAuthorization(app=SLACK, authType=oauth2) in it.
2. DELETE /appbundles/{bundle}.
3. Emulator: 409 ConflictException "You must delete all associated app authorizations...". AWS: 400 ValidationException "The specified app bundle cannot be deleted.", bundle untouched.
4. DeleteAppAuthorization, then DELETE /appbundles/{bundle} again - must now answer 204.

Cause 6 (DeleteAppAuthorization on a missing target):
1. CreateAppBundle; DELETE /appbundles/{bundle}/appauthorizations/{a-random-uuid}.
2. Emulator: 204, empty body, no x-amzn-errortype. AWS: 404 ResourceNotFoundException "The app authorization was not found."
3. Also confirm the double-delete direction: create an authorization, delete it (204), delete it again - the second delete must answer 404.

Re-replay appfabric-app-authorization-connect-apikey#task#2, appfabric-ingestion-and-destination-error-paths#task#3, appfabric-app-authorization-oauth2-lifecycle#task#6, and appfabric-resource-not-found-and-conflict-errors#task#3, #task#5 and #task#6 to confirm all six clusters pass.
