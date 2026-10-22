Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso.concurrent-invalid-token-requests-all-rejected sso.cross-operation-consistent-error-code-same-bad-token sso.empty-access-token-unauthorized sso.get-role-credentials-invalid-token-unauthorized sso.injection-like-role-name-and-account-id-rejected sso.invalid-token-error-excludes-credentials-payload sso.jwt-like-plausible-token-still-rejected sso.list-account-roles-invalid-token-unauthorized sso.list-accounts-invalid-token-unauthorized sso.logout-invalid-token-unauthorized sso.logout-repeated-calls-same-invalid-token-stay-rejected sso.malformed-next-token-pagination-invalid-access-unauthorized sso.max-results-boundary-values-invalid-token-unauthorized sso.oversized-access-token-value-rejected sso.unicode-role-name-query-param-rejected-safely sso.whitespace-access-token-unauthorized
Issue-Shape-Id: com.amazonaws.sso#SWBPortalService
Issue-Key: sso.concurrent-invalid-token-requests-all-rejected|run#6|AssertionError|assert message == MESSAGE, f"expected all 10 messages {MESSAGE!r}, got {errors}"
Issue-Key: sso.cross-operation-consistent-error-code-same-bad-token|run#5|AssertionError|assert message == MESSAGE, f"{name}: expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.empty-access-token-unauthorized|run#4|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.get-role-credentials-invalid-token-unauthorized|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.injection-like-role-name-and-account-id-rejected|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso|GetRoleCredentials|UnauthorizedException
Issue-Key: sso.jwt-like-plausible-token-still-rejected|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.list-account-roles-invalid-token-unauthorized|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.list-accounts-invalid-token-unauthorized|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.logout-invalid-token-unauthorized|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.logout-repeated-calls-same-invalid-token-stay-rejected|run#6|AssertionError|assert first_message == MESSAGE, f"first call: expected {MESSAGE!r}, got {first_message!r}"
Issue-Key: sso.malformed-next-token-pagination-invalid-access-unauthorized|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.max-results-boundary-values-invalid-token-unauthorized|run#3|AssertionError|assert message == MESSAGE, f"maxResults={edge}: expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.oversized-access-token-value-rejected|run#5|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Key: sso.unicode-role-name-query-param-rejected-safely|run#5|AssertionError|assert ascii_message == MESSAGE, (
Issue-Key: sso.whitespace-access-token-unauthorized|run#4|AssertionError|assert message == MESSAGE, f"expected {MESSAGE!r}, got {message!r}"
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# sso: UnauthorizedException message for an invalid/unknown bearer token diverges from the constant the SSO Portal API returns (all 4 operations, every rejection path)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccounts, ListAccountRoles, GetRoleCredentials, Logout

## Description
Cause 1 of 1: every invalid-bearer-token rejection in the SSO Portal API carries the wrong UnauthorizedException message. This single root cause fails all 16 sso behavioral checks (the service's entire behavioral surface, 0/16).

AWS: recorded behavior only. The Smithy model (services/service-com-amazonaws-sso-swbportalservice/smithy-model/com-amazonaws-sso-swbportalservice.json) declares UnauthorizedException as a service-common error on all four bound operations with errorKind=client, httpError=401, and a single optional `message` member (target ErrorDescription); its documentation is only "Indicates that the request is not authorized. This can happen due to an invalid access token in the request." — it does not state the text. The recordings do: across all four Portal operations (GetRoleCredentials, ListAccountRoles, ListAccounts, Logout) and across every negative token class the suite exercises (bogus token, empty string, whitespace-only, JWT-shaped, oversized, repeated calls, 10 concurrent calls, and combinations with malformed next_token / max_result boundary values / unicode or injection-like role_name and account_id), real AWS answers HTTP 401 UnauthorizedException with ONE fixed, operation- and input-independent message: "Session token not found or invalid". That operation-independence, pinned by 16 independent recordings, is the rule: the Portal API's authentication gate emits a single constant rejection message and never derives it from the request or the operation.

Emulator: the status (401) and the error code (UnauthorizedException) are correct in every one of the 16 checks — the ONLY divergence is the message. Each operation handler builds its own hand-written string and returns "the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session" instead. e.g. for ListAccounts the handler constructs UnauthorizedException { message: Some("the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session") } on the `state.session(&input.access_token, now)` miss; the other three operations do the same with their own copies of the literal.

Why: a client that reads the Portal API's authentication-failure text — a credential-provider or CLI that surfaces it to the user, a log/alert matcher, or a test that pins the documented rejection — sees a string real AWS never emits, and the emulator additionally leaks an internal wire/header detail (x-amz-sso_bearer_token, "sign-in session") into user-visible output. Because the message is a constant rather than a per-operation or per-input value, the divergence is reproducible on every rejected call, so no unauthenticated code path in this service behaves as it does against AWS.

Fix: the shared invalid-session rejection in this service's operation handlers — the UnauthorizedException construction should come from one place (a single helper on the service's session lookup) emitting the one constant the Portal API returns, instead of four per-handler literals. The four copies are in the operation handlers of the sso crate; the ListAccounts one is cited under Traces.

