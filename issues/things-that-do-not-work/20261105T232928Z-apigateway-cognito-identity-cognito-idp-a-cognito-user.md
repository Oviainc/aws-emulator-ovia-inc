Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative|run#16|AssertionError|assert status == 403, f"got status {status} for tampered signature (expected 403): {body[:200]}"
Issue-Commit: 4ce3cdbc6bb418520467c5ef8974ac614cd5f813

# apigateway__cognito-identity__cognito-idp: a COGNITO_USER_POOLS authorizer refuses a JWT with a tampered signature with 401 Unauthorized instead of 403 (1 cause)

> _Note: this issue was identified against the code base at commit `4ce3cdbc6bb418520467c5ef8974ac614cd5f813`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteApiInvoke

## Description
Cause 1 of 1: on the execute-api data plane, a COGNITO_USER_POOLS authorizer classifies a structurally well-formed, pool-issued ID token whose SIGNATURE no longer verifies as the same refusal class as a missing/malformed identity source, answering HTTP 401 instead of 403.

This cause is NET-NEW and is not carried by any open issue. Open issue 20261023T210711Z-apigateway-cognito-identity-cognito-idp-cognito-authorizer-checks-705f2d.md lists this same check id, but it carries it under the replay key `...apigw-cognito-authorizer-invalid-token-negative|_wait_api_ready#7|AssertionError|raise AssertionError("API never became ready ...")` — the stage-routing/404-UnknownService shape. That shape is gone: the stage is now reachable, the in-file positive control (the untampered ID token) is accepted with 200 and the integration's body, and the check now fails strictly later, on the refusal CLASS of the tampered token. Open issue 20261102T044056Z-apigateway-cognito-idp-lambda-a-cognito-user-pools.md looks similar but is a different service heading and a different symptom (500 Internal server error for an unverifiable bearer token, correctly 401 for a missing one); it does not cover a signature-verification failure being answered 401 where AWS answers 403.

AWS: recorded behavior only — this is the execute-api data plane, which the apigateway Smithy model does not describe. The control-plane model carries only the configuration used here (Smithy: Authorizer.type=COGNITO_USER_POOLS, Authorizer.providerARNs, Authorizer.identitySource; GatewayResponseType enum includes the distinct types UNAUTHORIZED, ACCESS_DENIED, INVALID_SIGNATURE and EXPIRED_TOKEN, i.e. API Gateway models these refusals as different gateway responses rather than one). The recording shows API Gateway splitting the Cognito refusals: an identity source that is absent or not parseable as a token, and a token whose issuer is not one of the authorizer's providerARNs, are refused 401 with body {"message":"Unauthorized"} (fixed text, the single word Unauthorized, no interpolated values); a token that the authorizer DID parse and then rejected on trust grounds — here one byte flipped in the signature segment, so the JWT is otherwise well-formed and claims the right pool but fails verification against the pool's signing keys — is refused with HTTP 403, and the integration is never invoked, so the backend body never appears. The recording constrains the 403 leg to its status and to the absence of the integration body; it does not pin the 403 body text. Hypothesis (not stated by the recording or the model): the 403 is API Gateway's ACCESS_DENIED/INVALID_SIGNATURE gateway response, whose default body is {"Message":"<$context.authorizer denial message>"} — a fixer should source that sentence before asserting it.

Emulator: with the same API (resource /widgets, GET with authorizationType=COGNITO_USER_POOLS, a MOCK integration returning a fixed body, deployed to a stage) the tampered-signature GET is answered HTTP 401 with body {"message":"Unauthorized"} — the same status and body the emulator serves for a missing or unparseable identity source. The integration body is correctly withheld, so only the refusal class diverges.

Why: a client cannot distinguish "present a credential / refresh your token" (401) from "your credential was read and is not trusted" (403). SDKs, browsers and auth middleware act on exactly that split: a 401 drives a token-refresh-and-retry loop, which here re-presents a token that can never verify, instead of surfacing the hard denial the 403 signals. Any negative test or alarm keyed on 403 for a forged/tampered token also silently passes as if the token were merely missing.

Fix: the execute-api data-plane request path in the apigateway crate — the COGNITO_USER_POOLS authorizer evaluation step. It must separate its refusal classes: a missing/unparseable identity source or an issuer outside providerARNs maps to the UNAUTHORIZED gateway response (401), while a token that parses and names a configured provider but fails signature verification maps to the 403 gateway response; integration dispatch stays skipped on both paths.

Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative (the only surviving failing check for this cause; the sibling Cognito-authorizer checks of this group are owned by the two issues named above).

## Evidence
Verbatim from the replay of apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative:

  File "check_apigateway__cognito_identity__cognito_idp_apigw_cognito_authorizer_invalid_token_negative.py", line 52, in run
    assert status == 403, f"got status {status} for tampered signature (expected 403): {body[:200]}"
  AssertionError: got status 401 for tampered signature (expected 403): {"message":"Unauthorized"}

Expected (AWS, from the recording): status 403 for a well-formed pool-issued ID token with one character flipped in its signature segment, and the integration response body absent.
Actual (emulator): status 401, body {"message":"Unauthorized"} — the emulator's missing/malformed-token answer.

Localizing evidence from the same check: the two preceding legs passed — the untampered ID token was accepted with 200 and the MOCK integration's body, and the readiness poll saw the stage answer 401 for an unauthenticated GET. So stage routing, token acceptance and the UNAUTHORIZED gateway response all work; only the classification of a signature-verification failure diverges.

Requirement being violated: HTTP status 403 (not 401) on the signature-failure path, with the integration body withheld. The api id, stage name, host/port, user-pool id, app-client id, user name and the token bytes in the recording are per-run values and are not part of the requirement; the fixed text of the 401 body the emulator wrongly serves here is the single word "Unauthorized" with no interpolated parts.

## Steps to Reproduce
1. Create a Cognito user pool, an app client with ALLOW_ADMIN_USER_PASSWORD_AUTH, and a confirmed user with a permanent password; AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) to obtain an IdToken.
2. Create a REST API; create resource /widgets; create-authorizer --type COGNITO_USER_POOLS --provider-arns <pool ARN> --identity-source method.request.header.Authorization.
3. put-method GET /widgets --authorization-type COGNITO_USER_POOLS --authorizer-id <id>; put-integration --type MOCK with requestTemplates {"application/json": "{\"statusCode\": 200}"}; put-method-response 200; put-integration-response 200 with a recognizable JSON body.
4. create-deployment --stage-name <stage>.
5. GET https://<stage-url>/widgets with header Authorization: <IdToken> -> 200 and the integration body (passes today).
6. Split the IdToken on '.', flip the first character of the signature segment, and GET the same URL with that token as Authorization.
   Expected: HTTP 403, and no integration body in the response.
   Observed: HTTP 401 with body {"message":"Unauthorized"}.
