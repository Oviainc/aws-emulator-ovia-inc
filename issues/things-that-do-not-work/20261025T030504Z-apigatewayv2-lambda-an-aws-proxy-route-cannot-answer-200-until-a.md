Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.http-api-route-key-precedence-and-path-parameters apigatewayv2.import-api-payload-format-version-extension-honored apigatewayv2.import-api-then-reimport-changes-routing
Issue-Key: apigatewayv2.http-api-route-key-precedence-and-path-parameters|_poll_http#17|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.import-api-payload-format-version-extension-honored|_poll_http#14|Timeout|time.sleep(3)
Issue-Key: apigatewayv2.import-api-then-reimport-changes-routing|_poll_http#19|Timeout|time.sleep(3)
Issue-Commit: 5d61882ef55489b75c810fe19d4c281a0249999c

# apigatewayv2 + lambda: an HTTP API route fronting an AWS_PROXY integration cannot answer 200, because no Lambda deployment package can run — the three route-precedence/ImportApi checks still hang (1 cause)

> _This report CARRIES FORWARD the three replay keys of
> `20261025T030504Z-apigatewayv2-an-http-api-s-invoke-url-never.md`, which is now under
> `issues/things-that-now-work/`. That report's cause was "a deployed HTTP API never answers on
> the invoke URL the emulator itself announces, so route resolution cannot be observed"; the
> invoke listener, the claim, stage resolution, the deployment gate, route-key matching with
> precedence and the authorization step are all implemented now
> (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs`), `ImportApi`
> persists the routes and integrations a document declares, and `ReimportApi` is no longer a 501
> stub — but NONE of its three checks can pass yet, for the single remaining reason below. The
> keys are re-filed here so they stay attributed to a cause instead of being lost._

## Operation
Invoke (execute-api data plane, AWS_PROXY/Lambda integration)

## Description

Cause 1 of 1: a route whose integration is `AWS_PROXY` cannot answer the HTTP 200 the checks poll
for, because the Lambda function it names cannot run.

AWS: recorded behavior only (execute-api data plane; not in the Smithy model). A request to a
deployed HTTP API stage whose matched route fronts an `AWS_PROXY` integration is answered with the
function's own response — HTTP 200 and the body the handler returned — and the event the function
receives carries the request's `rawPath`, its `pathParameters` and the shape the integration's
`PayloadFormatVersion` selects.

Emulator: the data plane resolves the api, the stage, the deployment and the route, applies the
route's authorization, and then has nothing to call: lambda `Invoke` answers HTTP 501
`NotImplemented` for `RequestResponse` and `Event` because there is no in-process runtime for a
deployment package
(`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` — "there is no
in-process runtime for it, and no response payload is returned that the function did not
produce"). So the request is answered the way API Gateway answers an integration that produced no
response, `500 {"message":"Internal Server Error"}`
(`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/invoke.rs`,
`serve_integration`). Each of the three checks polls the announced invoke URL with
`accept = (status == 200)` on a retry loop and never converges, so it is killed at the suite's
60 s cap and reported as `Check timed out after 60.0s`.

Why: every property these checks pin is only visible in the integration's answer — that a more
specific route key wins and its path-parameter values reach the event, that the
`x-amazon-apigateway-integration` `payloadFormatVersion` of an imported document governs the event
shape delivered, and that a `ReimportApi` of a changed body changes which route's backend answers.
The control-plane halves of all three are now implemented and covered by tests
(`tests/all/http_api_route_precedence.rs` pins route-key precedence end-to-end through the data
plane, and `tests/all/api_reimport.rs` pins that a reimported document changes which route answers
and what payload format version its integration carries), but no assertion that reads the
function's response or event can be reached.

Fix: an in-process runtime for a Lambda deployment package, and then the `AWS_PROXY` dispatch half
of the apigatewayv2 data plane — build the proxy event in the shape the integration's
`PayloadFormatVersion` selects (including `pathParameters` extracted from the matched route key),
invoke the function in-process, and map its response (`statusCode`/`headers`/`cookies`/`body`/
`isBase64Encoded`, plus the 2.0 response inference) onto the HTTP answer. The runtime half is owned
by the open issues `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`
and `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`, neither
of which carries these three checks' replay keys; the response-mapping half is adjacent to
`20261023T151412Z-apigatewayv2-lambda-all-four-http-api-error-propagation.md`. Nothing short of a
function actually running can close this: a status or body invented for a backend that never ran
would be indistinguishable from a real one.

Traces: apigatewayv2.http-api-route-key-precedence-and-path-parameters,
apigatewayv2.import-api-payload-format-version-extension-honored,
apigatewayv2.import-api-then-reimport-changes-routing.

## Evidence
Replay of the three checks against this checkout: 3 of 3 failed, each
`[mismatch] Error: Check timed out after 60.0s`, with every key reported `still_failing`.

Expected: HTTP 200 and the integration's response from the stage's announced invoke URL within each
check's internal poll window, so the remaining assertions on route precedence, path parameters and
reimported routing can run.
Actual: `500 {"message":"Internal Server Error"}` for the matched `AWS_PROXY` route — the honest
answer for an integration that produced no response — so the poll never accepts and the check is
killed at the cap.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. lambda: CreateFunction with a python3.12 zip whose handler echoes the event; wait for
   `State=Active`.
3. apigatewayv2: CreateApi(ProtocolType='HTTP'), CreateIntegration(IntegrationType='AWS_PROXY',
   IntegrationUri=<function arn>, PayloadFormatVersion='2.0'), CreateRoute for both
   'GET /items/{id}' and 'GET /items/special', CreateStage(StageName='$default', AutoDeploy=true).
4. GET the announced invoke URL at `/items/special` and at `/items/42`: both are answered 500
   rather than the function's response, so no event assertion is reachable.
5. Replay the three check ids in trace_ids.
