Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-create-routing-rule-missing-condition-rejected
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-create-routing-rule-missing-condition-rejected apigatewayv2-routing-rule-missing-condition-rejected apigatewayv2-domain-routing-rule-bogus-child-and-bounds apigatewayv2-get-apis-max-results-validation apigatewayv2-http-route-key-grammar apigatewayv2-websocket-route-key-grammar apigatewayv2-websocket-deployment-requires-route-targets apigatewayv2-websocket-model-route-response-status-codes apigatewayv2-websocket-route-selection-expression-required
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateRoutingRule|4xx->4xx|$.message
Issue-Key: apigatewayv2|ListRoutingRules|2xx->4xx|$.routingRules
Issue-Key: apigatewayv2|ListRoutingRules|2xx->4xx|status
Issue-Key: apigatewayv2|GetApis|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateRoute|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateRoute|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateRoute|4xx->2xx|status
Issue-Key: apigatewayv2|CreateRoute|2xx->2xx|$.routeId
Issue-Key: apigatewayv2|CreateRoute|2xx->2xx|$.routeKey
Issue-Key: apigatewayv2|CreateDeployment|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateDeployment|4xx->2xx|status
Issue-Key: apigatewayv2|GetRouteResponse|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateApi|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateApi|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateApi|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateApi|4xx->2xx|status
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigatewayv2 request validation and refusal messages diverge from AWS

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoutingRule

## Description
Ten clusters where a refusal is missing, inverted, or worded differently from AWS. The status, the error code and the message text are all part of the contract, so each cause states AWS's template with its fixed text and the parts taken from the request.

Cause 1 of 10: CreateRoutingRule with a condition that is neither matchBasePaths nor matchHeaders.
AWS: 400 with the message `Invalid conditions for RoutingRule. Each condition can only be either matchBasePaths or matchHeaders.` - both sentences are fixed text, with no index or member name interpolated. Smithy: RoutingRuleCondition has exactly the members MatchBasePaths (jsonName matchBasePaths) and MatchHeaders (jsonName matchHeaders).
Emulator: 400 with `Conditions[0] must specify exactly one of: MatchBasePaths, MatchHeaders.` - emulator-authored text that interpolates an index and uses the Smithy member names rather than the wire names.
Why: a client that surfaces or matches AWS's text never matches.
Fix: the CreateRoutingRule condition-validation message.
Traces: apigatewayv2-create-routing-rule-missing-condition-rejected#task#9.

Cause 2 of 10: ListRoutingRules on a domain with no routing rules.
AWS: 200 with `routingRules: []` - an empty collection, not an error.
Emulator: 400.
Why: a client enumerating a domain's routing rules cannot distinguish "none configured" from a bad request, and a list-then-act loop fails on every empty domain.
Fix: the ListRoutingRules handler - return the empty collection instead of refusing.
Traces: apigatewayv2-domain-routing-rule-bogus-child-and-bounds#task#5.

Cause 3 of 10: GetApis with a non-integer maxResults.
AWS: 400 with the message `maxResults must be an integer.` - the whole sentence is fixed text; AWS does NOT echo the offending value.
Emulator: `Invalid value for MaxResults: '<value>'. MaxResults must be a positive integer.`
Why/Fix: the shared maxResults query-parameter validation message used by the collection handlers.
Traces: apigatewayv2-get-apis-max-results-validation#task#1.

