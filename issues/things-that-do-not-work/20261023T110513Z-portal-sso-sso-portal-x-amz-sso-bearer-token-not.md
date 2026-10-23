Issue-Source: trace-sweep-divergence
Issue-Trace: sso-credential-vending-workflow
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-credential-vending-workflow sso-list-accounts-pagination-bounds sso-list-account-roles-unknown-account sso-get-role-credentials-errors sso-logout-repeated-and-malformed
Issue-Shape-Id: com.amazonaws.sso#SWBPortalService
Issue-Key: portal.sso|ListAccounts|4xx->4xx|$.__type
Issue-Key: portal.sso|ListAccounts|4xx->4xx|$.message
Issue-Key: portal.sso|ListAccounts|4xx->4xx|status
Issue-Key: portal.sso|ListAccountRoles|4xx->4xx|$.__type
Issue-Key: portal.sso|ListAccountRoles|4xx->4xx|$.message
Issue-Key: portal.sso|ListAccountRoles|4xx->4xx|status
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|$.__type
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|$.message
Issue-Key: portal.sso|GetRoleCredentials|4xx->4xx|status
Issue-Key: portal.sso|Logout|4xx->4xx|$.__type
Issue-Key: portal.sso|Logout|4xx->4xx|$.message
Issue-Key: portal.sso|Logout|4xx->4xx|status
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# SSO portal: x-amz-sso_bearer_token not honored — invalid access tokens answer 400 InvalidRequestException instead of 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAccounts

## Description
All four operations of the SSO portal service (sdkId SSO, endpointPrefix portal.sso, restJson1) take their bearer credential from the HTTP header binding `x-amz-sso_bearer_token` (Smithy: member `accessToken` -> AccessTokenType, @httpHeader("x-amz-sso_bearer_token"), @required, @sensitive, on ListAccountsRequest / ListAccountRolesRequest / GetRoleCredentialsRequest / LogoutRequest). The emulator never reads that header: on every operation it concludes `accessToken` is absent and emits the generic required-member validation failure, so a request carrying a token that is unknown/expired/revoked is reported as a malformed request rather than an unauthenticated one. All four causes below are the same defect surfacing once per operation, and each must be closed.

