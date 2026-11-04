Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-sstr-traj-http-lambda-integration-request-forms
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-sstr-traj-http-lambda-integration-request-forms apigatewayv2-sstr-traj-route-target-forms apigatewayv2-sstr-traj-quick-create-then-solve-sh-steps
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateIntegration|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateIntegration|4xx->2xx|status
Issue-Key: apigatewayv2|CreateRoute|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateRoute|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateRoute|4xx->4xx|status
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|$.apiKeyRequired
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|$.authorizationType
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|$.routeId
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|$.routeKey
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|$.target
Issue-Key: apigatewayv2|CreateRoute|2xx->4xx|status
Issue-Key: apigatewayv2|GetRoutes|2xx->2xx|$.items[*].routeId
Issue-Key: apigatewayv2|GetRoutes|2xx->2xx|$.items[*].target
Issue-Key: apigatewayv2|GetRoutes|2xx->2xx|$.items[*].apiGatewayManaged
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# apigatewayv2 route and integration validation and representation diverge

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
The HTTP-API integration and route surface accepts input AWS refuses, rewords the refusals it does make, and mis-serializes route members. 6 causes.

Cause 1 of 6: CreateIntegration's missing-PayloadFormatVersion refusal is reworded (fixture apigatewayv2-sstr-traj-http-lambda-integration-request-forms, step task#2; cluster also covers task#3 and task#4).
AWS: Smithy CreateIntegration — POST /v2/apis/{ApiId}/integrations, BadRequestException(400) modeled; PayloadFormatVersion targets StringWithLengthBetween1And64 and is optional in the model but required for HTTP APIs. Recorded 400 message: 'PayloadFormatVersion is a required parameter for integration' — fixed text, no variable parts.
Emulator: 400 with 'PayloadFormatVersion is required for HTTP APIs.' — different sentence (and a trailing period AWS does not send).
Why: the status is right but clients and tests that match the AWS sentence do not recognise the refusal.
Fix: CreateIntegration's validation layer — use AWS's sentence verbatim.