Cause 4 of 10: CreateRoute with a routeKey that does not match the protocol's grammar.
AWS: 400 with an x-amzn-errortype error header and the message `The provided route key is not formatted properly for HTTP protocol. Format should be "[HTTP METHOD] /[RESOURCE PATH]" or "$default"` - the whole text is fixed (the protocol word tracks the api's protocolType).
Emulator: 201 - the malformed route key is accepted and a route is created.
Why: an unroutable route is created and the failure surfaces only at the data plane.
Fix: CreateRoute's routeKey grammar validation.
Traces: apigatewayv2-http-route-key-grammar#task#2.

Cause 5 of 10: CreateRoute does not normalise the route key's HTTP method.
AWS: the stored and echoed `routeKey` has the method upper-cased - a request for `get /x` comes back as `GET /x`.
Emulator: echoes `get /x` verbatim.
Why: the route key is the data-plane match key; a lower-case method means the route never matches, and duplicate detection against an existing `GET /x` fails.
Fix: the routeKey normalisation in CreateRoute (upper-case the method segment).
Traces: apigatewayv2-http-route-key-grammar#task#6.

Cause 6 of 10: CreateRoute with a duplicate route key.
AWS: 409 with an x-amzn-errortype error header and the message `Route with key <routeKey> already exists for this API` - fixed text `Route with key ` / ` already exists for this API` plus the route key from the request body.
Emulator: 201 - a second route with the same key is created.
Why: two routes share a match key, so which one serves a request is undefined.
Fix: CreateRoute's duplicate-route-key check.
Traces: apigatewayv2-http-route-key-grammar#task#7.

Cause 7 of 10: CreateDeployment with a route that has no target.
AWS: 400 with an x-amzn-errortype error header and the message `A route target (typically an Integration) must be defined for route <routeId> with key <routeKey>` - fixed text `A route target (typically an Integration) must be defined for route ` / ` with key `, with the route id and route key taken from the api's state.
Emulator: 201 - the deployment succeeds with an unroutable route.
Why: a deployment AWS would refuse is published, so the api is deployed in a state that cannot serve the orphan route.
Fix: CreateDeployment's pre-deployment route-target validation.
Traces: apigatewayv2-websocket-deployment-requires-route-targets#task#6.

Cause 8 of 10: GetRouteResponse with an unknown route-response id.
AWS: 404 with the message `Invalid route response specified` - the whole sentence is fixed text (lower-case `route response`).
Emulator: `Invalid RouteResponse identifier specified`.
Why/Fix: the GetRouteResponse not-found message.
Traces: apigatewayv2-websocket-model-route-response-status-codes#task#7.

Cause 9 of 10: CreateApi for a WEBSOCKET api with a missing/invalid routeSelectionExpression.
AWS: 400 with the message `Invalid routeSelectionExpression` - the whole phrase is fixed text, with no member name or hint appended.
Emulator: `RouteSelectionExpression is required for WebSocket APIs.`
Why/Fix: the CreateApi routeSelectionExpression validation message.
Traces: apigatewayv2-websocket-route-selection-expression-required#task#1.

Cause 10 of 10: CreateApi with an unsupported variable in routeSelectionExpression.
AWS: 400 with an x-amzn-errortype error header and the message `Unexpected variable in selection expression: <variable>` - fixed text `Unexpected variable in selection expression: ` plus the offending variable from the submitted expression.
Emulator: 201 - the api is created with a selection expression that can never evaluate.
Why: a WebSocket api is created whose route selection cannot resolve, so no route ever matches.
Fix: CreateApi's routeSelectionExpression variable whitelist.
Traces: apigatewayv2-websocket-route-selection-expression-required#task#2.

## Evidence
Replay verdict `mismatch`. Details include `$.message: value_mismatch (expected 'Invalid conditions for RoutingRule. Each condition can only be either matchBasePaths or matchHeaders.', got 'Conditions[0] must specify exactly one of: MatchBasePaths, MatchHeaders.')`; ListRoutingRules `status: expected 200, got 400` with `$.routingRules: missing (expected [])`; CreateRoute `status: expected 400, got 201` with `$.message: missing (expected 'The provided route key is not formatted properly for HTTP protocol...')`; `$.routeKey: value_mismatch (expected 'GET /x', got 'get /x')`; CreateRoute duplicate `status: expected 409, got 201`; CreateDeployment `status: expected 400, got 201`; CreateApi `status: expected 400, got 201` for the selection-expression variable.

## Steps to Reproduce
Replay the corpus. Cause 2: create a domain name then GET /v2/domainnames/{domainName}/routingrules (fixture apigatewayv2-domain-routing-rule-bogus-child-and-bounds step task#5) - 400 instead of 200 with an empty routingRules list. Cause 6: POST the same routeKey twice to /v2/apis/{apiId}/routes (apigatewayv2-http-route-key-grammar#task#7) - 201 instead of 409.
