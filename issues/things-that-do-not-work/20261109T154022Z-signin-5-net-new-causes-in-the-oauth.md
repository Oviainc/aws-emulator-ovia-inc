Issue-Source: trace-sweep-divergence
Issue-Trace: signin-oauth2-token-required-member-validation-wire-only
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: signin-oauth2-token-required-member-validation-wire-only signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected signin-iam-client-credentials-token-introspected-active
Issue-Shape-Id: com.amazonaws.signin#Signin
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|$.error
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|$.message
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|$headers.content-type
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|status
Issue-Key: signin|CreateOAuth2Token|4xx->4xx|$.error_description
Issue-Key: signin|CreateOAuth2TokenWithIAM|4xx->4xx|$.error
Issue-Key: signin|CreateOAuth2TokenWithIAM|4xx->4xx|$.error_description
Issue-Key: signin|CreateOAuth2TokenWithIAM|4xx->4xx|$.message
Issue-Key: signin|CreateOAuth2TokenWithIAM|4xx->4xx|$headers.content-type
Issue-Key: signin|IntrospectOAuth2TokenWithIAM|2xx->2xx|$.client_id
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# signin: 5 net-new causes in the OAuth 2.0 token and introspection endpoints — anonymous /v1/token unroutable, refusals are not OAuth2 error objects, introspection omits client_id

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateOAuth2Token

## Description
Five independent causes in AWS Sign-In's OAuth 2.0 surface. They share one fault: the emulator does not serve the OAuth 2.0 wire contract of the /v1/token and /v1/introspect endpoints — the anonymous token endpoint is not reachable at all, the refusals of the IAM-authenticated token endpoint are framework-authored constraint reports instead of OAuth 2.0 error objects, and an IAM-minted token introspects without its client identifier. All five are net-new: no open issue carries any of these replay keys. ALL FIVE must be closed before this issue is submitted.

