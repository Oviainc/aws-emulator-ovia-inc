Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: support-app.create-channel-config-malformed-role-arn-rejected support-app.register-slack-workspace-malformed-team-id-invalid support-app.get-account-alias-access-denied-for-restricted-role
Issue-Shape-Id: com.amazonaws.supportapp#SupportApp
Issue-Key: support-app.create-channel-config-malformed-role-arn-rejected|run#8|AssertionError|assert code == "AccessDeniedException", f"unexpected code: {code}: {message}"
Issue-Key: support-app.register-slack-workspace-malformed-team-id-invalid|_error#7|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: support-app.get-account-alias-access-denied-for-restricted-role|_error#14|AssertionError|raise AssertionError(f"expected the call to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Commit: dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c

# Support App control plane: malformed channelRoleArn misclassified, unauthorized Slack workspace registration accepted, and no IAM authorization on GetAccountAlias (3 causes)

> _Note: this issue was identified against the code base at commit `dbb62b31e7ded4dc3fd01bf9d9c68db3eae6690c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterSlackWorkspaceForOrganization

## Description
Three distinct root causes in the Support App (sdkId "Support App", restJson1) control plane, found by behavioral replay. All three are over-permissive or mis-classified refusal paths. Numbered below; each must be closed before submitting.

Cause 1 of 3: CreateSlackChannelConfiguration rejects a malformed channelRoleArn as a syntactic ValidationException instead of failing the authorization attempt.
AWS: Smithy: CreateSlackChannelConfiguration.channelRoleArn is required, target roleArn with @length min 31/max 2048 and @pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$; modeled errors AccessDeniedException (httpError 403), ConflictException (409), ServiceQuotaExceededException (402), ValidationException (400). Recorded behavior only for the ordering: real AWS does not surface a pattern violation on this member as a request-validation error — it takes the supplied value into the pass-role/authorization step and refuses with AccessDeniedException, HTTP 403, code "AccessDeniedException" (recorded message example: "Cross-account pass role is not allowed."). The configuration is not created either way.
Emulator: returns HTTP 400 code "ValidationException" with message "1 validation error detected: Value 'not-an-iam-role-arn-<hex>' at 'channelRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:iam::[0-9]{12}:role/(.+)".
Why: a client that distinguishes a caller-input bug (ValidationException, non-retryable, fix the request) from an authorization/pass-role failure (AccessDeniedException, fix the role or the trust policy) branches the wrong way; error-handling and alarming keyed on AccessDeniedException never fire for a bad channelRoleArn.
Fix: the CreateSlackChannelConfiguration handler's validation ordering for channelRoleArn — the roleArn pattern must not be enforced as an up-front request-validation rejection; the value must flow into the role-authorization check that answers AccessDeniedException/403.
Traces: support-app.create-channel-config-malformed-role-arn-rejected.

Cause 2 of 3: RegisterSlackWorkspaceForOrganization registers any syntactically valid teamId instead of requiring a previously authorized Slack workspace.
AWS: Smithy: RegisterSlackWorkspaceForOrganization.teamId required, @length 1..256, @pattern ^\\S+$; modeled errors AccessDeniedException (403), ConflictException (409), ResourceNotFoundException (404), ValidationException (400). The operation's own documentation states a Slack workspace can only be authorized through the interactive flow in the Amazon Web Services Support Center console, and that this API then authorizes an already-authorized workspace for the organization. A syntactically valid but never-authorized teamId is therefore refused with code "ResourceNotFoundException" (modeled httpError 404); only the whitespace/pattern violation path yields ValidationException.
Emulator: the whitespace-only teamId is correctly rejected with ValidationException, but a syntactically valid, never-authorized teamId SUCCEEDS: HTTP 200 with body {"accountType": "management", "teamId": "T..."}. No authorization state is consulted, so every teamId is registrable.
Why: the operation becomes unconditionally successful, so a client can never observe the not-found path and the whole "authorize in the console, then register" precondition disappears; code that handles ResourceNotFoundException to tell the user to authorize the workspace is unreachable, and downstream state is populated with workspaces that do not exist.
Fix: the RegisterSlackWorkspaceForOrganization handler and its backing state — registration must look up an authorized-workspace record and answer ResourceNotFoundException/404 when there is none, keeping the existing syntactic validation ahead of the lookup.
Traces: support-app.register-slack-workspace-malformed-team-id-invalid.

Cause 3 of 3: no IAM authorization is applied to the Support App control plane — a session for a role with no identity-based policy is served normally.
AWS: recorded behavior only (GetAccountAlias models no AccessDeniedException; its only modeled error is the service-common InternalServerException). A principal assuming an IAM role whose only policy is the AssumeRole trust policy is refused by IAM implicit deny: HTTP 403, code "AccessDeniedException", with a message built from the request and the caller identity, naming the assumed-role principal, the action supportapp:GetAccountAlias and the resource arn:aws:supportapp:<region>:<account>:/control/get-account-alias (recorded example: "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: supportapp:GetAccountAlias on resource: arn:aws:supportapp:<region>:<account>:/control/get-account-alias because no identity-based policy allows the supportapp:GetAccountAlias action"). The same call with the caller's own permitted credentials succeeds.
Emulator: the restricted session's GetAccountAlias returns HTTP 200 (empty JSON body) — the call is never refused, so the permitted and the unpermitted principal are indistinguishable.
Why: any test or guardrail that asserts a least-privilege role CANNOT read the account alias passes against the emulator and fails in AWS; permission regressions in a customer's policies are invisible, and the emulator cannot be used to validate Support App IAM policy scoping at all.
Fix: the request authorization layer in front of the Support App operation handlers (the GetAccountAlias handler's auth gate), which must evaluate the signing principal's identity-based policy and answer AccessDeniedException/403 on implicit deny.
Traces: support-app.get-account-alias-access-denied-for-restricted-role.

Note on scope: the other failing support-app checks in this run are blocked by the unimplemented ListSlackWorkspaceConfigurations and DeleteSlackWorkspaceConfiguration scaffold stubs and are covered by their own already-filed issues; they are not part of this issue.

## Evidence
Cause 1 — support-app.create-channel-config-malformed-role-arn-rejected (mismatch):
  expected: code "AccessDeniedException" (HTTP 403), message example "Cross-account pass role is not allowed."
  actual:   code "ValidationException" (HTTP 400), "1 validation error detected: Value 'not-an-iam-role-arn-ffbb4b42f3d4' at 'channelRoleArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws:iam::[0-9]{12}:role/(.+)"
  AssertionError: unexpected code: ValidationException: ...

Cause 2 — support-app.register-slack-workspace-malformed-team-id-invalid (mismatch):
  expected (positive control, syntactically valid but never-authorized teamId): code "ResourceNotFoundException" (modeled httpError 404)
  actual:   HTTP 200, {'accountType': 'management', 'teamId': 'T3790D96EA9'}
  AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {... 'accountType': 'management', 'teamId': 'T3790D96EA9'}
  (the whitespace-only teamId ValidationException assertion ahead of it passed)

Cause 3 — support-app.get-account-alias-access-denied-for-restricted-role (mismatch):
  expected: code "AccessDeniedException" (HTTP 403) for a session assumed from a role with only a trust policy
  actual:   HTTP 200, body {} (content-length 2)
  AssertionError: expected the call to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Model evidence (services/service-com-amazonaws-supportapp-supportapp/smithy-model/com-amazonaws-supportapp-supportapp.json):
  CreateSlackChannelConfiguration.channelRoleArn: required, length 31..2048, pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$; errors AccessDeniedException/403, ValidationException/400
  RegisterSlackWorkspaceForOrganization.teamId: required, length 1..256, pattern ^\\S+$; errors ResourceNotFoundException/404, ValidationException/400, ConflictException/409, AccessDeniedException/403
  GetAccountAlias: no input members, output accountAlias; only InternalServerException modeled

## Steps to Reproduce
Cause 1:
1. supportapp CreateSlackChannelConfiguration with a fresh teamId and channelId, notifyOnCaseSeverity="none", and channelRoleArn="not-an-iam-role-arn-<random>" (violates ^arn:aws:iam::[0-9]{12}:role/(.+)$).
2. Observe the error code: AWS answers AccessDeniedException/403; the emulator answers ValidationException/400.
3. ListSlackChannelConfigurations and confirm no configuration was created (true in both).

Cause 2:
1. supportapp RegisterSlackWorkspaceForOrganization with teamId="   " -> ValidationException (passes today).
2. Call it again with a syntactically valid, never-authorized teamId such as "T<10 hex uppercase>".
3. AWS answers ResourceNotFoundException/404; the emulator returns HTTP 200 with {"accountType":"management","teamId":...}.

Cause 3:
1. iam CreateRole with only an sts:AssumeRole trust policy for the account root and NO identity-based policy.
2. sts AssumeRole into it and build a supportapp client from the returned session credentials.
3. Call GetAccountAlias with those credentials. AWS answers AccessDeniedException/403; the emulator returns HTTP 200.
4. Call GetAccountAlias with the caller's own credentials — succeeds in both.
