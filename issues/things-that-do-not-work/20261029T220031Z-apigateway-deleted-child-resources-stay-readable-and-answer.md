Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-integration-response-errors
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-integration-response-errors apigatewayv2-integration-response-lifecycle apigatewayv2-model-error-paths apigatewayv2-model-lifecycle apigatewayv2-portal-product-error-paths apigatewayv2-portal-product-lifecycle apigatewayv2-portal-with-included-product apigatewayv2-portal-product-page-errors apigatewayv2-portal-product-page-lifecycle apigatewayv2-product-page-lifecycle apigatewayv2-product-page-error-paths apigatewayv2-route-response-errors apigatewayv2-route-response-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetIntegrationResponse|4xx->2xx|$
Issue-Key: apigateway|GetIntegrationResponse|4xx->2xx|$.message
Issue-Key: apigateway|GetIntegrationResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetIntegrationResponse|4xx->2xx|status
Issue-Key: apigateway|GetModel|4xx->2xx|$.message
Issue-Key: apigateway|GetModel|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetModel|4xx->2xx|status
Issue-Key: apigateway|GetPortalProduct|4xx->2xx|$.message
Issue-Key: apigateway|GetPortalProduct|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetPortalProduct|4xx->2xx|status
Issue-Key: apigateway|GetProductPage|4xx->2xx|$.message
Issue-Key: apigateway|GetProductPage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetProductPage|4xx->2xx|status
Issue-Key: apigateway|GetProductPage|4xx->2xx|$
Issue-Key: apigateway|GetRouteResponse|4xx->2xx|$
Issue-Key: apigateway|GetRouteResponse|4xx->2xx|$.message
Issue-Key: apigateway|GetRouteResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|GetRouteResponse|4xx->2xx|status
Issue-Key: apigateway|UpdateRouteResponse|4xx->2xx|$.message
Issue-Key: apigateway|UpdateRouteResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateRouteResponse|4xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway deleted child resources stay readable and answer 200 instead of 404

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIntegrationResponse

## Description
After a successful DELETE, the emulator keeps serving the deleted resource: the subsequent read answers 200 with the full body where AWS answers 404 NotFoundException. The delete is acknowledged but never applied to the store. Nine causes, one per affected read operation / resource type.

Cause 1 of 9: GetIntegrationResponse after DeleteIntegrationResponse (polled).
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `IntegrationResponse with ID <integrationResponseId> is not present in integration with ID <integrationId>` - fixed text `IntegrationResponse with ID ` / ` is not present in integration with ID `, with both ids taken from the request path.
Emulator: 200 with the integration-response body; the replay polled 56 times and the resource never disappeared.
Why: a client that confirms deletion by reading until 404 never converges, and a recreate-after-delete flow sees a stale resource.
Fix: the DeleteIntegrationResponse handler's state mutation (the integration-response store).
Traces: apigatewayv2-integration-response-errors#task#9.

Cause 2 of 9: GetIntegrationResponse after delete (non-polled).
AWS: 404 NotFoundException with the same message template as cause 1.
Emulator: 200 with the body.
Why/Fix: as cause 1.
Traces: apigatewayv2-integration-response-lifecycle#task#9.

Cause 3 of 9: GetModel after DeleteModel.
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `Invalid model identifier specified: <modelId>` - fixed text `Invalid model identifier specified: ` plus the model id from the request path.
Emulator: 200 with the model body.
Why/Fix: the DeleteModel handler's state mutation.
Traces: apigatewayv2-model-error-paths#task#5.

Cause 4 of 9: GetPortalProduct after DeletePortalProduct.
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `Product not found in account <accountId> with id <portalProductId>` - fixed text `Product not found in account ` / ` with id `, with the account id from the caller's credentials and the product id from the request path.
Emulator: 200 with the product body.
Why/Fix: the DeletePortalProduct handler's state mutation.
Traces: apigatewayv2-portal-product-error-paths#task#9.

Cause 5 of 9: GetProductPage after DeleteProductPage.
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `ProductPage not found` - the whole sentence is fixed text with no variable parts.
Emulator: 200 with the product-page body.
Why/Fix: the DeleteProductPage handler's state mutation.
Traces: apigatewayv2-portal-product-page-errors#task#4.

Cause 6 of 9: GetProductPage after a repeat delete (polled).
AWS: 404 NotFoundException, message `ProductPage not found`.
Emulator: 200 with the body; polled 56 times without converging.
Why/Fix: as cause 5.
Traces: apigatewayv2-product-page-error-paths#task#7.

Cause 7 of 9: GetRouteResponse after DeleteRouteResponse (polled).
AWS: 404 with the message `Invalid route response specified` - the whole sentence is fixed text.
Emulator: 200 with the route-response body; polled 56 times without converging.
Why/Fix: the DeleteRouteResponse handler's state mutation.
Traces: apigatewayv2-route-response-errors#task#9.

Cause 8 of 9: GetRouteResponse after delete (non-polled).
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `Invalid route response specified`.
Emulator: 200 with the body.
Why/Fix: as cause 7.
Traces: apigatewayv2-route-response-lifecycle#task#13.

Cause 9 of 9: UpdateRouteResponse on a deleted route response.
AWS: 404 with x-amzn-errortype `NotFoundException` and the message `Invalid route response specified`.
Emulator: 200 - the update is applied to a resource that should not exist.
Why: a client's update silently succeeds against a deleted resource, so state the client believes is gone is resurrected.
Fix: the UpdateRouteResponse handler's existence check, plus the delete state mutation of cause 7.
Traces: apigatewayv2-route-response-lifecycle#task#14.

## Evidence
Replay verdict `mismatch` with `status: expected 404, got 200` on all nine clusters and `$headers.x-amzn-errortype: missing (expected 'NotFoundException')`. Three clusters additionally report `$: poll: status 200 not in [404] after 56 attempt(s)`, with the full live resource body returned each time.

## Steps to Reproduce
Replay the corpus. Cause 1: create an integration and an integration response, DELETE the integration response, then GET it (fixture apigatewayv2-integration-response-errors step task#9) - the emulator keeps answering 200 with the body. Repeat per listed fixture#step.
