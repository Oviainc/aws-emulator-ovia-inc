Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-authorizer-lifecycle-test-invoke
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-authorizer-lifecycle-test-invoke apigateway-authorizer-lifecycle-with-test-invoke apigw-patch-model-validator-authorizer-invalid-path apigateway-resource-method-integration-full-lifecycle apigw-envelope-test-invoke-request-validator-body apigw-envelope-test-invoke-selection-pattern-mapped-status
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|CreateAuthorizer|2xx->2xx|$.authType
Issue-Key: apigateway|GetAuthorizer|2xx->2xx|$.authType
Issue-Key: apigateway|GetAuthorizers|2xx->2xx|$.item[name="sim2real-probe-apigateway-authorizer-f96ddc73"].authType
Issue-Key: apigateway|UpdateAuthorizer|2xx->2xx|$.authType
Issue-Key: apigateway|GetAuthorizers|2xx->2xx|$.item[name="sim2real-probe-apigateway-authorizer-51fd3d14"].authType
Issue-Key: apigateway|CreateAuthorizer|4xx->4xx|$.message
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.body
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.latency
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.status
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.headers.x-amzn-ErrorType
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.log
Issue-Key: apigateway|TestInvokeMethod|2xx->2xx|$.multiValueHeaders.x-amzn-ErrorType
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway authorizer representation and TestInvokeMethod execution (9 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Authorizer representations drop a modeled member, the missing-URI refusal is emulator-authored, and TestInvokeMethod does not execute the method: it neither runs a MOCK integration, nor applies the request validator, nor evaluates integration-response selection patterns. Smithy: Authorizer carries authType on CreateAuthorizer/GetAuthorizer(s)/UpdateAuthorizer output; TestInvokeMethodResponse = {status, body, headers, multiValueHeaders, log, latency}.

Causes 1-5 share one fault: `authType` is never serialised. AWS echoes the authType supplied at creation (recorded value `custom` for a TOKEN authorizer) on every Authorizer representation. Emulator omits the member. Why: a client reading an authorizer and writing it back loses authType, and console/CLI output differs from AWS's. Fix: the Authorizer serialiser / authorizer state.
  Cause 1 of 9 - CreateAuthorizer. Trace: apigateway-authorizer-lifecycle-test-invoke#task#3 (2 steps).
  Cause 2 of 9 - GetAuthorizer. Trace: apigateway-authorizer-lifecycle-test-invoke#task#4 (4 steps).
  Cause 3 of 9 - GetAuthorizers item. Trace: apigateway-authorizer-lifecycle-test-invoke#task#5.
  Cause 4 of 9 - UpdateAuthorizer. Trace: apigateway-authorizer-lifecycle-test-invoke#task#6 (3 steps).
  Cause 5 of 9 - GetAuthorizers in the second fixture. Trace: apigateway-authorizer-lifecycle-with-test-invoke#task#5.

Cause 6 of 9 - CreateAuthorizer of type TOKEN with no authorizerUri. AWS: 400 BadRequestException `URI is a required field in an Authorizer` (fixed text). Emulator: `Invalid request input`. Why: a client cannot tell which member was missing. Fix: CreateAuthorizer's validation message. Trace: apigateway-authorizer-lifecycle-with-test-invoke#task#2.

Cause 7 of 9 - TestInvokeMethod against a MOCK integration. AWS: 200 with the method response - status 200 and the mock's (empty) body. Emulator: status 500 with body `{"message":"Internal server error"}`. Why: a client using TestInvokeMethod to verify a method can never see a success. Fix: the TestInvokeMethod execution path for MOCK integrations. Trace: apigateway-resource-method-integration-full-lifecycle#task#23.

Cause 8 of 9 - TestInvokeMethod does not apply the method's request validator. AWS: for a body that fails the attached validator, the test invocation reports status 400, body `{"message": "Invalid request body"}`, and `headers`/`multiValueHeaders` carrying `x-amzn-ErrorType: BadRequestException`. Emulator: status 200 with the integration's response body, no error header. Why: request validation cannot be tested at all, and a negative test silently passes. Fix: run request validation inside TestInvokeMethod before the integration. Trace: apigw-envelope-test-invoke-request-validator-body#task#9.

Cause 9 of 9 - TestInvokeMethod ignores integration-response selectionPattern. AWS: when the integration result matches a non-default integration response's selectionPattern, the test invocation reports that response's mapped status and body (recorded: status 503 with the mapped error body). Emulator: the default 200 response. Why: error mapping cannot be exercised. Fix: selectionPattern evaluation in the TestInvokeMethod response pipeline. Trace: apigw-envelope-test-invoke-selection-pattern-mapped-status#task#10.

Not filed: the `$.log` text and `$.latency` differences that accompany causes 7-9 - the log carries per-run timestamps and latency is a per-run measurement. No open issue owns these clusters; the open TestInvokeMethod issue '...AWS_PROXY integration pointing at a Lambda alias...' is about a Lambda backend, not these.

## Evidence
Causes 1-5: $.authType missing (expected 'custom'); status 201/200 matched.
Cause 6: 400 both; $.message expected 'URI is a required field in an Authorizer', got 'Invalid request input'.
Cause 7: $.status expected 200 got 500; $.body expected '' got '{"message":"Internal server error"}'.
Cause 8: $.status expected 400 got 200; $.body expected '{"message": "Invalid request body"}' got '{"created": true}'; $.headers.x-amzn-ErrorType and $.multiValueHeaders.x-amzn-ErrorType expected 'BadRequestException'/['BadRequestException'], missing.
Cause 9: $.status expected 503 got 200; $.body expected '{"error": "backend unavailable"}' got '{"ok": true}'.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-6: POST/GET/PATCH /restapis/{id}/authorizers with authType=custom (and once without authorizerUri) and compare authType / the refusal message.
3. Cause 7: POST /restapis/{id}/resources/{rid}/methods/GET (TestInvokeMethod) on a MOCK integration; expect status 200.
4. Causes 8/9: attach a request validator / an integration response with a selectionPattern, then TestInvokeMethod with a failing body resp. a matching integration result.
