Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-quick-create-managed-resources
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-quick-create-managed-resources apigatewayv2-http-api-route-config-cleanup apigatewayv2-product-rest-endpoint-page-error-paths apigatewayv2-product-rest-endpoint-page-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetRoute|2xx->4xx|$.apiGatewayManaged
Issue-Key: apigateway|GetRoute|2xx->4xx|$.apiKeyRequired
Issue-Key: apigateway|GetRoute|2xx->4xx|$.authorizationType
Issue-Key: apigateway|GetRoute|2xx->4xx|$.routeId
Issue-Key: apigateway|GetRoute|2xx->4xx|$.routeKey
Issue-Key: apigateway|GetRoute|2xx->4xx|$.target
Issue-Key: apigateway|GetRoute|2xx->4xx|status
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.apiGatewayManaged
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.apiKeyRequired
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.authorizationType
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.routeId
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.routeKey
Issue-Key: apigateway|UpdateRoute|2xx->4xx|$.target
Issue-Key: apigateway|UpdateRoute|2xx->4xx|status
Issue-Key: apigateway|GetStage|2xx->2xx|$.routeSettings.GET /items
Issue-Key: apigateway|DeleteProductRestEndpointPage|2xx->4xx|$
Issue-Key: apigateway|DeleteProductRestEndpointPage|2xx->4xx|status
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.displayContent.endpoint
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.status
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.statusException
Issue-Key: apigateway|UpdateProductRestEndpointPage|2xx->2xx|$.displayContent.endpoint
Issue-Key: apigateway|UpdateProductRestEndpointPage|2xx->2xx|$.lastModified
Issue-Key: apigateway|UpdateProductRestEndpointPage|2xx->2xx|$.status
Issue-Related: 20261029T220031Z-apigateway-api-stage-and-deployment-timestamps-serialized-as.md
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway managed routes, route-settings deletion and repeat deletes diverge

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoute

## Description
Six clusters where state the emulator holds diverges from AWS's: an API-Gateway-managed route disappears instead of being readable, deleting a stage's per-route settings leaves them in place, a repeat delete is not idempotent, and a rest-endpoint-page never leaves its initial status. Each cause has its own fixture step.

Cause 1 of 6: GetRoute on a quick-create managed route.
AWS: 200 with the managed route body - `routeKey: $default`, `authorizationType: NONE`, `apiKeyRequired: false`, `target: integrations/<integrationId>` and `apiGatewayManaged: true`. Quick-create creates a route API Gateway manages and that route stays readable.
Emulator: 404 - the managed route cannot be read at all.
Why: a client that inspects what quick-create provisioned (to re-target or delete it) cannot find the route the same call just reported.
Fix: the quick-create route materialization / route lookup for api-gateway-managed routes.
Traces: apigatewayv2-quick-create-managed-resources#task#6.

Cause 2 of 6: UpdateRoute on a quick-create managed route.
AWS: 201 with the updated managed route body (`routeKey: ANY /managed`, `apiGatewayManaged: true`, target unchanged).
Emulator: 404 - the update cannot be applied.
Why: a client cannot adjust the route quick-create made for it.
Fix: as cause 1, plus the UpdateRoute lookup path.
Traces: apigatewayv2-quick-create-managed-resources#task#12.

Cause 3 of 6: GetStage still reports route settings after DeleteRouteSettings.
AWS: once the per-route settings for a route key are deleted, the stage's `routeSettings` map no longer contains that key.
Emulator: `routeSettings["GET /items"]` is still present with its throttling values - the delete was acknowledged but not applied.
Why: a client that removes a throttle believes it is gone while the emulator keeps enforcing/reporting it.
Fix: the DeleteRouteSettings state mutation on the stage's routeSettings map. Related open issue: `20261027T053233Z-apigatewayv2-deleteroutesettings-stub.md` tracks the unimplemented stub; this cluster is the observable stage-state consequence and carries its own replay key.
Traces: apigatewayv2-http-api-route-config-cleanup#task#10.

Cause 4 of 6: DeleteProductRestEndpointPage is not idempotent.
AWS: deleting a rest-endpoint page that is already gone answers 204 with an empty body.
Emulator: 404 with `{"__type":"NotFoundException","message":"ProductRestEndpointPage not found"}`.
Why: teardown and retry paths that delete twice fail against the emulator where they succeed on AWS.
Fix: the DeleteProductRestEndpointPage absent-resource branch.
Traces: apigatewayv2-product-rest-endpoint-page-error-paths#task#6.

Cause 5 of 6: GetProductRestEndpointPage never leaves AVAILABLE and omits statusException.
AWS: a rest-endpoint page whose endpoint cannot be reached settles `status: FAILED` and carries a `statusException` structure with `exception` and `message` members describing the failure.
Emulator: `status: AVAILABLE` and no `statusException`.
Why: a client polling for a terminal status sees a healthy page that was never actually resolvable, and has no diagnostic to surface.
Fix: the ProductRestEndpointPage status state machine (endpoint resolution -> FAILED + statusException).
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#7.

Cause 6 of 6: UpdateProductRestEndpointPage reports a terminal status immediately.
AWS: the update answers 200 with `status: IN_PROGRESS` - the page re-resolves asynchronously.
Emulator: `status: AVAILABLE` straight away.
Why: a client that waits for IN_PROGRESS to clear never observes the transition and cannot tell a completed re-resolve from one that never ran.
Fix: as cause 5 - the status transition on update.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#8.

## Evidence
Replay verdict `mismatch`. Details: GetRoute `status: expected 200, got 404` with `$.routeKey: missing (expected '$default')` and `$.apiGatewayManaged: missing (expected True)`; UpdateRoute `status: expected 201, got 404`; GetStage `$.routeSettings.GET /items: extra (expected None, got {'throttlingBurstLimit': 100, 'throttlingRateLimit': 50.0})`; DeleteProductRestEndpointPage `status: expected 204, got 404` with body `{'__type': 'NotFoundException', 'message': 'ProductRestEndpointPage not found'}`; GetProductRestEndpointPage `$.status: value_mismatch (expected 'FAILED', got 'AVAILABLE')` and `$.statusException: missing`; UpdateProductRestEndpointPage `$.status: value_mismatch (expected 'IN_PROGRESS', got 'AVAILABLE')`.

## Steps to Reproduce
Replay the corpus. Cause 1: POST /v2/apis with a quick-create target, then GET /v2/apis/{apiId}/routes/{routeId} for the managed route (fixture apigatewayv2-quick-create-managed-resources step task#6) - 404. Cause 3: put route settings on a stage, delete them, then GET the stage (apigatewayv2-http-api-route-config-cleanup#task#10) - routeSettings still carries the key.
