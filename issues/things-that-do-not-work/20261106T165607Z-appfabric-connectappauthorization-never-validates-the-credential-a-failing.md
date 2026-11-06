Issue-Source: trace-sweep-divergence
Issue-Trace: appfabric-app-authorization-connect-apikey
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appfabric-app-authorization-connect-apikey
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric|ConnectAppAuthorization|4xx->2xx|$.message
Issue-Key: appfabric|ConnectAppAuthorization|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: appfabric|ConnectAppAuthorization|4xx->2xx|status
Issue-Key: appfabric|GetAppAuthorization|2xx->2xx|$.appAuthorization.status
Issue-Related: 20261029T200017Z-appfabric-date-time-members-serialize-as-epoch-numbers.md
Issue-Commit: 73f8eaecfe0448dfa40697a75965cd99973ceee3

# AppFabric ConnectAppAuthorization never validates the credential: a failing connect answers 200 and the authorization reports Connected

> _Note: this issue was identified against the code base at commit `73f8eaecfe0448dfa40697a75965cd99973ceee3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ConnectAppAuthorization

## Description
Two divergence clusters from the wire replay of trace set simparity-wire-aws-v1 (emulator_up=true, 53 steps, 44 passed, 9 mismatched, score 83%) that share one root cause: the emulator's ConnectAppAuthorization treats every connect request as successful. It never models the credential-verification step AppFabric performs against the target application, so (a) the modeled ValidationException failure path is unreachable and the call answers 200, and (b) the authorization's status is driven straight to Connected, so the modeled AppAuthorizationStatus value ConnectionValidationFailed is never reachable and a later read-back misreports the authorization as usable. Both causes are listed separately because each is its own fixture step that must be re-replayed before this is closed; one fix in the connect path is expected to close both. A fixer must confirm BOTH.

--------------------------------------------------
Cause 1 of 2: ConnectAppAuthorization answers 200 success where AWS refuses the unverifiable credential with ValidationException
--------------------------------------------------
AWS: ConnectAppAuthorization is POST /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}/connect with http code 200 and models ValidationException (errorKind client, httpError 400) among its errors; AppAuthorizationStatus documents ConnectionValidationFailed as "The app authorization received a validation exception when trying to connect to the application. If the app authorization is in this state, you should verify the configured credentials and try to connect the app authorization again." (Smithy: ConnectAppAuthorization errors = AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400; AppAuthorizationStatus values = PendingConnect, Connected, ConnectionValidationFailed, TokenAutoRotationFailed; ValidationException.message @required). So when the supplied credential cannot be verified against the application, the connect call is REFUSED: HTTP 400, error code ValidationException, carried on the restJson1 error header x-amzn-errortype whose recorded form is "ValidationException:http://internal.amazon.com/coral/com.amazon.fabricfrontendservice/", with a JSON body whose only member is "message". Recorded behavior for the message (the model does not state its text): the fixed text is "Could not verify token with application API: " ... ". The request failed with the status code " ... ": The application error is: status code: " ... ", reason phrase: " ...; the variable parts are the application's verification endpoint URL for the app being authorized, and the HTTP status code and reason phrase the application returned for the rejected credential. One recording of it reads "Could not verify token with application API: https://api.pagerduty.com/users?limit=1. The request failed with the status code 401: The application error is: status code: 401, reason phrase: Unauthorized" - the host, path and 401/Unauthorized there are values of that run's app and credential, not part of the template.
Emulator: the step returns HTTP 200 with a success ConnectAppAuthorizationResponse. The replay reports "status: expected 400, got 200", "$.message: missing (expected 'Could not verify token with application API: ... reason phrase: Unauthorized', got None)" and "$headers.x-amzn-errortype: missing (expected 'ValidationException:...', got None)". No error is produced at all - the connect path has no failure branch.
Why: a client that connects an app authorization with a bad or revoked API key is told the connection succeeded. It never sees the ValidationException a real SDK call raises, so it skips the "verify the configured credentials and try to connect again" remediation AWS documents and proceeds to ingest from an authorization that cannot authenticate. Error handling around ConnectAppAuthorization cannot be exercised against the emulator at all, since the operation has no reachable 4xx for an unusable credential.
Fix: the ConnectAppAuthorization handler (its validation/connect branch) must model credential verification and emit ValidationException with httpError 400, the x-amzn-errortype header and a message built from the template above instead of unconditionally succeeding. Hypothesis: since the emulator cannot reach a real application API, the failure must be driven deterministically from the request's credential (the fixture connects an apiKey credential that the recording shows the application rejected), not from a live call.
Traces: appfabric-app-authorization-connect-apikey#task#3.

--------------------------------------------------
Cause 2 of 2: after a failed connect, GetAppAuthorization reports status Connected instead of ConnectionValidationFailed
--------------------------------------------------
AWS: GetAppAuthorization (GET /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}, http 200) returns AppAuthorization, whose status member is @required and targets AppAuthorizationStatus. Per that member's documentation an authorization whose connect attempt hit a validation exception is left in ConnectionValidationFailed; Connected means "The app authorization is connected to the application, and is ready to be used". So the read-back after the refused connect of cause 1 must report ConnectionValidationFailed.
Emulator: the same read-back reports $.appAuthorization.status = 'Connected' where 'ConnectionValidationFailed' is expected (status 200/200, so only the member value diverges).
Why: the authorization's state is a lie about the service's own readiness - a client polling status to decide whether ingestion can start sees "ready to be used" for an authorization whose credential was rejected, and the ConnectionValidationFailed state (and any client logic branching on it) can never be observed on the emulator.
Fix: the connect path's state transition (AppAuthorizationRecord's status field in the appfabric crate's state layer, written by the ConnectAppAuthorization handler) must set ConnectionValidationFailed when verification fails, rather than setting Connected for every connect; GetAppAuthorization itself needs no change beyond returning the stored value.
Traces: appfabric-app-authorization-connect-apikey#task#4.

Note: cause 2's cluster also reports $.appAuthorization.createdAt / $.appAuthorization.updatedAt as epoch numbers. That encoding defect is NOT part of this issue - it is already owned by the open issue 20261029T200017Z-appfabric-date-time-members-serialize-as-epoch-numbers.md. The requirement filed here is only the status member. The other open appfabric issues (listtagsforresource ARN resourceType, untagresource empty tagKeys, duplicate-create conflict message, CreateAppAuthorization unsupported app name) concern different operations/paths and do not cover either cause.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up=true, "/__admin/health answered 200 and the data listener accepts"; steps_total=53, steps_passed=44, steps_mismatched=9, steps_blocked=0, score_pct=83.0, divergence_count=7). Five of the seven clusters carry owned_by=20261029T200017Z-appfabric-date-time-members-serialize-as-epoch-numbers.md and are skipped; the two below carry empty owned_by and empty stub_path.

Cause 1 - fixture appfabric-app-authorization-connect-apikey, step task#3, operation ConnectAppAuthorization, verdict mismatch, expected_status 400, actual_status 200:
  status: expected 400, got 200
  $.message: missing (expected 'Could not verify token with application API: https://api.pagerduty.com/users?limit=1. The request failed with the status code 401: The application error is: status code: 401, reason phrase: Unauthorized', got None)
  $headers.x-amzn-errortype: missing (expected 'ValidationException:http://internal.amazon.com/coral/com.amazon.fabricfrontendservice/', got None)
  divergence_key: service=appfabric|ConnectAppAuthorization|status=400->200|keys=$.message,$headers.x-amzn-errortype,status
  (the URL, 401 and 'Unauthorized' in that message are the recording's values for that app and credential; the fixed text is the requirement.)

Cause 2 - fixture appfabric-app-authorization-connect-apikey, step task#4, operation GetAppAuthorization, verdict mismatch, expected_status 200, actual_status 200:
  $.appAuthorization.status: value_mismatch (expected 'ConnectionValidationFailed', got 'Connected')
  ($.appAuthorization.createdAt / updatedAt also diverge as epoch numbers - owned by the existing timestamp issue, not filed here.)
  divergence_key: service=appfabric|GetAppAuthorization|status=200->200|keys=$.appAuthorization.createdAt,$.appAuthorization.status,$.appAuthorization.updatedAt

Model evidence (services/service-com-amazonaws-appfabric-fabricfrontendservice/smithy-model/com-amazonaws-appfabric-fabricfrontendservice.json):
  ConnectAppAuthorization: POST /appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}/connect, http code 200, errors include ValidationException (client, httpError 400); output appAuthorizationSummary @required.
  ValidationException: error structure, httpError 400, member message @required.
  AppAuthorizationStatus: enum [PendingConnect, Connected, ConnectionValidationFailed, TokenAutoRotationFailed].
  AppAuthorization.status: target AppAuthorizationStatus, @required, documented as ConnectionValidationFailed when the connect attempt received a validation exception.

## Steps to Reproduce
1. Boot the emulator and create an app bundle (POST /appbundles).
2. POST /appbundles/{bundle}/appauthorizations with authType="apiKey" and a credential whose API key the target application would reject, plus a tenant. The authorization is created in PendingConnect.
3. POST /appbundles/{bundle}/appauthorizations/{auth}/connect with the matching authRequest. The emulator answers HTTP 200 with a success appAuthorizationSummary. AWS answers HTTP 400 with header x-amzn-errortype: ValidationException:http://internal.amazon.com/coral/com.amazon.fabricfrontendservice/ and a body {"message": "Could not verify token with application API: <application verification URL>. The request failed with the status code <status>: The application error is: status code: <status>, reason phrase: <reason phrase>"} (cause 1).
4. GET /appbundles/{bundle}/appauthorizations/{auth}. The emulator reports appAuthorization.status = "Connected"; AWS reports "ConnectionValidationFailed" because the connect attempt failed credential validation (cause 2).
5. Re-replay fixture appfabric-app-authorization-connect-apikey steps task#3 and task#4 to confirm both clusters pass.
