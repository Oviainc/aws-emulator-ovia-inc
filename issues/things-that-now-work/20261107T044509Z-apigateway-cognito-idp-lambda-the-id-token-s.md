Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda|run#24|AssertionError|assert claims["iss"] == expected_iss, f"expected claims.iss {expected_iss!r}, got {claims.get('iss')!r}"
Issue-Key: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration|run#19|AssertionError|assert claims.get("iss") == expected_iss, (
Issue-Commit: 44a376568f69750b1a2f22f40952097a8153872e

# apigateway__cognito-idp__lambda: the ID token's `iss` claim is hard-coded to cognito-idp.<region>.amazonaws.com instead of the endpoint the pool is served on, so authorizer claims carry an unreachable issuer (1 cause)

> _Note: this issue was identified against the code base at commit `44a376568f69750b1a2f22f40952097a8153872e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1 — the Cognito user-pool token issuer (`iss`) is a fixed literal rather than the endpoint that serves the pool, so the issuer a COGNITO_USER_POOLS authorizer passes through to the integration names a host the client never addressed and cannot reach.

AWS: recorded behavior only — the `iss` claim is token content and is not Smithy-modeled (smithy_query on cognito-idp InitiateAuth/AdminInitiateAuth models only AuthenticationResultType.IdToken as an opaque string; no member or trait constrains the claim set). AWS documents the rule relationally, and it is what the recording shows: a user-pool ID/access token's `iss` is the user-pool endpoint for that pool — the SAME service endpoint the caller used for cognito-idp — with the pool id appended, i.e. `<cognito-idp endpoint>/<userPoolId>`. On real AWS that endpoint is `https://cognito-idp.<region>.amazonaws.com`, so `iss` reads `https://cognito-idp.<region>.amazonaws.com/<userPoolId>`; the fixed text is the scheme+host+path form `<endpoint>/<poolId>`, and the variable parts are the endpoint the request was served on and the pool id from the request. The relation matters because `iss` is the discovery root: verifiers (aws-jwt-verify, the published Cognito verification snippets, API Gateway/ALB JWT verification) fetch `<iss>/.well-known/jwks.json` to resolve `kid`, and they compare `iss` against the issuer they configured from their own cognito-idp endpoint. API Gateway surfaces the decoded claims verbatim in `event.requestContext.authorizer.claims`, so `iss` reaches the integration exactly as minted.

Emulator: the claim is minted from a hard-coded AWS hostname and ignores the endpoint the pool is actually served on. Replay shows the pool reached at the emulator's own cognito-idp endpoint, while the token delivered to the Lambda integration carries `iss` = `https://cognito-idp.us-west-2.amazonaws.com/<poolId>` — the right pool id, the wrong (and, for this deployment, unreachable) issuer host. Every other claim in the same token passes the check (`token_use`, `cognito:username`, `email`, `aud`, `sub`), so the divergence is isolated to how `iss` is composed.

Why: a client relying on AWS's behavior breaks two ways. (1) Issuer validation fails: it configures the expected issuer from the cognito-idp endpoint it is using and compares it to `iss`, and the two can never match, so every otherwise-valid token is rejected as foreign-issuer. (2) Key discovery leaves the deployment: a verifier that follows `iss` to `<iss>/.well-known/jwks.json` is sent to the public AWS hostname instead of the emulator that minted and can verify the token — the emulator announced an address other than the one it is reached on, so JWKS resolution either egresses or fails outright, and no token minted here is verifiable. Per-user authorization in the backend, which reads these claims, therefore cannot trust the issuer it is handed.

Fix: the cognito-idp token builder's claim assembly — the component that composes the `iss` claim for ID and access tokens must derive the issuer from the endpoint the pool is served on (the request's resolved service endpoint / configured external address) plus the pool id, not from a constant `cognito-idp.<region>.amazonaws.com` string. The same derived value must be used wherever the issuer is emitted or matched (token minting, token validation, and the pool's well-known JWKS route), so that `<iss>/.well-known/jwks.json` resolves on the emulator. API Gateway's COGNITO_USER_POOLS authorizer needs no change: it passes the claim through correctly.

Traces: apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-passthrough-to-lambda, apigateway__cognito-idp__lambda.apigw-cognito-authorizer-claims-reach-integration.

Related but NOT the same cause: the open issue 20261029T182116Z-cognito-idp-issued-tokens-diverge-es256-instead-of.md covers token signing (ES256 vs RS256) and the pool's JWKS route 404 under the `cognito-idp` service heading; it says nothing about the issuer host and carries neither of the replay keys above.

## Evidence
Verbatim from the replay (both checks fail on the same assertion, each with its own run's endpoint and pool id):

- apigw-cognito-authorizer-claims-passthrough-to-lambda:
  `assert claims["iss"] == expected_iss` ->
  `AssertionError: expected claims.iss 'https://<emulator cognito-idp endpoint>/us-west-2_e3c48d1ae', got 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_e3c48d1ae'`

- apigw-cognito-authorizer-claims-reach-integration:
  `assert claims.get("iss") == expected_iss` ->
  `AssertionError: claims.iss should be 'https://<emulator cognito-idp endpoint>/us-west-2_775c41b83', got 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_775c41b83'`

Which parts are requirements: the expected value is composed as `<the cognito-idp endpoint the caller is using>/<userPoolId>` (the check derives it as `ctx.client('cognito-idp').meta.endpoint_url.rstrip('/') + '/' + pool_id`). The fixed text is that composition; the endpoint host:port and the `us-west-2_<hex>` pool id shown above are this run's values and are NOT part of the requirement. What diverges is the constant `cognito-idp.us-west-2.amazonaws.com` authority the emulator substitutes for the serving endpoint.

Everything else in the same delivered token matched, which localizes the defect: `token_use == 'id'`, `cognito:username`, `email`, `aud == clientId` and `sub == the sub returned by AdminCreateUser` all passed before the `iss` assertion; the HTTP invoke returned 200 and the integration received `claims` as a map.

## Steps to Reproduce
1. CreateUserPool + CreateUserPoolClient; AdminCreateUser and confirm the user (note the `sub` returned).
2. Authenticate (AdminInitiateAuth/InitiateAuth USER_PASSWORD_AUTH) and keep the IdToken.
3. CreateRestApi; create_authorizer(type='COGNITO_USER_POOLS', providerARNs=[<pool arn>], identitySource='method.request.header.Authorization'); put_method(authorizationType='COGNITO_USER_POOLS') with an AWS_PROXY integration to a Lambda that echoes `event['requestContext']['authorizer']['claims']`; create_deployment.
4. GET the deployed stage URL with `Authorization: <IdToken>` -> 200 with the claims map.
5. Compare `claims['iss']` to `<the cognito-idp endpoint used in step 2>/<userPoolId>`: AWS makes these equal; the emulator returns `https://cognito-idp.<region>.amazonaws.com/<userPoolId>` regardless of the endpoint it is served on.
6. Follow the returned `iss` to `<iss>/.well-known/jwks.json` to see that the announced issuer is not the address the emulator is reached on.
