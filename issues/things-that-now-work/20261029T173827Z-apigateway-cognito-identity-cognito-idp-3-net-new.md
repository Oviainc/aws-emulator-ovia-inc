Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative apigateway__cognito-identity__cognito-idp.cognito-identity-authenticated-role-invokes-apigw-iam-auth-positive apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive|run#9|AssertionError|assert status == 200, f"expected 200 for a valid ID token, got {status}: {body}"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough|run#13|AssertionError|assert status == 200, f"expected 200, got {status}: {body}"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative|run#9|AssertionError|assert status == 200, f"positive control failed: expected 200, got {status}: {body}"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative|run#13|AssertionError|assert status == 200, f"positive control failed: expected 200, got {status}: {body}"
Issue-Key: apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative|run#14|AssertionError|assert "authoriz" in body.lower() or "forbidden" in body.lower(), body
Issue-Key: apigateway__cognito-identity__cognito-idp.cognito-identity-authenticated-role-invokes-apigw-iam-auth-positive|_poll_apigw_until#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive|run#8|AssertionError|assert status == 200, f"expected Allow decision to reach the integration, got {status}: {body!r}"
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# apigateway__cognito-identity__cognito-idp: 3 net-new data-plane root causes — a COGNITO_USER_POOLS authorizer always answers 500, SigV4 from Cognito-vended credentials is refused 'Sign the request with the emulator credentials', and a deployed stage URL answers 404 UnknownService

