Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-portal-product-error-paths
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-portal-product-error-paths apigatewayv2-product-page-error-paths apigatewayv2-route-settings-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|DeletePortalProduct|2xx->4xx|$
Issue-Key: apigateway|DeletePortalProduct|2xx->4xx|status
Issue-Key: apigateway|DeleteRouteSettings|4xx->4xx|$.message
Issue-Commit: 418fa7a6b82984f3652fde3052f2e7dd1ee98ae8

# apigateway deletes of absent child resources refuse instead of succeeding and use emulator-authored not-found text

> _Note: this issue was identified against the code base at commit `418fa7a6b82984f3652fde3052f2e7dd1ee98ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePortalProduct

## Description
Two clusters where a delete aimed at a child resource that is not there diverges from AWS: one refuses a delete AWS accepts, the other refuses with text AWS does not use. Both causes below must be closed.

Cause 1 of 2: DeletePortalProduct is not idempotent.
AWS: Smithy: DeletePortalProduct http {method DELETE, uri /v2/portalproducts/{PortalProductId}, code 204}, output Unit, modeled errors AccessDeniedException(403)/BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429). Recorded behavior: deleting a portal product that is already gone answers 204 with an empty body — the absent-resource case is a success, not a NotFoundException.
Emulator: 404 with body `{"__type":"NotFoundException","message":"Product not found in account 000000000000 with id cetidi75uc"}`. In that message only `Product not found in account ` and ` with id ` are fixed emulator-authored text; the account id comes from the caller's credentials and the id from the request's PortalProductId label — AWS emits no message here at all, because it does not refuse.
Why: teardown and retry paths that issue the delete twice (or race a concurrent delete) succeed against AWS and fail against the emulator, so a client's cleanup aborts with an error on a resource that is already in the desired state. The same fault was already reported for DeleteProductRestEndpointPage and DeletePortal in `20261029T220031Z-apigateway-managed-routes-route-settings-deletion-and-repeat.md` and `20261029T220031Z-apigateway-portal-lifecycle-and-representation-preview-hostedzoneid-publishstatus.md`; DeletePortalProduct is a separate handler with its own replay key.
Fix: the DeletePortalProduct handler's absent-resource branch — return the modeled 204 with an empty body instead of raising NotFoundException.
Traces: apigatewayv2-portal-product-error-paths#task#8 (also apigatewayv2-product-page-error-paths#task#8).

Cause 2 of 2: DeleteRouteSettings' not-found refusal carries emulator-authored text.
AWS: Smithy: DeleteRouteSettings modeled error NotFoundException (httpError 404, member `message`), input RouteKey a required httpLabel. Recorded behavior: deleting route settings for a route key that has none answers 404 with the message `The settings for the Route with key 'GET /no-such-route' does not exist`. Fixed text: `The settings for the Route with key '` ... `' does not exist`; the only variable part is the route key, taken verbatim from the request's RouteKey path label.
Emulator: the status 404 is right but the message is `Invalid route settings specified` — a sentence AWS never emits, naming neither the resource kind nor the key.
Why: a client that distinguishes "these settings do not exist" from "the key you sent is malformed" by matching the message (or that surfaces the key to an operator) cannot tell the two apart, and error-message assertions written against AWS fail. Related open issue `20261029T220031Z-apigateway-notfoundexception-messages-do-not-match-aws-wording.md` covers other operations' wording and does not carry this key.
Fix: the DeleteRouteSettings handler's not-found refusal — render AWS's sentence with the request's route key interpolated.
Traces: apigatewayv2-route-settings-lifecycle#task#7.

## Evidence
Replay verdict `mismatch`. Cause 1: `status: expected 204, got 404` and `$: value_mismatch (expected {}, got {'__type': 'NotFoundException', 'message': 'Product not found in account 000000000000 with id cetidi75uc'})` — AWS returns an empty body, the emulator an error envelope. Cause 2: `$.message: value_mismatch (expected "The settings for the Route with key 'GET /no-such-route' does not exist", got 'Invalid route settings specified')`, status 404 expected and 404 returned.

## Steps to Reproduce
Replay the corpus. Cause 1: create a portal product, DELETE /v2/portalproducts/{portalProductId}, then DELETE the same id again (fixture apigatewayv2-portal-product-error-paths step task#8) — the second delete answers 404 NotFoundException instead of 204 with an empty body. Cause 2: create an HTTP API with a stage, then DELETE /v2/apis/{apiId}/stages/{stageName}/routesettings/GET%20%2Fno-such-route for a route key that has no settings (apigatewayv2-route-settings-lifecycle#task#7) — the 404 body says 'Invalid route settings specified'.
