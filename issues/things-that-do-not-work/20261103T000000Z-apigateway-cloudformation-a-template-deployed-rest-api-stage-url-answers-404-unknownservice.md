Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough
Issue-Key: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive|run#8|AssertionError|assert status == 200, f"expected Allow decision to reach the integration, got {status}: {body!r}"
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough|run#13|AssertionError|assert status == 200, f"expected 200, got {status}: {body}"
Issue-Commit: c2c15166cf9b7cee19d5a54b2d3b1c8d8385cb40

# The two causes left open by 20261029T173827Z-apigateway-cognito-identity-cognito-idp-3-net-new: a template-deployed stage URL answers 404 UnknownService, and the claims-passthrough method answers 500

> _Re-filed from `20261029T173827Z-apigateway-cognito-identity-cognito-idp-3-net-new.md`, whose
> Causes 1 and 2 are now resolved. These two replay keys are NOT resolved and are carried here
> so a later scan re-runs them; nothing claims either behaviour works._

## Operation
Invoke

## Description
Two root causes, each carrying one replay key of the issue this is re-filed from.

Cause 1 of 2 — a request to the invoke URL of a REST API a CloudFormation TEMPLATE deployed is
not recognised as an `execute-api` data-plane request, because the api does not exist in API
Gateway at all.
AWS: an HTTP request to the stage invoke URL is routed to the method/integration and returns the
integration's response (200 for an Allow decision).
Emulator: HTTP 404 with body
`{"__type":"UnknownService","message":"could not resolve a target service from the request (no
SigV4 credential scope, X-Amz-Target, or Host header)"}`.
Why: the data plane itself resolves all five documented addressings already
(`services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs::resolve`,
pinned by that crate's `tests/all/stage_invoke_endpoint.rs` and
`tests/all/stage_invoke_shared_stage_names.rs`), and the sibling checks of this same suite that
create their api through `CreateRestApi` now reach it. What this check's URL names is the id
CloudFormation MINTED for its `AWS::ApiGateway::RestApi` — see
`services/service-com-amazonaws-cloudformation-cloudformation/src/state/template.rs:3520` — while
that stack performed no create against API Gateway: `state/provisioner.rs:1246` dispatches only
`AWS::ApiGateway::DomainName` for the REST control plane. So the stack reports CREATE_COMPLETE
for a resource API Gateway does not hold, and nothing can serve its stage.
Fix: the CloudFormation resource provider for `AWS::ApiGateway::RestApi` (and the
`::Resource`/`::Method`/`::Deployment`/`::Stage` children a deployed stage needs) must create
those resources through API Gateway and report the ids that service minted. This is the SAME
root cause as
`20261023T125454Z-apigateway-cloudformation-provisioned-api-gateway-resources-are-unaddressable-through.md`
(its Causes 1 and 2) and
`20261029T193654Z-apigateway-cloudformation-lambda-cloudformation-deployed-rest-apis-invoking-the-stage-url.md`
(its Cause 1); whichever change lands it closes this key too. Note for the fixer: both files
that must change are over the merge gate's 5,000-line limit (`state/provisioner.rs` 28,667
lines, `state/template.rs` 8,138 lines), so they need splitting into module directories first,
as its own commit.
Traces: apigateway__cognito-identity__cognito-idp.deploy-apigw-execution-role-trust-policy-template-positive.

Cause 2 of 2 — the method the claims-passthrough check invokes answers 500
`{"message":"Internal server error"}` after the authorizer has admitted the call.
AWS: the invocation returns the integration's response, 200, with the authorizer's claims
available to the integration.
Emulator: 500 `{"message":"Internal server error"}` — one of the two gateway responses that
sentence belongs to.
Why: it is no longer the authorizer. A `COGNITO_USER_POOLS` authorizer is now evaluated in full
(`src/service/invoke/cognito_user_pools.rs`) and answers 200 or 401 from state for every token,
and the sibling checks of this suite that present tokens of the same pools to the same authorizer
shape are served 200 — so what this method cannot serve is the INTEGRATION half, or an
authorizer of a kind whose decision is a Lambda return value. Both are gaps other open issues
own: `20261029T193654Z-apigateway-lambda-api-gateway-lambda-16-integration-checks-never-complete.md`
(no in-process runtime for a deployment package, so an `AWS_PROXY` integration is answered
through `API_CONFIGURATION_ERROR`) and
`20261027T030000Z-apigateway-a-token-lambda-authorizer-allow-decision-is-never-obtained.md`
(a `TOKEN`/`REQUEST` authorizer's decision, answered through
`AUTHORIZER_CONFIGURATION_ERROR`). A third possibility is a mapping template carrying a Velocity
DIRECTIVE (`#set`, `#if`), which `src/service/mapping_template.rs` does not execute and
`src/service/invoke.rs::carries_velocity_directive` refuses.
Fix: identify which of the three this check's api declares, then close it in the issue that owns
that gap; nothing about the user-pool authorizer evaluation is left to do for it.
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-claims-context-passthrough.

## Evidence
Cause 1: "expected Allow decision to reach the integration, got 404:
b'{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request
(no SigV4 credential scope, X-Amz-Target, or Host header)\"}'".
Cause 2: "expected 200, got 500: {\"message\":\"Internal server error\"}".

## Steps to Reproduce
1. Boot the emulator and replay the two checks in trace_ids.
2. Cause 1: create a stack whose template declares an `AWS::ApiGateway::RestApi` with a
   `::Resource`, a `::Method`, a `::Deployment` and a `::Stage`, wait for CREATE_COMPLETE, then
   call `GetRestApi` with the id the stack's `Ref` reports — it answers 404 NotFoundException —
   and GET the stage invoke URL built from that id, which answers 404 UnknownService.
3. Cause 2: read the api the claims-passthrough check builds (`GetMethod` / `GetIntegration` on
   the method it invokes) and check which of the three gaps above its configuration names.
