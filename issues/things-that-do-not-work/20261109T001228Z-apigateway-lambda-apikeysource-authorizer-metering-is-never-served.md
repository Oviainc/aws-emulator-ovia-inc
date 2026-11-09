Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header
Issue-Key: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header|_http#49|Timeout|with urllib.request.urlopen(req, timeout=timeout) as resp: # noqa: S310 - fixed scheme
Issue-Commit: 3b62b2c2884780aa78691e31a574f6e99c14f5dd

# apigateway__lambda: apiKeySource=AUTHORIZER metering is never served — the stage invoke poll runs to the harness cap, so the authorizer's usageIdentifierKey is never metered and x-api-key is never ignored

> _Note: this issue was identified against the code base at commit `3b62b2c2884780aa78691e31a574f6e99c14f5dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRestApi

## Description
Cause 1 of 1: a GET against a deployed REST API stage whose RestApi.apiKeySource is AUTHORIZER, on a method with apiKeyRequired=true behind a REQUEST Lambda authorizer that returns a usageIdentifierKey associated with a usage plan, never reaches the 200 the check's first (positive-control) poll waits for. The poll therefore never converges and the check is killed at the harness cap (180 s). Failing check id: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header.

AWS: Smithy — CreateRestApi.apiKeySource targets ApiKeySourceType, an enum whose only values are HEADER and AUTHORIZER, documented as "The source of the API key for metering requests according to a usage plan. Valid values are: HEADER to read the API key from the X-API-Key header of a request. AUTHORIZER to read the API key from the UsageIdentifierKey from a custom authorizer." So with apiKeySource=AUTHORIZER the key that satisfies apiKeyRequired and meters the request is the usageIdentifierKey the custom authorizer returns, and the x-api-key request header is not consulted at all: a request carrying NO x-api-key, a bogus (never-created) x-api-key, or a real-but-unassociated x-api-key is still ADMITTED (200, body produced by the integration) as long as the authorizer's usageIdentifierKey is an enabled API key on a usage plan associated with that api/stage; a request whose authorizer-returned usageIdentifierKey matches no API key is REFUSED with HTTP 403 and the INVALID_API_KEY gateway response, whose $context.error.responseType substitutes to the fixed text INVALID_API_KEY (the body itself is whatever the stage's PutGatewayResponse template for responseType=INVALID_API_KEY renders — template text and the api/stage it was configured on are request/state values, not part of the requirement). Flipping /apiKeySource to HEADER and redeploying inverts every verdict: no x-api-key becomes 403 INVALID_API_KEY, and a valid x-api-key admits even when the authorizer returns a bogus usageIdentifierKey. Recorded behavior only for the latency: in the recording the stage answers each leg promptly, within the check's own 240 s propagation window.

Emulator: the invoke URL for the deployed stage never answers 200 for the header-less positive-control leg, so the check's first _poll_until never satisfies its predicate and the replay ends with `Error: Check timed out after 180.0s`. Because that first leg blocks, none of the downstream legs are ever exercised: the 403/INVALID_API_KEY refusal for an unassociated usageIdentifierKey, the "x-api-key is ignored in both directions" legs (valid header + bogus authorizer key must still be 403; bogus or orphan header + valid authorizer key must still be 200), and the HEADER-mode inversion after UpdateRestApi replaces /apiKeySource and CreateDeployment redeploys.

Why: a client cannot use AUTHORIZER-sourced API keys at all. Per-caller metering behind a custom authorizer — the entire purpose of apiKeySource=AUTHORIZER — is unobservable, and a caller that polls the stage until it is served blocks until its own timeout instead of getting a response. Worse, because the semantics are never exercised, a target that silently falls back to reading x-api-key would be a metering and tenancy bypass: any client could forge the header to be billed/throttled as another tenant.

Fix: the execute-api (v1) data-plane stage pipeline for this api — the apiKeySource resolution step that must take the API key from the authorizer response's usageIdentifierKey instead of the x-api-key header, the apiKeyRequired/usage-plan admission check that consumes it, and the INVALID_API_KEY gateway-response serializer that renders the 403 — plus whatever is keeping the deployed stage from answering at all for this configuration.

NOTE: the open issue 20261025T014422Z-apigateway-lambda-the-api-key-source-authorizer-metering.md carries this same check id under a different corpus (simparity-behavioral-ga-prod-v1) and a 60 s cap, and the replay report did NOT mark this check as owned by it, so it is filed here against this suite's 180 s observation; if that issue is still the live work item they should be closed together.

Traces: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header.

## Evidence
Failing check (suite report, service heading `### apigateway__lambda`):
  [mismatch] apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header
  Error: Check timed out after 180.0s

