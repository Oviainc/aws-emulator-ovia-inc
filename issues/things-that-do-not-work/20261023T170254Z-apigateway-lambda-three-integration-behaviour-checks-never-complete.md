Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-integration-timeout-maps-to-504-and-does-not-cancel-the-lambda apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend
Issue-Key: apigateway__lambda.apigw-integration-timeout-maps-to-504-and-does-not-cancel-the-lambda|_poll_until#38|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: f9ff4020ff731d5f2129f1ceb9f16685ad9310db

# apigateway__lambda: three integration-behaviour checks never complete — the deployed REST API hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `f9ff4020ff731d5f2129f1ceb9f16685ad9310db`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: requests driven at a deployed REST API with a Lambda integration never return — three checks are terminated by the harness at its 60 s budget, having received no HTTP status and no error code.
AWS: recorded behavior only. Real AWS answers each of these promptly and distinguishably: (a) an integration that exceeds the method's integration timeout is answered to the caller with HTTP 504 while the Lambda invocation itself is NOT cancelled; (b) a cached authorizer policy scoped to one method is reused and a request to a method the policy does not cover is answered HTTP 403 without re-invoking the authorizer; (c) a TOKEN authorizer returning Deny yields HTTP 403 to the caller while the authorizer's context values still reach the backend on the allow path.
Emulator: none of the three produces any response. Each is killed with 'Error: Check timed out after 60.0s' — the emulator accepted the request and never completed it.
Why: a hang is the worst failure mode for a client: SDK and HTTP clients block until their own socket timeout rather than receiving the 504 or 403 they are written to handle, retry/backoff logic is never exercised, and the three distinct behaviours above (timeout mapping, authorizer-cache method scoping, deny-plus-context) cannot be distinguished from each other or from a crash.
Fix: the deployed-stage request path — integration timeout handling, authorizer cache lookup, and the response serializer for each — must always terminate and emit a response. Note the related open issue 20261023T125454Z-apigateway-traffic-driven-at-a-deployed-rest-api-and.md covers 41 other hanging traffic checks but does not carry these three replay keys.
Traces: apigateway__lambda.apigw-integration-timeout-maps-to-504-and-does-not-cancel-the-lambda, apigateway__lambda.deep-apigw-integration-authorizer-cache-reuses-method-scoped-policy-denied, apigateway__lambda.deep-apigw-integration-token-authorizer-deny-forbidden-and-context-reaches-backend.

## Evidence
Expected: HTTP 504 for the exceeded integration timeout (with the Lambda invocation left running); HTTP 403 for the method not covered by the cached authorizer policy; HTTP 403 for the TOKEN-authorizer deny, with the authorizer context reaching the backend on the allow leg.
Actual: Error: Check timed out after 60.0s for all three (report lines 97201, 100467, 101559). No HTTP status, no error code, no body — the request never completed.

## Steps to Reproduce
1. Create a REST API with a Lambda integration, deploy it to a stage.
2. (a) Set the integration's timeoutInMillis below the function's runtime and call the method — expect HTTP 504. (b) Attach a TOKEN authorizer with a method-scoped policy and caching enabled, call an uncovered method — expect HTTP 403. (c) Make the authorizer return Deny and call the method — expect HTTP 403.
3. Observe that each request never returns within 60 s.
Re-replay the three check ids in trace_ids.