Cause 2 of 6: CreateIntegration accepts a malformed integrationUri ARN (same fixture, step task#5).
AWS: 400 BadRequestException with message 'Invalid ARN specified in the request' and the error type in x-amzn-errortype. Fixed text, no variable parts. IntegrationUri targets UriWithLengthBetween1And2048.
Emulator: answers 201 and creates the integration, sending no message and no x-amzn-errortype.
Why: an integration pointing at an unparseable ARN is persisted, so the API looks deployable and the failure surfaces only later (or never) instead of at create time; a client's error path for a bad ARN never runs.
Fix: CreateIntegration's validation — parse integrationUri as an ARN for AWS_PROXY/AWS integration types and refuse with BadRequestException.

Cause 3 of 6: CreateRoute does not validate the route target, so the bogus-target refusal never happens (fixture apigatewayv2-sstr-traj-route-target-forms, step task#4; cluster also covers task#5).
AWS: Smithy CreateRoute — POST /v2/apis/{ApiId}/routes, BadRequestException(400) modeled; Target names an existing integration as 'integrations/<integrationId>'. Recorded 400 message: 'Unable to find route target of type integrations with ID abc1234' — fixed text 'Unable to find route target of type ' + the target type + ' with ID ' + the integration id taken from the request's target member (the id in the recorded sentence is that request's value, not part of the template).
Emulator: 409 ConflictException with 'Route with key GET /capture already exists for this API' and x-amzn-errortype ConflictException — i.e. it skipped target validation, created the route for the bogus target on an earlier attempt, and now reports a route-key conflict.
Why: a client that references a nonexistent integration gets a route that cannot route, and the conflict it is told about is not the problem it has. The wrong error class also sends retry/create-or-update logic down the wrong branch.
Fix: CreateRoute's handler/validation — resolve target 'integrations/<id>' against the API's integrations before insert and refuse with BadRequestException and AWS's sentence.

Cause 4 of 6: the subsequent valid CreateRoute is refused 409 (same fixture, step task#6).
AWS: 201 with routeId, routeKey, authorizationType 'NONE', apiKeyRequired false and target 'integrations/<integrationId>'.
Emulator: 409, no body members, because the route created by cause 3's unvalidated request still occupies the route key.
Why: the state left behind by the missing validation makes a legitimate route creation impossible — the API ends up with no usable route.
Fix: same component as cause 3 (CreateRoute validation must not leave a route behind for a request AWS refuses); re-replay this step to confirm.

Cause 5 of 6: GetRoutes serializes target without the 'integrations/' prefix (same fixture, step task#7).
AWS: Route.Target is returned in the same form it is supplied, 'integrations/<integrationId>' (recorded response); Smithy Route.Target targets StringWithLengthBetween1And128.
Emulator: returns the bare integration id with the 'integrations/' type prefix stripped. (The routeId mismatch reported on the same step is a consequence of cause 4 — the expected id is the one the refused-then-conflicted create would have minted — and is not a separate requirement on id values.)
Why: a client round-tripping a route into UpdateRoute/CreateRoute, or rendering the target, gets a value that is not a valid route target and must guess the type prefix.
Fix: the route serializer — emit target as '<type>/<id>'.

Cause 6 of 6: quick-create routes are not flagged as managed (fixture apigatewayv2-sstr-traj-quick-create-then-solve-sh-steps, step task#3).
AWS: Smithy Route.ApiGatewayManaged — 'Specifies whether a route is managed by API Gateway. If you created an API using quick create, the $default route is managed by API Gateway. You can't modify the $default route key.' The recorded GetRoutes item has apiGatewayManaged true.
Emulator: returns apiGatewayManaged false for the quick-create $default route.
Why: clients use this flag to decide a route is API-Gateway-owned and must not be modified or deleted; false invites a modification AWS refuses, and tooling that filters managed resources mis-reports the API's route set.
Fix: the quick-create path in CreateApi/route state must mark the generated $default route managed, and the route serializer must carry the flag.

Related but non-owning open issues: 20261029T220031Z-apigatewayv2-request-validation-and-refusal-messages-diverge-from.md, 20261029T220031Z-apigateway-managed-routes-route-settings-deletion-and-repeat.md, 20261029T193654Z-apigateway-api-gateway-control-plane-is-over-permissive-6.md — none carries these replay keys.

## Evidence
Replay divergence clusters (service=apigatewayv2):
- CreateIntegration 400/400, '$.message: value_mismatch (expected "PayloadFormatVersion is a required parameter for integration", got "PayloadFormatVersion is required for HTTP APIs.")' — apigatewayv2-sstr-traj-http-lambda-integration-request-forms#task#2 (+task#3, task#4).
- CreateIntegration 'status: expected 400, got 201'; '$.message: missing (expected "Invalid ARN specified in the request")'; '$headers.x-amzn-errortype: missing' — same fixture#task#5.
- CreateRoute 'status: expected 400, got 409'; '$.message: value_mismatch (expected "Unable to find route target of type integrations with ID abc1234", got "Route with key GET /capture already exists for this API")'; '$headers.x-amzn-errortype: shape_mismatch (got ConflictException)' — apigatewayv2-sstr-traj-route-target-forms#task#4 (+task#5).
- CreateRoute 'status: expected 201, got 409' with $.routeId/$.routeKey ('GET /capture')/$.authorizationType ('NONE')/$.apiKeyRequired (false)/$.target ('integrations/ur7ayid') all missing — same fixture#task#6.
- GetRoutes 200/200, '$.items[0].target: value_mismatch (expected "integrations/ur7ayid", got "ur7ayid")' (plus the cascading $.items[0].routeId mismatch) — same fixture#task#7.
- GetRoutes 200/200, '$.items[0].apiGatewayManaged: value_mismatch (expected True, got False)' — apigatewayv2-sstr-traj-quick-create-then-solve-sh-steps#task#3.

## Steps to Reproduce
1. Replay fixture apigatewayv2-sstr-traj-http-lambda-integration-request-forms: task#2 POST /v2/apis/{apiId}/integrations without payloadFormatVersion (message wording differs); task#5 POST with a malformed integrationUri ARN (emulator 201, AWS 400 'Invalid ARN specified in the request').
2. Replay fixture apigatewayv2-sstr-traj-route-target-forms: task#4 POST /v2/apis/{apiId}/routes with target 'integrations/<nonexistent-id>' (emulator creates it, then answers 409; AWS answers 400 'Unable to find route target of type integrations with ID <id>'); task#6 POST the same routeKey with the real integration target (emulator 409 instead of 201); task#7 GET /v2/apis/{apiId}/routes (target returned without the 'integrations/' prefix).
3. Replay fixture apigatewayv2-sstr-traj-quick-create-then-solve-sh-steps: task#3 GET /v2/apis/{apiId}/routes for a quick-create API (apiGatewayManaged false instead of true).
