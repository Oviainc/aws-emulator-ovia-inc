Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sso.concurrent-invalid-token-requests-all-rejected sso.cross-operation-consistent-error-code-same-bad-token sso.empty-access-token-unauthorized sso.get-role-credentials-invalid-token-unauthorized sso.injection-like-role-name-and-account-id-rejected sso.invalid-token-error-excludes-credentials-payload sso.jwt-like-plausible-token-still-rejected sso.list-account-roles-invalid-token-unauthorized sso.list-accounts-invalid-token-unauthorized sso.logout-invalid-token-unauthorized sso.logout-repeated-calls-same-invalid-token-stay-rejected sso.malformed-next-token-pagination-invalid-access-unauthorized sso.max-results-boundary-values-invalid-token-unauthorized sso.oversized-access-token-value-rejected sso.unicode-role-name-query-param-rejected-safely sso.whitespace-access-token-unauthorized
Issue-Shape-Id: com.amazonaws.sso#SWBPortalService
Issue-Key: sso.concurrent-invalid-token-requests-all-rejected|run#6|AssertionError|assert code == "UnauthorizedException", (
Issue-Key: sso.cross-operation-consistent-error-code-same-bad-token|run#5|AssertionError|assert code == "UnauthorizedException", (
Issue-Key: sso.empty-access-token-unauthorized|run#3|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.get-role-credentials-invalid-token-unauthorized|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.injection-like-role-name-and-account-id-rejected|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso|GetRoleCredentials|UnknownService
Issue-Key: sso.jwt-like-plausible-token-still-rejected|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.list-account-roles-invalid-token-unauthorized|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.list-accounts-invalid-token-unauthorized|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.logout-invalid-token-unauthorized|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.logout-repeated-calls-same-invalid-token-stay-rejected|run#5|AssertionError|assert first_code == "UnauthorizedException", (
Issue-Key: sso.malformed-next-token-pagination-invalid-access-unauthorized|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.max-results-boundary-values-invalid-token-unauthorized|run#3|AssertionError|assert code == "UnauthorizedException", (
Issue-Key: sso.oversized-access-token-value-rejected|run#4|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Key: sso.unicode-role-name-query-param-rejected-safely|run#4|AssertionError|assert ascii_code == "UnauthorizedException", (
Issue-Key: sso.whitespace-access-token-unauthorized|run#3|AssertionError|assert code == "UnauthorizedException", f"expected UnauthorizedException, got {code!r}"
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# SSO: every operation is unroutable — front door answers UnknownService instead of dispatching to SSO, so the bearer-token gate never returns 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoleCredentials

## Description
Cause 1 of 1: the emulator's front-door service-resolution layer cannot attribute an AWS SSO (SSO Portal) request to the sso service, so all four SSO operations are unreachable and the service's authentication gate is never exercised. All 16 failing sso checks fail with the identical emulator reply, independent of operation, token value, parameter values and concurrency.

