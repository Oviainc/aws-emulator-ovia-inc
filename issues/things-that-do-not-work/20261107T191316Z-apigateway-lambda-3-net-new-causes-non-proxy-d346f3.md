Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied apigateway__lambda.apigw-mock-integration-serves-cors-preflight
Issue-Key: apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status|_poll#18|AssertionError|raise AssertionError(f"{description}: condition not met within {timeout_s}s; last {last!r}")
Issue-Key: apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response|_poll#21|AssertionError|raise AssertionError(f"{description}: condition not met within {timeout_s}s; last {last!r}")
Issue-Key: apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend|run#25|AssertionError|assert json.loads(d_text) == {
Issue-Key: apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied|run#26|AssertionError|assert json.loads(b_text) == {
Issue-Key: apigateway__lambda.apigw-mock-integration-serves-cors-preflight|_poll_http#15|AssertionError|raise AssertionError(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigateway__lambda: 3 net-new causes — non-proxy AWS/Lambda integrations answer 500, the Lambda-authorizer 403 body truncates AWS's policy-reason clause, and a CORS preflight to a MOCK OPTIONS method is refused 403 AccessDenied by the emulator's cross-site guard

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane)

## Description
Three distinct net-new root causes, grounded in failing checks of this suite that are neither ALREADY FILED nor OWNED by an open issue.

=== Cause 1 of 3: non-proxy AWS (Lambda) integrations answer 500 at a deployed stage ===
AWS: Smithy: PutIntegration.type -> IntegrationType enum [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]; `AWS` is the non-proxy Lambda/service integration. API Gateway renders the request template, invokes the function, then selects an integration response by `selectionPattern` against the Lambda error payload and applies the response template — so a healthy invocation is served as the mapped 200 and an error payload is mapped to the pattern's status.
Emulator: every request through a non-proxy AWS integration is answered HTTP 500 with the DEFAULT_5XX body `{"message":"Internal server error"}` for the full 150 s poll window, including the `mode=ok` happy path. The AWS_PROXY legs of the same suite are served, so the defect is specific to the non-proxy dispatch/mapping path.
Why: a client using the non-proxy Lambda integration — the way request/response shaping and `selectionPattern` error mapping are configured — gets a blanket 500 and can observe neither the mapped success body nor the error-to-status mapping.
Fix: the execute-api data-plane dispatcher for IntegrationType AWS (invoke the function, then run selectionPattern selection and the response mapping template).
Traces: apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status, apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response.

=== Cause 2 of 3: the Lambda-authorizer 403 body drops the clause naming WHY authorization failed ===
AWS: when a TOKEN authorizer's policy explicitly denies the method, API Gateway answers 403 with `x-amzn-errortype: AccessDeniedException` and body `{"Message":"User is not authorized to access this resource with an explicit deny in an identity-based policy"}`. When the returned policy simply does not cover the method (no matching Allow), the body is `{"Message":"User is not authorized to access this resource because no identity-based policy allows the execute-api:Invoke action"}`. Both sentences are fixed text — they carry no request- or state-derived values — and the two are distinct on purpose: they tell a caller whether a Deny matched or no Allow matched.
Emulator: both messages are truncated to a prefix. The explicit-deny case serves `{"Message":"User is not authorized to access this resource with an explicit deny"}` (the trailing `in an identity-based policy` is missing) and the no-matching-Allow case serves `{"Message":"User is not authorized to access this resource"}` (the whole `because no identity-based policy allows the execute-api:Invoke action` clause is missing). The status (403) and the error code (`AccessDeniedException`) are correct; only the message diverges.
Why: the two refusals become indistinguishable from each other and from a generic denial, so clients and operators that match on the message to tell an explicit Deny from a missing Allow — and tooling that asserts on AWS's documented sentence — mis-diagnose authorizer policies.
Fix: the execute-api authorizer-decision serializer that renders the AccessDeniedException body for a Lambda-authorizer policy evaluation; it must emit AWS's full sentence for each of the two outcomes.
Traces: apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend (explicit deny), apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied (cached policy that does not cover the method).

