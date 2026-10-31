Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.greedy-proxy-and-path-parameter-routing apigateway.put-integration-response-selection-pattern-server-evaluated apigateway.mock-selection-pattern-selects-status-and-headers
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.greedy-proxy-and-path-parameter-routing|run#29|AssertionError|assert posted.get("route") == "greedy" and posted.get("method") == "POST", (
Issue-Key: apigateway.put-integration-response-selection-pattern-server-evaluated|run#13|AssertionError|assert ok_resp["status"] == 200, f"expected 200, got {ok_resp['status']}"
Issue-Key: apigateway.mock-selection-pattern-selects-status-and-headers|_poll_http#16|Timeout|time.sleep(interval_s)
Issue-Commit: 055568fd9ce9a7be471ce8dc9e6772f2075ef53d

# API Gateway MOCK/VTL evaluation: $context.httpMethod reports the configured ANY instead of the request method, and a MOCK request template's statusCode + integration-response selectionPattern are never evaluated (3 causes)

> _Note: this issue was identified against the code base at commit `055568fd9ce9a7be471ce8dc9e6772f2075ef53d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Three net-new root causes in the API Gateway request/response mapping layer (VTL evaluation of a MOCK integration and the $context namespace). None of the three failing checks is marked OWNED or ALREADY FILED by the replay report. The open issue 20261029T193654Z-apigateway-api-gateway-execute-api-data-plane-10-root.md ("API Gateway execute-api data plane: 10 root causes — invocations hang, VTL/selectionPattern unevaluated, ...") describes a neighbouring area and owns OTHER checks (e.g. apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body); it does not carry the replay keys of the three below, which is why they are filed here. Related: 20261029T200808Z-apigateway-api-gateway-authorizer-representation-and-testinvokemethod-execution-9.md.

Cause 1 of 3: $context.httpMethod resolves to the METHOD's configured httpMethod, not the HTTP method of the incoming request.
AWS: a method may be declared with httpMethod "ANY", which answers every HTTP verb on that resource; inside a mapping template $context.httpMethod is the HTTP method of the request being served (AWS mapping-template context-variable reference: $context.httpMethod = "The HTTP method used"). The execute-api data plane is not Smithy-modelled, so this is recorded behavior only: a POST to a resource whose only method is ANY must render $context.httpMethod as 'POST'. The rest of the greedy-routing contract does hold in the emulator (route selection, $context.resourcePath='/{proxy+}', and the multi-segment capture of method.request.path.proxy all matched), so this cause is isolated to the context variable.
Emulator: for POST on an ANY method the template renders $context.httpMethod as the configured literal. Replay detail: `AssertionError: expected the ANY method on {proxy+} to serve POST with $context.httpMethod='POST', got {'route': 'greedy', 'proxy': 'elsewhere/deep', 'resourcePath': '/{proxy+}', 'method': 'ANY'}` (status 200, no error code involved).
Why: a client or backend that branches on $context.httpMethod (a common single-ANY-method proxy pattern, and the value copied into access/execution logs and into non-proxy integration request templates) sees a constant 'ANY' for every verb, so GET/POST/DELETE become indistinguishable to the integration.
Fix: the request-mapping context builder for the execute-api data plane — populate $context.httpMethod from the inbound request line, not from the matched Method resource's httpMethod attribute (and keep 'ANY' only as the route-matching wildcard).
Traces: apigateway.greedy-proxy-and-path-parameter-routing.

Cause 2 of 3: TestInvokeMethod does not evaluate a MOCK integration's request template, so the integration status code is not derived and the integration-response selectionPattern is never matched.
AWS: a MOCK integration's request template produces the integration result, and `{"statusCode": <n>}` sets the backend status; the integration responses' selectionPattern then chooses the mapping (Smithy: IntegrationResponse.selectionPattern — "Specifies the regular expression (regex) pattern used to choose an integration response based on the response from the back end ... For all other HTTP and Amazon Web Services back ends, the HTTP status code is matched"; an absent selectionPattern is the default/success mapping). TestInvokeMethod returns that evaluated result (Smithy: TestInvokeMethodResponse members status/body/headers/log/latency; TestInvokeMethod errors are BadRequestException 400, NotFoundException 404, UnauthorizedException 401, TooManyRequestsException 429 — none is returned here). With the template `#set($code = $input.params('code'))\n{"statusCode": $code}`, pathWithQueryString `/?code=200` must yield status 200 and the default response's body, and `/?code=500` must yield status 500 and the selectionPattern='5\\d{2}' response's body.
Emulator: TestInvokeMethod answers status 500 for the code=200 invocation. Replay detail: `AssertionError: expected 200, got 500`. Hypothesis: $input.params()/#set in the MOCK request template are not evaluated, so no integration statusCode is derived and the invocation is treated as a backend failure instead of selecting the default (no selectionPattern) integration response.
Why: a client testing a method through TestInvokeMethod is told the method fails (500) when AWS serves 200 with the mapped body, and the selectionPattern branch that distinguishes success from error mappings is never exercised, so every branch collapses to one answer.
Fix: the TestInvokeMethod handler's integration-execution path — evaluate the MOCK integration's requestTemplates with the VTL engine (including $input.params and #set), take statusCode from the rendered body, then select the integration response whose selectionPattern matches that status (empty/absent = default) and apply its responseTemplates.
Traces: apigateway.put-integration-response-selection-pattern-server-evaluated.

Cause 3 of 3: on a deployed stage the same MOCK/selectionPattern path never produces a response at all, so neither the non-200 branch nor per-integration-response header mapping is served.
AWS: the same MOCK contract as cause 2 applies to a deployed stage: a request template containing `#if($input.params('scope') == "internal") {"statusCode": 200} #else {"statusCode": 500} #end` serves HTTP 200 for scope=internal and a real HTTP 500 on the wire for any other scope (not 200 with an error body), each branch rendering its own responseTemplates body and its own responseParameters header mapping (`method.response.header.X-Sim-Route` = 'internal' vs 'external'), because responseParameters are per integration response, not per method. The prior control-plane legs of the check (PutMethodResponse/PutIntegrationResponse and the GetIntegrationResponse round-trip of selectionPattern='5\\d{2}') succeeded, so the divergence is in serving the deployed stage.
Emulator: the check is killed at the harness's 60 s cap while polling `GET <stage>/select?scope=internal` for a 200 (its own budget is 120 s). Replay detail: `Error: Check timed out after 60.0s`. No status line, body or X-Sim-Route header is ever produced for either branch.
Why: a client of a deployed MOCK-backed method gets no response where AWS returns 200/500 with branch-specific bodies and headers, and an emulator that collapsed both branches to one status or emitted one static header set would be indistinguishable from a correct one.
Fix: the execute-api data plane's integration-execution and response-mapping stage — evaluate the MOCK requestTemplates (including #if), surface the derived status on the wire, select the integration response by selectionPattern, and apply that integration response's own responseParameters header mappings for the matching method response.
Traces: apigateway.mock-selection-pattern-selects-status-and-headers.

## Evidence
Cause 1 — apigateway.greedy-proxy-and-path-parameter-routing [mismatch]:
  expected: {"route":"greedy", "proxy":"elsewhere/deep", "resourcePath":"/{proxy+}", "method":"POST"} for POST /elsewhere/deep on an ANY method
  actual:   {'route': 'greedy', 'proxy': 'elsewhere/deep', 'resourcePath': '/{proxy+}', 'method': 'ANY'}
  (the earlier legs passed: GET /items/42 was served by /items/{id} with id='42', and the greedy route captured the full remainder)

Cause 2 — apigateway.put-integration-response-selection-pattern-server-evaluated [mismatch]:
  expected: TestInvokeMethod(pathWithQueryString='/?code=200') -> status 200, body containing 'ok-template'; TestInvokeMethod('/?code=500') -> status 500, body containing 'error-template'
  actual:   AssertionError: expected 200, got 500 (the code=200 invocation; the 500-branch assertions were never reached)

Cause 3 — apigateway.mock-selection-pattern-selects-status-and-headers [mismatch]:
  expected: GET /select?scope=internal -> 200, body {"route":"internal"}, header X-Sim-Route: internal; GET /select?scope=external -> 500 on the wire, body {"route":"external"}, header X-Sim-Route: external; repeating scope=internal selects the 200 branch again
  actual:   Error: Check timed out after 60.0s — the stage never answered 200 for scope=internal (poll budget 120 s), so no branch, body or X-Sim-Route header was observed

## Steps to Reproduce
Cause 1:
1. CreateRestApi (REGIONAL); CreateResource 'items' and child '{id}' with a MOCK GET whose 200 responseTemplate renders $context.resourcePath and $context.httpMethod; CreateResource '{proxy+}' under root with a MOCK method httpMethod='ANY' rendering $input.params('proxy'), $context.resourcePath and $context.httpMethod.
2. CreateDeployment + CreateStage, then POST <stage>/elsewhere/deep with Content-Type: application/json.
3. Observe "method":"ANY" in the rendered body where AWS renders "POST".

Cause 2:
1. CreateRestApi; PutMethod GET on root (authorizationType NONE); PutIntegration type=MOCK with requestTemplates {"application/json": "#set($code = $input.params('code'))\n{\"statusCode\": $code}"}.
2. PutMethodResponse 200 and 500; PutIntegrationResponse 200 (no selectionPattern, responseTemplates {"application/json":"{\"result\": \"ok-template\"}"}) and 500 (selectionPattern '5\\d{2}', responseTemplates {"application/json":"{\"result\": \"error-template\"}"}).
3. TestInvokeMethod with pathWithQueryString='/?code=200' — observe status 500 instead of 200/'ok-template'; then '/?code=500' — the 500/'error-template' selection is likewise not made.

Cause 3:
1. CreateRestApi (REGIONAL) with a '/select' resource; PutMethod GET with requestParameters {method.request.querystring.scope: false}; PutIntegration type=MOCK with the #if/#else request template emitting {"statusCode": 200} or {"statusCode": 500}.
2. PutMethodResponse 200 and 500 with responseParameters {method.response.header.X-Sim-Route: true}; PutIntegrationResponse 200 (selectionPattern '') and 500 (selectionPattern '5\\d{2}'), each with its own responseTemplates and responseParameters {method.response.header.X-Sim-Route: "'internal'" / "'external'"}.
3. CreateDeployment + CreateStage, then GET <stage>/select?scope=internal — nothing is ever served 200 (the check dies at the 60 s harness cap); GET ?scope=external likewise yields no 500/'external' response.
4. Or replay the three check ids in trace_ids.
