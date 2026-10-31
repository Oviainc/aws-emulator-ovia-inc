Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-stage-route-settings-bogus-child-not-found
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-stage-route-settings-bogus-child-not-found
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|DeleteRouteSettings|4xx->4xx|$.message
Issue-Key: apigatewayv2|DeleteRouteSettings|2xx->2xx|$headers.content-length
Issue-Commit: 418fa7a6b82984f3652fde3052f2e7dd1ee98ae8

# apigatewayv2 DeleteRouteSettings not-found message and 204 framing diverge

> _Note: this issue was identified against the code base at commit `418fa7a6b82984f3652fde3052f2e7dd1ee98ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRouteSettings

## Description
Two clusters on DeleteRouteSettings as reached through the apigatewayv2 service token: the refusal text for an absent route key, and the framing of the successful 204. Both causes below must be closed.

Cause 1 of 2: the not-found refusal carries emulator-authored text.
AWS: Smithy: DeleteRouteSettings http {method DELETE, uri /v2/apis/{ApiId}/stages/{StageName}/routesettings/{RouteKey}, code 204}, modeled error NotFoundException (httpError 404, member `message`); RouteKey is a required httpLabel. Recorded behavior: deleting route settings for a route key that has none answers 404 with the message `The settings for the Route with key 'GET /no-such-route' does not exist`. Fixed text: `The settings for the Route with key '` ... `' does not exist`; the single variable part is the route key, copied from the request's RouteKey path label.
Emulator: status 404 is correct but the message is `Invalid route settings specified`, a sentence AWS never emits — it names neither the resource kind nor the offending key.
Why: a client that branches on the message to tell "no settings exist for this key" from "the key is malformed", or that surfaces the key to an operator, gets no usable information, and assertions written against AWS's wording fail. The open issue `20261029T220031Z-apigatewayv2-request-validation-and-refusal-messages-diverge-from.md` covers other apigatewayv2 refusal wording and does not carry this replay key.
Fix: the DeleteRouteSettings handler's not-found refusal path — render AWS's sentence with the request's route key interpolated.
Traces: apigatewayv2-stage-route-settings-bogus-child-not-found#task#7.

Cause 2 of 2: the successful 204 omits the Content-Length header.
AWS: Smithy: DeleteRouteSettings output shape Unit with http code 204 (no body). Recorded behavior only for the header: the recorded 204 carries `Content-Length: 0`, an explicit zero-length body framing.
Emulator: status 204 is correct but the `content-length` header is absent from the response.
Why: a client or intermediary that frames the body from Content-Length must fall back to connection-close semantics or report a malformed response, so a successful delete can surface as a transport error and keep-alive reuse is lost.
Fix: the shared REST-JSON response serializer for empty-body (204) responses — emit `Content-Length: 0` instead of omitting the header.
Traces: apigatewayv2-stage-route-settings-bogus-child-not-found#task#8.

## Evidence
Replay verdict `mismatch`. Cause 1: `$.message: value_mismatch (expected "The settings for the Route with key 'GET /no-such-route' does not exist", got 'Invalid route settings specified')` with expected status 404 and actual status 404. Cause 2: `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)` with expected status 204 and actual status 204.

## Steps to Reproduce
Replay the corpus. Cause 1: create an HTTP API and a stage, then DELETE /v2/apis/{apiId}/stages/{stageName}/routesettings/{routeKey} for a route key that has no settings (fixture apigatewayv2-stage-route-settings-bogus-child-not-found step task#7) — the 404 body carries 'Invalid route settings specified'. Cause 2: put per-route settings on the stage and DELETE them (same fixture, step task#8) — the 204 arrives with no content-length header.
