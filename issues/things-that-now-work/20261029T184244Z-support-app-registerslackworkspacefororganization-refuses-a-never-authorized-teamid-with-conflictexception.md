Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: support-app.register-slack-workspace-malformed-team-id-invalid
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: support-app.register-slack-workspace-malformed-team-id-invalid|run#8|AssertionError|assert code2 == "ResourceNotFoundException", f"unexpected code: {code2}: {message2}"
Issue-Commit: 3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf

# RegisterSlackWorkspaceForOrganization refuses a never-authorized teamId with ConflictException/409 about Organizations membership instead of ResourceNotFoundException/404; the modeled 404 path is unreachable

> _Note: this issue was identified against the code base at commit `3b929b447a52a39dfc36e4a4cbd93099ed5ef8cf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSlackWorkspaceForOrganization

## Description
One root cause in the Support App (sdkId "Support App", restJson1) control plane, found by behavioral replay.

Cause 1 of 1: RegisterSlackWorkspaceForOrganization has no authorized-Slack-workspace lookup, so the not-found refusal AWS gives for a syntactically valid but never-authorized teamId cannot be produced; the emulator instead gates the call on the caller account's Organizations membership and, in a default emulator account (which belongs to no organization), refuses every call with ConflictException/409.

AWS: Smithy (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json): RegisterSlackWorkspaceForOrganization input teamId is required, target teamId with @length 1..256 and @pattern ^\S+$; modeled errors AccessDeniedException (httpError 403), ConflictException (409), ResourceNotFoundException (404), ValidationException (400), InternalServerException (500); http POST /control/register-slack-workspace-for-organization, success 200. ResourceNotFoundException is documented as "The specified resource is missing or doesn't exist, such as an account alias, Slack channel configuration, or Slack workspace configuration", and the operation's own documentation states a Slack workspace is authorized only through the interactive flow in the AWS Support Center console, after which this API authorizes that already-authorized workspace. Recorded behavior for the classification: a syntactically valid teamId that was never authorized through that console flow is refused with HTTP 404, error code "ResourceNotFoundException"; the message is not a requirement pinned by the recording because it names the organization's management account, i.e. it is built from the caller's account topology (fixed text plus an account id read from state), not from any request value. The whitespace-only teamId ("   ") on the same operation is a separate, correctly-classified path: ValidationException/400 with the standard envelope "1 validation error detected: Value '<teamId>' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: \S+" (fixed text; the quoted value is the request's teamId) — that assertion passes today.

Emulator: the same call returns HTTP 409, error code "ConflictException", message "Account 000000000000 doesn't belong to an organization in Organizations. To call the RegisterSlackWorkspaceForOrganization API, your account must be part of an organization in Organizations." (fixed text, with the caller account id read from state and the operation name interpolated). The membership gate at src/ops/register_slack_workspace_for_organization.rs:224-240 runs before this service's registry is ever consulted, and because the emulator's default account is in no organization it short-circuits every call. Even when an organization does hold the account, the handler still never answers 404: with a management-account caller any teamId is REGISTERED (HTTP 200 with teamId/teamName/accountType), and with a member-account caller an unregistered teamId is refused with the second ConflictException (:264-278). No code path in the operation constructs ResourceNotFoundException, so the modeled 404 is unreachable for every account topology.

Why: a client branches on the error class. ResourceNotFoundException is the signal that tells the user "authorize this Slack workspace in the Support Center console first" — it is non-retryable and actionable by a human; ConflictException/409 reads as "your account topology is wrong (join an organization)" and is what SDK and user code treat as a conflicting-state problem. Against the emulator the actionable not-found path is never observed, and in a default (no-organization) emulator account the operation is unusable altogether: callers see a membership conflict for a request whose actual defect is an unauthorized workspace.

Related open issue (does NOT cover this): 20261022T200620Z-support-app-control-plane-malformed-channelrolearn-misclassified-unauthorized.md, whose Cause 2 recorded this same check failing because the call SUCCEEDED with HTTP 200. The call is now refused, but with the wrong error class and status, so the replay key differs and the suite did not report this failure as owned.

Fix: the RegisterSlackWorkspaceForOrganization handler and its backing state — it needs an authorized-workspace lookup that answers ResourceNotFoundException/404 for a teamId no console authorization recorded, ordered after the existing syntactic validation and ahead of (or independent of) the Organizations-membership conflict, so the membership gate cannot mask the not-found path; and the emulator's default account state must not make that gate refuse every call.

Traces: support-app.register-slack-workspace-malformed-team-id-invalid.

## Evidence
Cause 1 — support-app.register-slack-workspace-malformed-team-id-invalid (mismatch):
  step 1 (passes today): RegisterSlackWorkspaceForOrganization teamId="   " -> ValidationException/400, "1 validation error detected: Value '   ' at 'teamId' failed to satisfy constraint: Member must satisfy regular expression pattern: \\S+"
  step 2 (positive control, fails): RegisterSlackWorkspaceForOrganization with a syntactically valid, never-authorized teamId (e.g. "T<10 uppercase hex>")
    expected: error code "ResourceNotFoundException" (modeled httpError 404); message varies (it names the organization's management account)
    actual:   error code "ConflictException" (409), message "Account 000000000000 doesn't belong to an organization in Organizations. To call the RegisterSlackWorkspaceForOrganization API, your account must be part of an organization in Organizations."
  AssertionError: unexpected code: ConflictException: Account 000000000000 doesn't belong to an organization in Organizations. To call the RegisterSlackWorkspaceForOrganization API, your account must be part of an organization in Organizations.

Model evidence (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json):
  RegisterSlackWorkspaceForOrganization: input teamId required, len 1..256, pattern ^\\S+$; errors AccessDeniedException/403, ConflictException/409, ResourceNotFoundException/404, ValidationException/400, InternalServerException/500; POST /control/register-slack-workspace-for-organization, success 200.

Code evidence (services/service-com-amazonaws-supportapp-supportapp/src/ops/register_slack_workspace_for_organization.rs):
  :218-240 the Organizations-membership gate runs first and returns ConflictException via crate::state::account_not_in_organization_message when crate::state::caller_account_type yields None — the default emulator account.
  :246-278 the only other refusal is a second ConflictException, for a member-account caller and an unregistered teamId.
  :280-328 a management-account caller registers ANY teamId and returns 200.
  The enum at :150-156 declares ResourceNotFoundException, but no code path in handle() constructs it.

## Steps to Reproduce
1. Point a botocore client at the emulator's support-app endpoint with default credentials (default account, which belongs to no organization in the emulator's Organizations store).
2. Call RegisterSlackWorkspaceForOrganization with teamId="   " -> ValidationException/400 (matches AWS; this step passes).
3. Call RegisterSlackWorkspaceForOrganization with a syntactically valid, never-authorized teamId such as "T3790D96EA9".
4. AWS answers HTTP 404 with error code "ResourceNotFoundException"; the emulator answers HTTP 409 with error code "ConflictException" and the "doesn't belong to an organization in Organizations" message.
5. Optionally make the caller account the management account of an organization and repeat step 3: the emulator now returns HTTP 200 and registers the workspace, so the modeled 404 is never produced in any topology.