Traces: all 16 failing checks below. Grounding citation for the emulator side: services/service-com-amazonaws-sso-swbportalservice/src/ops/list_accounts.rs lines 173-182 (the same literal is repeated in the list_account_roles, get_role_credentials and logout handlers). Note that sso.invalid-token-error-excludes-credentials-payload also asserts a raw HTTP 401 and the absence of a roleCredentials member in the error body, but it aborted on the message assertion before reaching those, so those two properties are unverified rather than known-good.

## Evidence
Suite: sso 0/16 (0%), 16 mismatched, 0 errored, 0 skipped. Every failure is the same assertion.

Expected (real AWS, recorded in all 16 checks):
  code    = "UnauthorizedException"
  message = "Session token not found or invalid"
  HTTP    = 401

Actual (emulator):
  code    = "UnauthorizedException"   (matches)
  message = "the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session"   (diverges)

Representative failures:
- sso.cross-operation-consistent-error-code-same-bad-token: "GetRoleCredentials: expected 'Session token not found or invalid', got 'the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session'"
- sso.concurrent-invalid-token-requests-all-rejected: all 10 concurrent GetRoleCredentials calls returned ('UnauthorizedException', 'the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session'); expected all 10 messages 'Session token not found or invalid'
- sso.empty-access-token-unauthorized / sso.whitespace-access-token-unauthorized / sso.oversized-access-token-value-rejected / sso.jwt-like-plausible-token-still-rejected (ListAccounts), sso.list-account-roles-invalid-token-unauthorized, sso.logout-invalid-token-unauthorized, sso.get-role-credentials-invalid-token-unauthorized: identical expected-vs-got pair.
- sso.max-results-boundary-values-invalid-token-unauthorized: fails at maxResults=1 with the same pair, showing the message does not vary with input.

Emulator source (one of four copies), services/service-com-amazonaws-sso-swbportalservice/src/ops/list_accounts.rs:173-182 — on a session-lookup miss it returns UnauthorizedException with the literal message above.

Model: com.amazonaws.sso#UnauthorizedException — errorKind client, httpError 401, members {message: ErrorDescription}; listed in common_errors for all four operations. No message text is modeled, hence "recorded behavior only".

## Steps to Reproduce
1. Boot the emulator and point a botocore `sso` client at it.
2. Call any Portal operation with a token that is not a live session, e.g.:
   sso.list_accounts(accessToken="sim2real-bogus-abc")
   sso.list_account_roles(accessToken="sim2real-bogus-abc", accountId="111122223333")
   sso.get_role_credentials(roleName="AdministratorAccess", accountId="111122223333", accessToken="sim2real-bogus-abc")
   sso.logout(accessToken="sim2real-bogus-abc")
3. Catch ClientError and read err.response["Error"]["Code"] and ["Message"].
4. Observed: Code == "UnauthorizedException" (correct), Message == "the access token presented in x-amz-sso_bearer_token is not a valid IAM Identity Center sign-in session".
   Expected: Message == "Session token not found or invalid" for every operation and every invalid-token class (empty string, "   ", JWT-shaped, oversized, unicode/injection-like role_name or account_id, malformed next_token, max_result at 1 or 100, repeated calls, and 10 concurrent calls).
5. Re-replay the 16 check ids in trace_ids to confirm a fix.
