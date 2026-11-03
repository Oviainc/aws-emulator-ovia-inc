Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.e2e-custom-authorizer-protected-method-flow apigateway.flush-stage-cache-bypasses-ttl apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.e2e-custom-authorizer-protected-method-flow|run#48|AssertionError|assert allowed_status == 200, f"expected 200 on the real endpoint once authorized, got {allowed_status}"
Issue-Key: apigateway.flush-stage-cache-bypasses-ttl|_get_json_with_retries#18|AssertionError|raise AssertionError(f"never got a successful response from {url}: {last_err}")
Issue-Key: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found|_served#17|Timeout|time.sleep(3)
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# API Gateway execute-api data plane: a deployed stage answers 500 for a Lambda-backed integration, and a MOCK-method stage never answers at all (2 causes)

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two data-plane causes on the execute-api endpoint of a deployed REST API stage. Hosts, ports, api ids and stage names below are per-run values, never requirements.

Cause 1 of 2 — a deployed stage whose method has a Lambda integration answers HTTP 500 to a request that AWS serves from the backend. AWS: a GET on `https://<apiId>.execute-api.<region>.amazonaws.com/<stage>/<path>` for a method whose integration is a Lambda (AWS_PROXY) function, with the integration permitted and (where present) the custom authorizer allowing the call, is answered with the backend's own status — 200 and the Lambda's body. Recorded behavior only for the endpoint shape; the checks assert 200 plus the backend's body. Emulator: HTTP 500 Internal Server Error on every attempt, both for the authorized request behind an allowing TOKEN authorizer and for the cache-behaviour backend; the authorizer check reaches the stage (so routing works) and still receives 500, and the cache check never obtains a single successful baseline response in 15 s of retries. Why: no client can exercise anything downstream of the stage — authorizer allow/deny, stage cache hit/flush, response mapping — because the integration never produces a response, and a 500 is indistinguishable from a genuine backend failure. Fix: the execute-api data-plane integration dispatcher for AWS_PROXY/Lambda integrations (it must invoke the function and translate its proxy response instead of erroring). Related open issues 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describe the same family but do not own these two checks. Traces: apigateway.e2e-custom-authorizer-protected-method-flow, apigateway.flush-stage-cache-bypasses-ttl.

Cause 2 of 2 — a deployed stage serving MOCK methods never answers, so the check hits the 60 s cap with no response at all. AWS: on a resource carrying both an explicit GET method and an ANY method (both MOCK), the stage serves GET from the GET method and POST from ANY, while GetMethod for POST is NotFoundException; after the GET method is deleted and redeployed, ANY serves GET. Recorded behavior only (the checks assert the served bodies, not a modeled error). Emulator: the check is killed by the harness with `Check timed out after 60.0s` — no HTTP status, code or body is ever produced for the stage URL. Hypothesis: the execute-api route for a resource that has both an explicit verb and ANY never resolves to a handler, so the request hangs instead of being served or refused. Why: verb precedence between an explicit method and ANY is a routing requirement that an IAM grant or a GetMethod lookup is written against; a hang gives a client no verdict at all, and a hanging data plane cannot be distinguished from a slow one. Fix: the execute-api data-plane router's method-selection step for explicit-verb vs ANY on the same resource. Related open issue 20261029T193654Z-apigateway-api-gateway-execute-api-data-plane-10-root.md covers other execute-api root causes but does not own this check. Traces: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found.

## Evidence
From the simparity-behavioral-aws-v1 replay.
Cause 1: apigateway.e2e-custom-authorizer-protected-method-flow — AssertionError 'expected 200 on the real endpoint once authorized, got 500'. apigateway.flush-stage-cache-bypasses-ttl — AssertionError 'never got a successful response from https://127.0.0.1:<port>/_aws/execute-api/<apiId>/test/cachetest: HTTP Error 500: Internal Server Error' after the check's 15 s retry window; the backend is a real Lambda returning a fresh UUID per invocation.
Cause 2: apigateway.explicit-verb-method-takes-precedence-over-any-and-get-method-post-stays-not-found — 'Check timed out after 60.0s'. The check creates the API, /data-agreements/{agreementId} with MOCK GET and MOCK ANY, CreateDeployment to stage 'beta', then invokes the stage; no response is ever returned.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite.
2. Cause 1: create a REST API with an AWS_PROXY method backed by a Lambda, add the lambda:InvokeFunction permission for apigateway, deploy a stage, and GET the stage URL — the emulator answers 500 where the backend's 200 is expected. Repeat with a TOKEN authorizer returning Allow: the authorized request is still 500.
3. Cause 2: create a resource with both a MOCK GET method and a MOCK ANY method, CreateDeployment to a stage, then GET and POST the stage path — neither request is answered within 60 s.