AWS: SSO is a restJson1 service whose four bound operations — GetRoleCredentials (GET /federation/credentials), ListAccountRoles (GET /assignment/roles), ListAccounts (GET /assignment/accounts), Logout (POST /logout) — are ALL modeled `optionalAuth: true` with an empty `auth: []` list (Smithy: SWBPortalService, sdkId SSO, protocols [restJson1], operation_count 4). They therefore carry NO SigV4 Authorization header and no credential scope, and because the protocol is restJson1 they carry no X-Amz-Target either. The caller is authenticated solely by the `accessToken` member, which every one of the four inputs binds as `httpHeader: x-amz-sso_bearer_token` (required, sensitive) — Smithy: GetRoleCredentials/ListAccountRoles/ListAccounts/Logout input member `accessToken` -> AccessTokenType. A request bearing a token that is not a live session is rejected with `UnauthorizedException`, which is a service-common error bound to all four operations and carries httpError 401 (Smithy: com.amazonaws.sso#UnauthorizedException, errorKind client, httpError 401, doc "Indicates that the request is not authorized. This can happen due to an invalid access token in the request."). So the requirement is: HTTP 401 with error code `UnauthorizedException`. The recorded message for this family is "Session token not found or invalid" — that is an EXAMPLE, not a requirement, since neither the model nor AWS documents how it is built.

Emulator: no SSO operation is dispatched at all. Every call to all four operations returns error code `UnknownService` with the message "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)". The message states the cause: the router resolves a target service only from a SigV4 credential scope, an X-Amz-Target header, or a Host header, and an SSO request legitimately presents none of the first two (anonymous auth + restJson1), while the Host header at the emulator's own endpoint does not identify the service. `UnknownService` is not a shape in the SSO model and is not a member of the common error set, so the failure is in the front door, not in an SSO handler. Hypothesis: the SSO handler and its bearer-token gate may be implemented but simply never reached; the replay cannot distinguish, because no request arrives at them.

Why: a client relying on AWS's behavior gets a nonsense error for every SSO call. botocore cannot map `UnknownService` to any modeled SSO error, so credential-provider code that catches `SSO.Client.exceptions.UnauthorizedException` to detect an expired/invalid SSO session — the normal path that triggers a re-login — never matches and the exception escapes as a bare ClientError. Code branching on a 401 to refresh a token sees neither the status nor the code. Since this affects GetRoleCredentials, ListAccountRoles, ListAccounts and Logout alike, the SSO service is effectively absent: not one success path and not one error path behaves as AWS does. It also silently hides whether the token gate is correct — including the security-relevant guarantees the checks assert: that the gate is stateless and per-request so no concurrent call slips through authenticated (sso.concurrent-invalid-token-requests-all-rejected), that the same bad token yields the identical code across all four operations (sso.cross-operation-consistent-error-code-same-bad-token), and that a rejected GetRoleCredentials body never carries a fabricated roleCredentials payload (sso.invalid-token-error-excludes-credentials-payload).

Fix: the front-door request-routing / service-resolution component, which must resolve an anonymous restJson1 service from its request signature (the HTTP method + path of the four modeled http bindings, and/or the presence of the `x-amz-sso_bearer_token` header) rather than requiring a SigV4 credential scope, an X-Amz-Target or a service-identifying Host header. Once requests reach the service, the SSO operations' authentication gate and error serializer must reject an unknown bearer token with HTTP 401 and error code `UnauthorizedException`, consistently for all four operations and with no credentials payload in the error body.

Traces: all 16 sso checks listed in trace_ids; representative sso.get-role-credentials-invalid-token-unauthorized ("expected UnauthorizedException, got 'UnknownService'").

## Evidence
Expected (AWS / Smithy model): HTTP 401, Error.Code = "UnauthorizedException" (com.amazonaws.sso#UnauthorizedException, errorKind client, httpError 401; a service-common error on all four SSO operations), for a request whose x-amz-sso_bearer_token header is not a valid session. Recorded example message: "Session token not found or invalid" (example only).

Actual (emulator, every one of the 16 sso checks): Error.Code = "UnknownService", Error.Message = "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)".

Per-check assertion text from the replay:
- sso.get-role-credentials-invalid-token-unauthorized: AssertionError: expected UnauthorizedException, got 'UnknownService'
- sso.list-accounts-invalid-token-unauthorized: AssertionError: expected UnauthorizedException, got 'UnknownService'
- sso.list-account-roles-invalid-token-unauthorized: AssertionError: expected UnauthorizedException, got 'UnknownService'
- sso.logout-invalid-token-unauthorized: AssertionError: expected UnauthorizedException, got 'UnknownService'
- sso.empty-access-token-unauthorized / sso.whitespace-access-token-unauthorized / sso.oversized-access-token-value-rejected / sso.jwt-like-plausible-token-still-rejected / sso.injection-like-role-name-and-account-id-rejected / sso.invalid-token-error-excludes-credentials-payload / sso.malformed-next-token-pagination-invalid-access-unauthorized: AssertionError: expected UnauthorizedException, got 'UnknownService'
- sso.max-results-boundary-values-invalid-token-unauthorized: AssertionError: maxResults=1: expected UnauthorizedException, got 'UnknownService'
- sso.unicode-role-name-query-param-rejected-safely: AssertionError: ASCII control: expected UnauthorizedException, got 'UnknownService'
- sso.logout-repeated-calls-same-invalid-token-stay-rejected: AssertionError: first call: expected UnauthorizedException, got 'UnknownService'
- sso.cross-operation-consistent-error-code-same-bad-token: AssertionError: GetRoleCredentials: expected UnauthorizedException, got 'UnknownService' (errors={'GetRoleCredentials': ('UnknownService', ...), 'ListAccountRoles': ('UnknownService', ...), 'ListAccounts': ('UnknownService', ...), 'Logout': ('UnknownService', ...)}) — all four operations identical
- sso.concurrent-invalid-token-requests-all-rejected: AssertionError: expected all 10 concurrent calls rejected with UnauthorizedException, got 10x ('UnknownService', 'could not resolve a target service from the request ...')

Suite totals: sso 0/16 passed (0%), 16 mismatched, 0 errored, 0 skipped. The uniformity across operations, HTTP methods (GET and POST), token shapes and parameter values shows the divergence is in service resolution at the front door, not in any per-operation logic.

## Steps to Reproduce
1. Boot the emulator and point a botocore/boto3 `sso` client at it.
2. Call any SSO operation with an arbitrary bearer token that is not a live session, e.g.:
     sso.get_role_credentials(roleName="AdministratorAccess", accountId="111122223333", accessToken="sim2real-bogus-tok")
   Note the SDK sends GET /federation/credentials?role_name=...&account_id=... with header x-amz-sso_bearer_token and NO SigV4 Authorization header (the operation is optionalAuth with auth: []).
3. Observe the ClientError: Error.Code is "UnknownService" with message "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)". AWS returns HTTP 401 with Error.Code "UnauthorizedException".
4. Repeat with sso.list_accounts(accessToken=...), sso.list_account_roles(accountId="111122223333", accessToken=...) and sso.logout(accessToken=...) — all four return the same UnknownService error, confirming no SSO request is routed to the service.
5. Replay the behavioral checks in trace_ids to confirm a fix; all 16 must reach the SSO handler and receive 401 / UnauthorizedException, including the concurrency check (10 parallel GetRoleCredentials calls must ALL be rejected) and sso.invalid-token-error-excludes-credentials-payload (the raw error body must contain no roleCredentials member).