=== Cause 1 of 5: the anonymous CreateOAuth2Token endpoint (POST /v1/token) is not routed — every call is answered 404 UnknownService (fixture signin-oauth2-token-required-member-validation-wire-only, step task#1; same cluster also covers task#3 and task#4) ===
AWS: Smithy: CreateOAuth2Token is bound http POST /v1/token with NO auth scheme (the operation's auth list is empty — it is anonymous, authenticated by the OAuth 2.0 client credentials in its body, not by SigV4). Its input is the @httpPayload CreateOAuth2TokenRequestBody, whose clientId (@pattern ^arn:aws:signin:::devtools/(cross-device|same-device)$) and grantType (@pattern ^(authorization_code|refresh_token)$) are both @required. Recorded behavior: a request to that route whose client authentication cannot be established is answered HTTP 400 with content-type 'application/json' and an OAuth 2.0 error object: error 'INVALID_REQUEST' (the OAuth2ErrorCode member the model documents for validation failures) and message 'Client authentication failed.', plus an x-amzn-errortype header. That message is entirely fixed text — it interpolates nothing from the request or from state.
Emulator: HTTP 404 with x-amzn-errortype 'UnknownService', content-type 'application/x-amz-json-1.1', and the body message 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'. No 'error' member is present at all. The request never reaches the signin service, so the operation is entirely unreachable over the wire.
Why: a client that redeems an authorization code or refreshes a token against /v1/token — the only way to obtain Sign-In OAuth credentials without IAM signing — gets a 404 naming a service-resolution failure instead of the operation's modelled 400/200. The whole anonymous grant flow (authorization_code and refresh_token) cannot be exercised, and a client cannot even distinguish 'my credentials were rejected' from 'this endpoint does not exist'.
Fix: the front-door router / service resolution. An unsigned restJson1 request must be resolved to its service by its HTTP route (method + modelled URI) when there is no SigV4 credential scope, no X-Amz-Target and no recognisable Host, so an operation modelled with no auth scheme reaches its handler. Related but NOT covering: open issue 20261029T182307Z-smoke-an-unsigned-request-to-the-sts-endpoint describes the same resolution rule for an unsigned STS request, and open issues 20261102T032217Z-signin-resource-policy-and-console-authorization-operations-are-0875c7 / 20261106T021124Z-signin-console-authorization-and-resource-policy-refusals-are concern signin's host advertisement and its resource-policy refusals — none of them carries this fixture step or this operation.

=== Cause 2 of 5: once routed, CreateOAuth2Token must answer an unsupported grant type with the RFC 6749 unsupported_grant_type object (fixture signin-oauth2-token-required-member-validation-wire-only, step task#2) ===
AWS: Smithy: CreateOAuth2TokenRequestBody.grantType is @required with @pattern ^(authorization_code|refresh_token)$. Recorded behavior only (the model's OAuth2ErrorCode enum does not contain this code, so the wire is the authority): a /v1/token request naming a grant type outside that pattern is answered HTTP 400, content-type 'application/json', with error 'unsupported_grant_type', error_description 'The authorization grant type is not supported by the authorization server.' and message carrying that same sentence. Both the code and the sentence are fixed text for this error class — nothing in them comes from the request.
Emulator: HTTP 404, x-amzn-errortype 'UnknownService', content-type 'application/x-amz-json-1.1', message 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'; neither 'error' nor 'error_description' is serialized.
Why: a client that sends a grant type this service does not implement cannot tell it apart from any other failure, and cannot fall back to a supported grant: the standard OAuth 2.0 discriminator (the 'error' code) is absent. Note this is a SEPARATE requirement from cause 1 even after routing is fixed — the handler must distinguish 'client authentication failed' (INVALID_REQUEST) from 'grant type not supported' (unsupported_grant_type), and the error body must carry error_description alongside error.
Fix: CreateOAuth2Token's handler and its error serializer — the operation needs its own validation branch for grantType and an OAuth 2.0 error body shape that carries error / error_description / message with content-type application/json, rather than the generic constraint walk's report.

=== Cause 3 of 5: CreateOAuth2TokenWithIAM answers an unacceptable grant_type with the generic constraint-walk message instead of the OAuth 2.0 error object (fixture signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected, step task#1) ===
AWS: Smithy: CreateOAuth2TokenWithIAM.grantType targets ClientCredentialsGrantType, jsonName 'grant_type', @required, @pattern ^client_credentials$; the operation models ValidationException (HTTP 400) whose members are error (OAuth2ErrorCode, @required) and message (@required). Recorded behavior: a request whose grant type is not acceptable to the client-credentials endpoint is answered HTTP 400 with error 'unsupported_grant_type', error_description 'The authorization grant type is not supported by the authorization server.' and message carrying that same sentence — fixed text for this error class, with no request-derived values.
Emulator: HTTP 400 (status matches) but the body is "1 validation error detected: Value null at 'grantType' failed to satisfy constraint: Member must not be null" in the message member, with no 'error' and no 'error_description' member at all.
Why: two distinct defects visible in one response. (a) The refusal is worded by the generic Smithy constraint walk, so a client parsing the OAuth 2.0 'error' code gets nothing to branch on — exactly what services/service-com-amazonaws-signin-signin/src/ops/create_o_auth2_token_with_iam.rs claims not to do in its own doc comment ('Input rejection is this operation's own, not the generic constraint walk's'), yet the walk clearly still runs first. (b) Hypothesis: the walk reports grantType as null even though the recorded request carries the member, which is consistent with the deserializer not honouring the member's jsonName 'grant_type' — if so, a well-formed client_credentials request is also mis-read as missing its grant type.
Fix: CreateOAuth2TokenWithIAM's validation and its deserializer — the grantType/resource constraints must be judged by the handler (which already owns that logic) and the refusal serialized as the OAuth 2.0 error object, and the input deserializer must bind the member from its modelled jsonName.

=== Cause 4 of 5: CreateOAuth2TokenWithIAM's missing-required-parameter refusal is misworded, omits error/error_description, and uses the wrong content-type (fixture signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected, step task#2) ===
AWS: Smithy: CreateOAuth2TokenWithIAM.resource is @required, @length(min 1, max 2048); ValidationException carries error (OAuth2ErrorCode, documented 'Will be INVALID_REQUEST for validation errors') and message. Recorded behavior: the refusal is HTTP 400 with content-type 'application/json;charset=UTF-8', error 'INVALID_REQUEST', error_description 'Missing required parameter: client_id' and message carrying that same sentence. The template is fixed text 'Missing required parameter: ' followed by the name of the absent parameter as the endpoint names it on the wire — the parameter name is the variable part; it is NOT a per-run value.
Emulator: HTTP 400 with content-type 'application/json' and message "1 validation error detected: Value null at 'resource' failed to satisfy constraint: Member must not be null" — the generic constraint-walk wording, with no 'error' and no 'error_description' member, and it names a different parameter than the endpoint does.
Why: a client cannot discover WHICH OAuth 2.0 parameter it failed to supply in the vocabulary the endpoint accepts, and cannot branch on the INVALID_REQUEST code at all. The charset-qualified content-type also differs, so a strict client content-negotiating on 'application/json;charset=UTF-8' sees an unexpected media type.
Fix: CreateOAuth2TokenWithIAM's validation and error serializer — emit the 'Missing required parameter: <wire parameter name>' message as error_description and message under error 'INVALID_REQUEST', and serialize the error with the content-type the endpoint uses.

=== Cause 5 of 5: IntrospectOAuth2TokenWithIAM never reports client_id for a token minted by the IAM client-credentials grant (fixture signin-iam-client-credentials-token-introspected-active, step task#2) ===
AWS: Smithy: IntrospectOAuth2TokenWithIAMResponse.clientId, jsonName 'client_id'. Recorded behavior: introspecting an active token that CreateOAuth2TokenWithIAM minted returns HTTP 200 whose body carries client_id 'arn:aws:signin:::client-credentials/sigv4' — a fixed, account-and-region-less ARN that identifies the IAM client-credentials grant itself, so it is a constant of the grant and not a per-run value.
Emulator: HTTP 200 (status matches) but the client_id member is absent from the body entirely. The handler at services/service-com-amazonaws-signin-signin/src/ops/create_o_auth2_token_with_iam.rs stores 'client_id: None' for this grant on the reasoning that no OAuth client takes part in the IAM flow, so introspection has nothing to report.
Why: a relying party that introspects a Sign-In token to decide which client it was issued to sees no client_id and must treat the token as clientless, so it cannot distinguish an IAM client-credentials token from any other and cannot apply per-client policy — the one claim that identifies how the token was obtained is missing.
Fix: CreateOAuth2TokenWithIAM's state write and/or IntrospectOAuth2TokenWithIAM's response assembly — a token minted by the IAM client-credentials grant must record/report the grant's fixed client identifier ARN as its clientId claim.

## Evidence
Replay of trace set simparity-wire-aws-v1 (12 steps, 5 passed, 7 mismatched, score 41.7%); emulator up (/__admin/health 200). Per cause:

Cause 1 — signin-oauth2-token-required-member-validation-wire-only#task#1 (cluster also: #task#3, #task#4), CreateOAuth2Token.
  expected: status 400; $.error = 'INVALID_REQUEST'; $.message = 'Client authentication failed.'; headers.content-type = 'application/json'; headers.x-amzn-errortype present.
  actual:   status 404; $.error missing; $.message = 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'; headers.content-type = 'application/x-amz-json-1.1'; headers.x-amzn-errortype = 'UnknownService'.

Cause 2 — signin-oauth2-token-required-member-validation-wire-only#task#2, CreateOAuth2Token.
  expected: status 400; $.error = 'unsupported_grant_type'; $.error_description = 'The authorization grant type is not supported by the authorization server.'; $.message = same sentence; headers.content-type = 'application/json'.
  actual:   status 404; $.error and $.error_description missing; $.message = 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'; headers.content-type = 'application/x-amz-json-1.1'; headers.x-amzn-errortype = 'UnknownService'.

Cause 3 — signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected#task#1, CreateOAuth2TokenWithIAM.
  expected: status 400; $.error = 'unsupported_grant_type'; $.error_description = 'The authorization grant type is not supported by the authorization server.'; $.message = same sentence.
  actual:   status 400; $.error and $.error_description missing; $.message = "1 validation error detected: Value null at 'grantType' failed to satisfy constraint: Member must not be null".

Cause 4 — signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected#task#2, CreateOAuth2TokenWithIAM.
  expected: status 400; $.error = 'INVALID_REQUEST'; $.error_description = 'Missing required parameter: client_id'; $.message = 'Missing required parameter: client_id'; headers.content-type = 'application/json;charset=UTF-8'.
  actual:   status 400; $.error and $.error_description missing; $.message = "1 validation error detected: Value null at 'resource' failed to satisfy constraint: Member must not be null"; headers.content-type = 'application/json'.

Cause 5 — signin-iam-client-credentials-token-introspected-active#task#2, IntrospectOAuth2TokenWithIAM.
  expected: status 200; $.client_id = 'arn:aws:signin:::client-credentials/sigv4'.
  actual:   status 200; $.client_id missing (None).

Model evidence: smithy_query(operation="CreateOAuth2Token,CreateOAuth2TokenWithIAM,IntrospectOAuth2TokenWithIAM") — CreateOAuth2Token http POST /v1/token with auth: [] (anonymous), input @httpPayload CreateOAuth2TokenRequestBody (clientId/grantType @required); CreateOAuth2TokenWithIAM grantType jsonName 'grant_type' @required @pattern ^client_credentials$, resource @required @length 1..2048, errors include ValidationException (400); IntrospectOAuth2TokenWithIAMResponse.clientId jsonName 'client_id'; ValidationException members error (OAuth2ErrorCode, @required) + message (@required). Code evidence: services/service-com-amazonaws-signin-signin/src/ops/create_o_auth2_token_with_iam.rs lines 187-236 (handler-owned INVALID_REQUEST branch, and 'client_id: None' at line 227); services/service-com-amazonaws-signin-signin/src/state.rs lines 73-100 (VALIDATION_ERROR_CODE / INVALID_REQUEST_MESSAGE).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1/2: POST an unsigned request to the signin data listener at /v1/token with a JSON body (no SigV4 Authorization header, no X-Amz-Target) — as fixture signin-oauth2-token-required-member-validation-wire-only does at task#1 (missing client authentication) and task#2 (unsupported grant type). Observe 404 'UnknownService' with content-type application/x-amz-json-1.1 instead of a 400 OAuth 2.0 error object with content-type application/json.
4. Cause 3/4: POST a SigV4-signed CreateOAuth2TokenWithIAM request to /v1/token?x-amz-client-auth-method=iam — fixture signin-oauth2-token-with-iam-grant-type-and-token-shape-rejected task#1 (grant type the endpoint does not support) and task#2 (missing required parameter). Observe the 400 body carries only a generic "1 validation error detected: ..." message, with no 'error'/'error_description' members, and (task#2) content-type 'application/json' rather than 'application/json;charset=UTF-8'.
5. Cause 5: call CreateOAuth2TokenWithIAM (grant_type=client_credentials, resource set) and then IntrospectOAuth2TokenWithIAM on the returned access token — fixture signin-iam-client-credentials-token-introspected-active task#1 then task#2. The 200 introspection body omits client_id, where AWS reports 'arn:aws:signin:::client-credentials/sigv4'.
