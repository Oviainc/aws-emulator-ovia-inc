Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: grafana.workspace-api-key-lifecycle-round-trip grafana.service-account-token-seconds-to-live-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana.workspace-api-key-lifecycle-round-trip|_poll_grafana_api#11|Timeout|time.sleep(interval_s)
Issue-Key: grafana.service-account-token-seconds-to-live-out-of-range-rejected|run#9|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 7f205ee96657d4151f96c4aa7296372f34590a63

# Grafana: 2 behavioral divergences (workspace native HTTP API unserved for API keys; out-of-range secondsToLive rejected with the wrong error code)

> _Note: this issue was identified against the code base at commit `7f205ee96657d4151f96c4aa7296372f34590a63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkspaceApiKey

## Description
Two distinct, net-new root causes in the Grafana emulator, both grounded in failing behavioral checks. They are independent and must BOTH be closed before this issue is submitted.

Cause 1 of 2: the workspace's own (native) Grafana HTTP API is not served, so a key minted by CreateWorkspaceApiKey is unusable and DeleteWorkspaceApiKey revocation cannot be observed at the destination.
Failing check ids: grafana.workspace-api-key-lifecycle-round-trip
  AWS: Smithy (services/service-com-amazonaws-grafana-awsgrafanacontrolplane/smithy-model) models CreateWorkspaceApiKey as POST /workspaces/{workspaceId}/apikeys returning 200 with required members keyName, workspaceId and key (ApiKeyToken, @sensitive), and DeleteWorkspaceApiKey as DELETE /workspaces/{workspaceId}/apikeys/{keyName} returning 200 with keyName+workspaceId and modelling ResourceNotFoundException (HTTP 404). The model describes only the control plane; the data-plane contract is recorded behavior only: the recording shows the `endpoint` reported on an ACTIVE workspace by DescribeWorkspace serving the native Grafana HTTP API, where `GET <endpoint>/api/org` with header `Authorization: Bearer <key returned by CreateWorkspaceApiKey>` answers HTTP 200 with a JSON object carrying `id` and `name`; after DeleteWorkspaceApiKey for that keyName, the same request at that endpoint is refused with HTTP 401 or 403. A second DeleteWorkspaceApiKey for the now-removed keyName is then rejected with ResourceNotFoundException (HTTP 404) (recorded message example: "API key <keyName> not found.").
  Emulator: CreateWorkspaceApiKey succeeds and returns a non-empty key, but no request to the workspace `endpoint` ever gets an HTTP answer: the check's poll of `GET <endpoint>/api/org` with the bearer key never observes 200 and keeps retrying on transport errors until the harness kills the check ("Check timed out after 60.0s"). Because nothing serves that endpoint, neither the positive control (the fresh key authenticates) nor the revocation read (the deleted key is refused with 401/403) can be reached, and the trailing second-delete assertion on ResourceNotFoundException is never exercised either. The workspace endpoint is announced by the control plane but is not routed anywhere the client is reached on, so the data plane is effectively absent rather than merely incomplete.
  Why: a client that does the only thing an API key is for — calling the workspace's Grafana HTTP API with it — hangs instead of getting a response. Provisioning code that creates a key, calls /api/org (or any native Grafana route) to verify it, then deletes the key, blocks until its own timeout against the emulator, while the same code works against AWS. Revocation also cannot be verified: the emulator cannot show that a deleted key stopped working, so tests that assert least-privilege/rotation behavior silently cannot run.
  Fix: the workspace data-plane surface for this service — the component that serves the native Grafana HTTP API on the endpoint DescribeWorkspace reports, plus its bearer-token authentication against the API-key records that CreateWorkspaceApiKey persists and DeleteWorkspaceApiKey removes (see the key store reached by services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace_api_key.rs and .../delete_workspace_api_key.rs). At minimum: route the announced endpoint on the address the emulator is reached on, accept `Authorization: Bearer <key>` for live keys, answer 200 for GET /api/org, and answer 401/403 once the key is deleted. Note the related already-filed stub issue for CreateWorkspaceServiceAccountToken (which blocks grafana.grafana-http-api-native-round-trip): that one covers minting service-account tokens, NOT serving the native API, so it does not cover this cause.
  Traces: grafana.workspace-api-key-lifecycle-round-trip (harness timeout at the first destination poll of <endpoint>/api/org).

Cause 2 of 2: an out-of-range secondsToLive on CreateWorkspaceServiceAccountToken is refused with the modeled ValidationException instead of the error code AWS actually returns.
Failing check ids: grafana.service-account-token-seconds-to-live-out-of-range-rejected
  AWS: Smithy: CreateWorkspaceServiceAccountToken.secondsToLive (Integer) @range min 1, max 2592000, @required; the operation's modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400). For this specific constraint violation the recorded behavior does NOT use the modeled ValidationException: recorded behavior only — real AWS answers error code `BadRequestException` (an unmodeled error; HTTP status as recorded for a 4xx client refusal), with message "Invalid request body" as the recorded example, and creates no token (the rejected name does not appear in ListWorkspaceServiceAccountTokens).
  Emulator: the pre-handler modeled-constraint walk refuses secondsToLive=2592001 with error code `ValidationException`. Replay: "AssertionError: expected BadRequestException for secondsToLive=2592001, got 'ValidationException'". The refusal itself is correct (the value is out of range and no token is created); only the error code is wrong.
  Why: clients branch on the error code, not the message. Code written against AWS catches BadRequestException (or treats it as a non-retryable bad-request) for an out-of-range token lifetime; against the emulator it sees ValidationException, so the branch is missed and the request is misclassified — retried, surfaced as an unexpected error, or routed to a different handler. Error-mapping tables generated from real traffic will not match.
  Fix: the validation layer for this operation — CreateWorkspaceServiceAccountToken's secondsToLive range check must produce the BadRequestException code AWS returns for this class rather than falling through to the generic modeled-constraint ValidationException (services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/create_workspace_service_account_token.rs and the shared constraint-validation path that raises Scaffolded::validation). Note that this operation's handler body is still a 501 scaffold stub covered by a separate already-filed issue; that issue is about implementing the operation, while this cause is about the error code of the range refusal that happens BEFORE the handler, and the check fails on the code, not on a 501.
  Traces: grafana.service-account-token-seconds-to-live-out-of-range-rejected (assertion at the secondsToLive=2592001 refusal).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1): grafana 7/19 passed, 12 failed.

Cause 1 — grafana.workspace-api-key-lifecycle-round-trip [mismatch]
  Expected (AWS): GET <workspace endpoint>/api/org with `Authorization: Bearer <CreateWorkspaceApiKey key>` -> HTTP 200, JSON containing `id` and `name`; after DeleteWorkspaceApiKey the same request -> HTTP 401/403; a second DeleteWorkspaceApiKey for the same keyName -> ResourceNotFoundException (404).
  Actual (emulator): CreateWorkspaceApiKey returns a key, then no HTTP response is ever obtained from the announced workspace endpoint; the check's retry loop never sees 200 and the run is terminated: "Error: Check timed out after 60.0s". The revocation read and the second-delete ResourceNotFoundException assertion are never reached.

Cause 2 — grafana.service-account-token-seconds-to-live-out-of-range-rejected [mismatch]
  Request: CreateWorkspaceServiceAccountToken(workspaceId=<ACTIVE workspace>, serviceAccountId=<VIEWER service account>, name=<unique>, secondsToLive=2592001)  # 1 over the modeled @range max 2592000
  Expected (AWS, recorded): error code BadRequestException, message example "Invalid request body", and no token created.
  Actual (emulator): error code ValidationException.
  Replay output: File "check_grafana_service_account_token_seconds_to_live_out_of_range_rejected.py", line 40, in run / assert code == "BadRequestException" / AssertionError: expected BadRequestException for secondsToLive=2592001, got 'ValidationException'

## Steps to Reproduce
Boot the emulator and point a botocore `grafana` client at it.

Cause 1 (grafana.workspace-api-key-lifecycle-round-trip):
1. Create an IAM role trusted by grafana.amazonaws.com, then CreateWorkspace(accountAccessType=CURRENT_ACCOUNT, permissionType=SERVICE_MANAGED, authenticationProviders=[SAML], workspaceRoleArn=<role>).
2. Poll DescribeWorkspace until status == ACTIVE and read `workspace.endpoint`.
3. CreateWorkspaceApiKey(workspaceId=<id>, keyName=<unique>, keyRole="ADMIN", secondsToLive=3600) and keep the returned `key`.
4. Issue `GET <endpoint>/api/org` with header `Authorization: Bearer <key>`.
   Expected: HTTP 200 with a JSON body containing `id` and `name`. Observed: no HTTP answer at all — retries never succeed and the check is killed at the 60s harness budget.
5. (Blocked by step 4) DeleteWorkspaceApiKey(workspaceId=<id>, keyName=<same>), re-issue the request from step 4 and expect HTTP 401/403, then call DeleteWorkspaceApiKey again for the same keyName and expect ResourceNotFoundException (404).

Cause 2 (grafana.service-account-token-seconds-to-live-out-of-range-rejected):
1. Create a workspace and wait for ACTIVE as above.
2. CreateWorkspaceServiceAccount(workspaceId=<id>, name=<unique>, grafanaRole="VIEWER").
3. CreateWorkspaceServiceAccountToken(workspaceId=<id>, serviceAccountId=<id from step 2>, name=<unique>, secondsToLive=2592001).
   Expected: error code BadRequestException (message example "Invalid request body"). Observed: error code ValidationException.
4. ListWorkspaceServiceAccountTokens for that service account and confirm no token with the rejected name exists (the emulator is correct on this part).
