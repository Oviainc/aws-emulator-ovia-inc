Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.http-api-route-key-precedence-and-path-parameters apigatewayv2.import-api-payload-format-version-extension-honored apigatewayv2.import-api-then-reimport-changes-routing
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.http-api-route-key-precedence-and-path-parameters|_poll_http#17|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.import-api-payload-format-version-extension-honored|_poll_http#14|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.import-api-then-reimport-changes-routing|_poll_http#19|Timeout|time.sleep(3)
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2: an HTTP API's invoke URL never serves, so route-key precedence and both ImportApi routing checks hang to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `5d61882ef55489b75c810fe19d4c281a0249999c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane), ImportApi

## Description
One cause, grounded in three failing checks the replay report marked neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: a deployed HTTP API never answers on the invoke URL the emulator itself announces, so route resolution cannot be observed ===
AWS: recorded behavior only (execute-api data plane; not in the Smithy model). After CreateApi(ProtocolType='HTTP') — or ImportApi/ReimportApi of an OpenAPI body — plus an integration, routes and a stage, a request to the stage's own invoke URL is served by API Gateway with HTTP 200 and the integration's response for the matching route key. The three checks pin properties that are only visible there: a more specific route key wins over a less specific one and path-parameter values reach the integration event; the x-amazon-apigateway-integration payloadFormatVersion extension carried in an imported OpenAPI body governs the event shape actually delivered; and a ReimportApi of a changed body changes which route answers.
Emulator: no invoke ever returns the expected status. Each check polls the announced invoke URL on a retry loop and never converges, so the check is killed at the suite's 60 s cap and reported as `Check timed out after 60.0s`. Hypothesis: the same absent execute-api HTTP listener as the related open issue below — no response was observed at any point in these three checks, so the exact failing hop (invoke listener vs. the ImportApi-created routes never being deployable) is not pinned by the replay itself.
Why: everything a client does with an HTTP API after creating it is invoke traffic. Route-key precedence and path parameters are the core of HTTP API routing, and ImportApi is only useful if the imported document actually changes what the endpoint serves; all of it is unverifiable, and an IaC or OpenAPI-driven workflow that passes against the emulator proves nothing about AWS.
Fix: the execute-api HTTP data-plane listener (route-key matching with precedence, path-parameter extraction, stage resolution, integration dispatch and payload-format-version event shaping) and the invoke-URL host/path the control plane announces; plus the deployment path for routes created by ImportApi/ReimportApi. Related open issue 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md describes this surface as its Cause 1 but does not carry these three checks' replay keys, so a fixer re-replaying only that issue's trace list will not re-run them.
Traces: apigatewayv2.http-api-route-key-precedence-and-path-parameters, apigatewayv2.import-api-payload-format-version-extension-honored, apigatewayv2.import-api-then-reimport-changes-routing.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 against this checkout: 18/194 passed.

apigatewayv2.http-api-route-key-precedence-and-path-parameters [mismatch]: Error: Check timed out after 60.0s.
apigatewayv2.import-api-payload-format-version-extension-honored [mismatch]: Error: Check timed out after 60.0s.
apigatewayv2.import-api-then-reimport-changes-routing [mismatch]: Error: Check timed out after 60.0s.

Expected: HTTP 200 (and the route-specific body) from the stage's announced invoke URL within each check's internal poll window, so the remaining assertions on route precedence, path parameters and reimported routing can run.
Actual: no expected status is ever returned; each check's poll loop runs until the suite's 60 s cap kills it, so not one invoke assertion is reached.

None of the three carried an ALREADY FILED or OWNED-by marker in the replay report.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='HTTP'), CreateIntegration(AWS_PROXY or HTTP_PROXY), CreateRoute for both 'GET /items/{id}' and 'GET /items/special', CreateStage(StageName='$default', AutoDeploy=true).
3. GET the invoke URL the emulator announced for that api id and stage, once for each route key. Expect HTTP 200 with the integration's response and the path parameter present in the event; observe no answer at all.
4. ImportApi with an OpenAPI body carrying x-amazon-apigateway-integration payloadFormatVersion, deploy it, and GET the announced invoke URL; then ReimportApi with a changed body and GET again. Expect the served route/event shape to follow the imported document.
5. Re-replay the three check ids in trace_ids.