=== Cause 3 of 3: a CORS preflight is refused by an emulator-invented cross-site guard ===
AWS: an OPTIONS method with a MOCK integration and the CORS response-header mappings is the documented way to serve a preflight. API Gateway has no notion of "a browser from another site": it evaluates the OPTIONS method and returns the mapped status (200/204) with the Access-Control-Allow-* headers, regardless of the `Origin` request header. `AccessDenied` is not an API Gateway execute-api error code.
Emulator: the preflight is answered HTTP 403 with `{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}` for the whole 150 s window. That status, error code and message are all emulator-authored — the message is fixed text with no AWS counterpart — and the configured MOCK OPTIONS method is never reached.
Why: no browser client can call any API through this emulator: the preflight that precedes every cross-origin XHR/fetch is refused, so CORS can never be configured or verified.
Fix: the emulator's execute-api front-door request filter must not refuse requests carrying an `Origin` header (or must exempt the execute-api data plane from it) and must dispatch OPTIONS to the method's integration.
Traces: apigateway__lambda.apigw-mock-integration-serves-cors-preflight.

## Evidence
Cause 1 — expected the mapped 200 (and, for the error leg, the selectionPattern's status). Actual: `AssertionError: GET ?mode=ok through the Lambda integration: condition not met within 150.0s; last (500, {'content-type': 'application/json', ...}, '{"message":"Internal server error"}')` and `AssertionError: POST /orders/{id} through the AWS integration: condition not met within 150.0s; last (500, {...}, '{"message":"Internal server error"}')`.

Cause 2 — expected body (explicit deny): `{"Message": "User is not authorized to access this resource with an explicit deny in an identity-based policy"}`; actual: `{"Message":"User is not authorized to access this resource with an explicit deny"}`. Expected body (policy does not cover the method): `{"Message": "User is not authorized to access this resource because no identity-based policy allows the execute-api:Invoke action"}`; actual: `{"Message":"User is not authorized to access this resource"}`. Both served with the correct 403 and `x-amzn-errortype: AccessDeniedException`. Every word of both expected sentences is fixed text; neither contains a request- or state-derived value.

Cause 3 — expected: 200/204 from the MOCK OPTIONS method with the Access-Control-Allow-* headers. Actual: `AssertionError: CORS preflight on the MOCK integration https://<host>/_aws/execute-api/<apiId>/<stage>/items never reached an accepted status within 150.0s. Last: 403 '{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}'`.

## Steps to Reproduce
1. Cause 1: CreateRestApi; PutMethod; PutIntegration(type="AWS", integrationHttpMethod="POST", uri="arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fn arn>/invocations", credentials=<role>, requestTemplates={...}); PutMethodResponse + PutIntegrationResponse for 200 and for an error selectionPattern; CreateDeployment. Call the stage -> 500 {"message":"Internal server error"} on the happy path.
2. Cause 2: CreateAuthorizer(type="TOKEN") backed by a Lambda that returns a policy with an explicit Deny for the method; PutMethod(authorizationType="CUSTOM"); deploy; call with that token -> 403 body `{"Message":"User is not authorized to access this resource with an explicit deny"}` instead of AWS's `... with an explicit deny in an identity-based policy`. Then return a policy that Allows only another method and call this one -> body `{"Message":"User is not authorized to access this resource"}` instead of `... because no identity-based policy allows the execute-api:Invoke action`.
3. Cause 3: PutMethod(OPTIONS, authorizationType=NONE) + PutIntegration(type="MOCK") + method/integration responses mapping Access-Control-Allow-Origin/-Methods/-Headers; deploy; send `OPTIONS /items` with `Origin: https://example.com` and `Access-Control-Request-Method: GET` -> 403 `{"__type":"AccessDenied","message":"Requests a browser sends from another site are refused"}` instead of the mapped 200/204.