The check's first blocking step is the positive control:
  _poll_until(lambda: GET <stage-invoke-url>/metered with {"x-key-mode": "good"} and NO x-api-key,
             predicate r[0] == 200, timeout_s=240.0, interval_s=8.0,
             description="... to be ADMITTED because the authorizer returned a usageIdentifierKey associated with the usage plan")

expected: HTTP 200 whose body is the MOCK integration response template, i.e. it carries the integration marker, because apiKeySource=AUTHORIZER makes the authorizer's usageIdentifierKey (an enabled API key on a usage plan associated with this api/stage) satisfy apiKeyRequired; and, on the later legs, HTTP 403 with the stage's INVALID_API_KEY gateway response whose $context.error.responseType renders the fixed text "INVALID_API_KEY" when the authorizer returns a usageIdentifierKey matching no API key.
actual:   the poll never observes a 200 — the stage invoke does not produce the admitted response within the window, and the replay is killed at the 180.0 s harness cap, so neither the admission nor the 403/INVALID_API_KEY refusal nor the HEADER-mode inversion is ever reached.

(The check's own propagation budget is 240 s, so the harness cap is hit first; on a target that serves this configuration the first leg is satisfied on an early poll and the whole check completes well inside the cap.)

## Steps to Reproduce
1. CreateRole + Lambda CreateFunction for a REQUEST authorizer that always Allows and returns usageIdentifierKey chosen from the caller's x-key-mode header ("good" -> a key value that will exist, anything else -> a syntactically valid but never-created value).
2. CreateRestApi with endpointConfiguration.types=[REGIONAL] and apiKeySource="AUTHORIZER"; confirm GetRestApi reports apiKeySource=AUTHORIZER.
3. CreateAuthorizer type=REQUEST pointing at that function with identitySource=method.request.header.x-key-mode and authorizerResultTtlInSeconds=0; Lambda AddPermission for apigateway.amazonaws.com on arn:aws:execute-api:<region>:<account>:<apiId>/authorizers/<authorizerId>.
4. CreateResource pathPart="metered"; PutMethod GET authorizationType=CUSTOM authorizerId=<id> apiKeyRequired=true; PutIntegration type=MOCK with requestTemplates {"application/json": "{\"statusCode\": 200}"}; PutMethodResponse 200; PutIntegrationResponse 200 with a response template containing a marker; PutGatewayResponse responseType=INVALID_API_KEY with a template embedding a distinct marker and "$context.error.responseType".
5. CreateDeployment + CreateStage; CreateApiKey (value >= 20 chars, enabled=true), CreateUsagePlan with apiStages=[{apiId,stage}], CreateUsagePlanKey keyType=API_KEY for that key. Also CreateApiKey a second enabled key that is NOT added to any usage plan.
6. GET https://<stage-invoke-url>/metered with header x-key-mode: good and NO x-api-key header. Expected 200 carrying the integration marker; observed: no such response — the request is never admitted and a poll runs to the cap.
7. Then (all currently unreachable): x-key-mode: bogus -> 403 with the INVALID_API_KEY gateway response and type=INVALID_API_KEY; x-key-mode: bogus + a VALID x-api-key -> still 403; x-key-mode: good + a bogus or a real-but-unassociated x-api-key -> still 200; UpdateRestApi replace /apiKeySource=HEADER + CreateDeployment(stageName) -> every verdict inverts.