Cause 1 of 4: ListAccounts (fixture sso-credential-vending-workflow, step task#1; same cluster also covers sso-list-accounts-pagination-bounds task#1..#3).
  AWS: ListAccounts reads `accessToken` from the `x-amz-sso_bearer_token` header; when the token is not a valid session the operation fails with the modeled UnauthorizedException, httpError 401 (Smithy: ListAccounts errors = InvalidRequestException 400, ResourceNotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401; UnauthorizedException doc: "Indicates that the request is not authorized. This can happen due to an invalid access token in the request."). The recorded response is HTTP 401 with error discriminator `UnauthorizedException` (recorded `__type` value `com.amazonaws.switchboard.portal#UnauthorizedException`) and, as an example of the text, message "Session token not found or invalid".
  Emulator: HTTP 400 with `__type` `InvalidRequestException` and message "1 validation error detected: Value null at 'accessToken' failed to satisfy constraint: Member must not be null" — i.e. the header-bound member was treated as missing.
  Why: an SDK/CLI that must distinguish "re-login required" from "bad request" keys off the 401/UnauthorizedException pair. Getting 400 InvalidRequestException makes the AWS CLI and SDK credential providers report a client bug instead of triggering re-authentication, and no caller can ever reach a successful ListAccounts page because its token is never seen at all.
  Fix: the operation's request deserialization for the portal.sso service — bind `accessToken` from the `x-amz-sso_bearer_token` request header before required-member validation runs — plus the handler's session lookup, which must raise UnauthorizedException (401) for a token that is absent from or invalid in session state rather than falling through to the validation error.
  Traces: sso-credential-vending-workflow#task#1 (representative), sso-list-accounts-pagination-bounds#task#1, #task#2, #task#3.

Cause 2 of 4: ListAccountRoles (fixture sso-credential-vending-workflow, step task#2; cluster also covers sso-list-account-roles-unknown-account task#1..#2).
  AWS: same header binding; Smithy ListAccountRoles input = accessToken (@httpHeader x-amz-sso_bearer_token, @required), accountId (@httpQuery account_id, @required); errors include UnauthorizedException httpError 401. Recorded response: 401, discriminator `UnauthorizedException`, example message "Session token not found or invalid".
  Emulator: 400 `InvalidRequestException` with the null-`accessToken` validation message.
  Why: role enumeration for a stale session is reported as a malformed request, so a client never refreshes its token and the unknown-account path (which should be reachable only after authentication) can never be exercised.
  Fix: ListAccountRoles request deserialization (header binding) and its authentication check in the handler, before accountId/query validation.
  Traces: sso-credential-vending-workflow#task#2 (representative), sso-list-account-roles-unknown-account#task#1, #task#2.

Cause 3 of 4: GetRoleCredentials (fixture sso-credential-vending-workflow, step task#3; cluster also covers sso-get-role-credentials-errors task#1..#3).
  AWS: Smithy GetRoleCredentials input = roleName (@httpQuery role_name, @required), accountId (@httpQuery account_id, @required), accessToken (@httpHeader x-amz-sso_bearer_token, @required, @sensitive); errors include UnauthorizedException httpError 401. Recorded response: 401, discriminator `UnauthorizedException`, example message "Session token not found or invalid".
  Emulator: 400 `InvalidRequestException` with the null-`accessToken` validation message.
  Why: this is the credential-vending call. A credential provider that receives 400 InvalidRequestException treats the request as permanently broken instead of refreshing the SSO session, so automated role-credential refresh fails hard; and because the token is never read, the operation can never vend credentials at all.
  Fix: GetRoleCredentials request deserialization (header binding) plus the handler's token/session validation ahead of role resolution.
  Traces: sso-credential-vending-workflow#task#3 (representative), sso-get-role-credentials-errors#task#1, #task#2, #task#3.

Cause 4 of 4: Logout (fixture sso-credential-vending-workflow, step task#4; cluster also covers sso-logout-repeated-and-malformed task#1..#2).
  AWS: Smithy Logout input = accessToken only (@httpHeader x-amz-sso_bearer_token, @required); POST /logout; errors = InvalidRequestException 400, TooManyRequestsException 429, UnauthorizedException 401. Recorded response: 401, discriminator `UnauthorizedException`, example message "Session token not found or invalid".
  Emulator: 400 `InvalidRequestException` with the null-`accessToken` validation message.
  Why: repeated or post-expiry logout must be distinguishable as an auth failure (401); reporting 400 makes clients surface a request-construction error, and an already-invalidated session cannot be modeled because the token is never read.
  Fix: Logout request deserialization (header binding) and the session-state invalidation path, which must answer UnauthorizedException (401) for a token not present in state.
  Traces: sso-credential-vending-workflow#task#4 (representative), sso-logout-repeated-and-malformed#task#1, #task#2.

Note: the HTTP status (401) and the error code (`UnauthorizedException`) are the requirements here; the recorded message text "Session token not found or invalid" is cited only as an example. No open issue covers this service — the existing open `sso-admin-*` issues are for the sso-admin (SWBExternalService) surface, not the portal.sso bearer-token path.

## Evidence
Replay of simparity-wire-aws-v1 (5 fixtures, 15 steps, 14 mismatched, score 6.7%, emulator_up=true, bootstrap_signal=false, state_reset_failures=0, no blocked/not-implemented steps) produced 4 divergence clusters, all on service `portal.sso`, all with the identical shape:

  divergence_key service=portal.sso|ListAccounts|status=401->400|keys=$.__type,$.message,status
  status: expected 401, got 400
  $.__type: expected 'com.amazonaws.switchboard.portal#UnauthorizedException', got 'InvalidRequestException'
  $.message: expected 'Session token not found or invalid', got "1 validation error detected: Value null at 'accessToken' failed to satisfy constraint: Member must not be null"

Identical detail for ListAccountRoles (task#2), GetRoleCredentials (task#3) and Logout (task#4). 14 of the 15 replayed steps fail this way across the 5 sso fixtures. Model evidence: smithy_query operation=ListAccounts,ListAccountRoles,GetRoleCredentials,Logout on services/service-com-amazonaws-sso-swbportalservice/smithy-model/com-amazonaws-sso-swbportalservice.json shows `accessToken` bound via @httpHeader("x-amz-sso_bearer_token") and @required on all four inputs, and UnauthorizedException with httpError 401 in each operation's error list (also a service common error).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1 (replay_wire_corpus).
3. Fixture sso-credential-vending-workflow, step task#1: GET /assignment/accounts against the portal.sso endpoint with the session bearer token in the `x-amz-sso_bearer_token` header (a token that is not a valid/known session). Expected HTTP 401 with __type ending `#UnauthorizedException`; observed HTTP 400 with __type `InvalidRequestException` and a null-`accessToken` validation message.
4. Repeat with step task#2 (GET /assignment/roles?account_id=...), task#3 (GET /federation/credentials?account_id=...&role_name=...) and task#4 (POST /logout) — same 400-instead-of-401 result.
5. Also reproducible standalone via fixtures sso-list-accounts-pagination-bounds, sso-list-account-roles-unknown-account, sso-get-role-credentials-errors and sso-logout-repeated-and-malformed, whose every step fails identically.
