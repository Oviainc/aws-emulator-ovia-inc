Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-mock-integration-serves-cors-preflight apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response apigateway__lambda.deep-apigw-integration-request-authorizer-cache-keys-on-every-identity-source
Issue-Key: apigateway__lambda.apigw-mock-integration-serves-cors-preflight|_poll_http#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status|_poll#18|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response|_poll#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__lambda.deep-apigw-integration-request-authorizer-cache-keys-on-every-identity-source|_poll#19|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__lambda: four integration/authorizer behaviour checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage never returns.
AWS: a deployed stage answers its invoke URL — an OPTIONS preflight served from a MOCK integration returns the configured CORS headers; a non-proxy Lambda integration whose selectionPattern matches the function's error maps that error to the configured status; a non-proxy integration's VTL request and response templates transform the bodies; a REQUEST authorizer's cache is keyed on every declared identity source, so changing any one of them re-invokes it. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives for any of the four. Each exhausts the harness budget and is reported as "Check timed out after 60.0s".
Why: CORS preflight, error-selection-pattern status mapping, VTL request/response transformation and authorizer cache keying are all unobservable, and the caller stalls rather than getting an answer.
Fix: the execute-api data-plane request path — stage/route resolution, template evaluation, authorizer caching and integration dispatch must always produce a response.
Traces: apigateway__lambda.apigw-mock-integration-serves-cors-preflight, apigateway__lambda.deep-apigw-integration-lambda-error-selection-pattern-maps-status, apigateway__lambda.deep-apigw-integration-lambda-non-proxy-vtl-maps-request-and-response, apigateway__lambda.deep-apigw-integration-request-authorizer-cache-keys-on-every-identity-source.

## Evidence
Detail for all four checks is exactly "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. Deploy a REST API with (a) an OPTIONS method backed by a MOCK integration returning CORS headers and (b) a non-proxy Lambda integration with VTL templates, a selectionPattern and a REQUEST authorizer with several identity sources.
2. Request the stage invoke URL and observe that no response arrives within 60 s.
3. Or replay the check ids in trace_ids.