> _Note: this issue was identified against the code base at commit `c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Three distinct root causes on the API Gateway execute-api data plane as driven by Cognito. Each is a separate work item; all three must be closed before submitting. Related open issues that do NOT carry these replay keys: 20261023T170254Z-apigateway-cognito-identity-cognito-idp-cognito-authorizer-checks.md (TestInvokeAuthorizer/CreateAuthorizer stubs) and 20261023T210711Z-apigateway-cognito-identity-cognito-idp-cognito-authorizer-checks-705f2d.md.

Cause 1 of 3 — a method secured by a COGNITO_USER_POOLS authorizer answers HTTP 500 for every token, including a valid ID token.
AWS: recorded behavior only: invoking the deployed stage with a valid user-pool ID token in the configured identity source returns the integration's response, HTTP 200, with the authorizer's claims available to the integration as $context.authorizer.claims; an invalid/expired token returns 401 {"message":"Unauthorized"} and a token from a different user pool (or a wrong audience) returns 401 — never 500.
Emulator: every invocation returns HTTP 500 with body {"message":"Internal server error"}, so even the in-file positive controls fail.
Why: no client can authenticate against an API Gateway method backed by a Cognito authorizer, and the distinction between 'unauthorized' (401, fix the token) and 'the authorizer is broken' (500, page the operator) is inverted — a mis-authenticated caller is told the service failed.
Fix: the COGNITO_USER_POOLS authorizer evaluation on the execute-api request path in the apigateway service crate (token verification against the configured provider ARNs, claims injection, and the 401 refusal path).
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative.

Cause 2 of 3 — an AWS_IAM-authorized method rejects a SigV4 request signed with Cognito-vended temporary credentials, instead of evaluating execute-api:Invoke.
AWS: recorded behavior only: the signed request is authenticated as the federated role session and authorized against the role's policy — with execute-api:Invoke on the method ARN it returns the integration's 200 body; without that permission it returns 403 with an execute-api authorization message naming the assumed-role principal and the execute-api:Invoke action. Either way the request is accepted as a SigV4 request from those credentials.
Emulator: the invocation is refused with {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"} — the vended session's signature is not accepted at all, so the negative check never observes an execute-api authorization decision and the positive check never reaches 200 (it polled to the harness's 60 s cap).
Why: IAM-authorized APIs cannot be exercised with Cognito identity-pool credentials, which is the entire federation story for API Gateway; and the refusal advertises emulator-specific credentials rather than an AWS authorization outcome.
Fix: SigV4 verification + execute-api authorization on the data-plane request path — it must accept any credential the emulator's STS/Cognito vended and then evaluate the session's policy.
Traces: apigateway__cognito-identity__cognito-idp.cognito-identity-apigw-missing-execute-api-permission-negative, apigateway__cognito-identity__cognito-idp.cognito-identity-authenticated-role-invokes-apigw-iam-auth-positive (this second check times out at the harness's 60 s cap while polling its own 90 s budget for a 200; it is listed because the same refusal is what keeps it from ever reaching 200).

Cause 3 of 3 — a request to a deployed stage's invoke URL is not recognised as an execute-api data-plane request at all.
AWS: recorded behavior only: an HTTP request to the stage invoke URL is routed to the method/integration and returns the integration's response (200 for an Allow decision from the authorizer).
Emulator: HTTP 404 with body {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the front door tried to resolve a control-plane service from the request and found none.
Why: anything that calls a deployed API by URL (the normal way an API is consumed, including every CloudFormation-deployed API) gets a protocol-level 404 instead of its integration response, so no end-to-end API test can pass.
Fix: the emulator's request router — execute-api invoke URLs must be dispatched to the apigateway data plane rather than resolved as a control-plane target.
Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive.

## Evidence
Cause 1: "expected 200 for a valid ID token, got 500: {\"message\":\"Internal server error\"}"; "positive control failed: expected 200, got 500: {\"message\":\"Internal server error\"}" (twice); "expected 200, got 500: {\"message\":\"Internal server error\"}".
Cause 2: AssertionError: {"__type":"AccessDenied","message":"Sign the request with the emulator credentials"} from the SigV4-signed stage invocation; the paired positive check reports 'Check timed out after 60.0s' while polling _poll_apigw_until(..., expected_status=200, timeout_s=90.0).
Cause 3: "expected Allow decision to reach the integration, got 404: b'{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'".

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-replay exactly the checks in trace_ids; each cause's own checks are listed under its 'Traces:' line.
3. By hand for Cause 1: create a REST API with a COGNITO_USER_POOLS authorizer over a confirmed user pool, deploy a stage, and GET the stage URL with Authorization=<valid ID token> — expect 200 and the integration body; repeat with a token from another pool — expect 401 {"message":"Unauthorized"}.
4. By hand for Cause 2: vend credentials via GetCredentialsForIdentity for an authenticated role holding execute-api:Invoke on the method ARN, SigV4-sign a GET of the stage URL with them, and expect 200 (and a 403 execute-api authorization message when the grant is removed).

## Resolution — what this report does and does NOT claim now works

Read this before trusting the file's location: this report is filed as resolved for Causes 1
and 2 only.

* **Cause 1 — FIXED.** A `COGNITO_USER_POOLS` authorizer's decision is computed in full from
  the token presented, the user pools its `providerARNs` name (each publishes its own key set
  in this same process) and the method's `authorizationScopes`
  (`services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke/cognito_user_pools.rs`).
  A token one of those pools signed, inside its window, matching the authorizer's
  `identityValidationExpression` and claiming one of the method's scopes reaches the
  integration with its claims published as `$context.authorizer.claims.<name>`; every other
  token is `401 {"message":"Unauthorized"}`. No token question answers 500 any more. Three of
  the four replay keys under this cause pass; the fourth
  (`apigw-cognito-authorizer-claims-context-passthrough`) still answers 500 from the
  INTEGRATION half rather than the authorizer, and is re-filed (below).
* **Cause 2 — FIXED.** An `AWS_IAM` method is authorized by evaluating `execute-api:Invoke` on
  the method's own ARN for the principal the request's credential was minted for — which is
  how a session a Cognito identity pool vended becomes a principal the api's policies can be
  written about — refusing a caller neither its identity-based policies nor the api's resource
  policy allow with the sentence IAM words an authorization failure in
  (`.../src/service/invoke/execute_api_authorization.rs`).
* **Cause 3 — NOT FIXED.** The 404 `UnknownService` for a template-deployed stage URL is
  unchanged by that work and is not declined as a non-defect: its root cause is that
  CloudFormation reports an `AWS::ApiGateway::RestApi` created while performing no create
  against API Gateway, so the id its `Ref` hands back names no api. It is carried forward,
  with its replay key, in
  `issues/things-that-do-not-work/20261103T000000Z-apigateway-cloudformation-a-template-deployed-rest-api-stage-url-answers-404-unknownservice.md`,
  beside the one remaining Cause 1 key.
