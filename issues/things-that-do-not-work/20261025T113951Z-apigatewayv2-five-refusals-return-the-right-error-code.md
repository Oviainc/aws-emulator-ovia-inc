Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.create-model-with-an-invalid-schema-rejected apigatewayv2.create-stage-duplicate-name-rejected apigatewayv2.http-proxy-payload-format-version-2-rejected apigatewayv2.protocol-type-illegal-attributes-rejected apigatewayv2.update-route-to-jwt-without-an-authorizer-rejected
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.create-model-with-an-invalid-schema-rejected|run#6|AssertionError|assert message == (
Issue-Key: apigatewayv2.create-stage-duplicate-name-rejected|run#8|AssertionError|assert message == "Stage already exists", f"unexpected message {message!r}"
Issue-Key: apigatewayv2.http-proxy-payload-format-version-2-rejected|run#10|AssertionError|assert message == PFV2_HTTP_PROXY_MESSAGE, (
Issue-Key: apigatewayv2.protocol-type-illegal-attributes-rejected|run#18|AssertionError|assert err == (
Issue-Key: apigatewayv2.update-route-to-jwt-without-an-authorizer-rejected|run#7|AssertionError|assert message == (
Issue-Commit: 9ff0151e93167cf00d59909edc96bc1d4e3529be

# apigatewayv2: five refusals return the right error code with a message that diverges from AWS's recorded text (5 causes)

> _Note: this issue was identified against the code base at commit `9ff0151e93167cf00d59909edc96bc1d4e3529be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIntegration

## Description
Five independent refusals whose HTTP status and error code already MATCH AWS (400/BadRequestException, 409/ConflictException per the model) but whose Error.Message is not the text AWS produces for that rule. Each check asserts the message AWS was recorded emitting for the rule, and each one is a distinct message site in a distinct handler, so this is five causes, not one. Evidence that these texts are treated as requirements by the emulator itself: in cause 4 the sibling leg of the SAME check (CreateModel on an HTTP API) already returns AWS's exact text "Currently, Models are not permitted for APIs with a protocol type of HTTP" and passes — only the MOCK-integration leg diverges. Related but NOT covering this: open issue 20261025T041541Z-apigatewayv2-refusal-messages-do-not-name-the-blocking.md (different operations/message sites). A fixer must close all five before submitting.

Cause 1 of 5: CreateModel with a schema that is not valid JSON.
AWS: Smithy: CreateModel errors BadRequestException (client, httpError 400); Schema targets StringWithLengthBetween0And32K. Recorded behavior only for the text: AWS answers BadRequestException with the fixed message "Invalid model specified: Validation Result: warnings : [], errors : [Invalid model schema specified]" — a constant for the whole malformed-schema class, naming nothing from the request.
Emulator: BadRequestException (correct) with 'Invalid model specified: Schema is not valid JSON: key must be a string at line 1 column 2' — the emulator's own JSON parser diagnostic, i.e. internal implementation detail (parser, line/column) surfaced to the caller instead of AWS's constant validation-result text.
Why: a client that matches or logs AWS's validation-result text does not recognise the refusal, and the emulator exposes parser internals that vary with the input and with the parser version, so nothing stable can be asserted.
Fix: CreateModel's schema validation message in the operation's validation layer.
Traces: apigatewayv2.create-model-with-an-invalid-schema-rejected.

Cause 2 of 5: CreateStage with a StageName the API already has.
AWS: Smithy: CreateStage errors ConflictException (client, httpError 409). Recorded behavior only for the text: the message is the constant "Stage already exists".
Emulator: ConflictException (correct) with 'Stage with name prod already exists for this API.' — a message built from the request value instead of AWS's constant.
Why: callers that branch on the conflict text see an unrecognised message, and the emulator advertises a different (request-derived) message contract than AWS for the same rule.
Fix: the duplicate-name conflict message in the CreateStage handler.
Traces: apigatewayv2.create-stage-duplicate-name-rejected.

Cause 3 of 5: PayloadFormatVersion 2.0 on an HTTP_PROXY integration (CreateIntegration and UpdateIntegration).
AWS: Smithy: CreateIntegration/UpdateIntegration errors BadRequestException (client, httpError 400); PayloadFormatVersion targets StringWithLengthBetween1And64 and is cross-validated against IntegrationType. Recorded behavior: the refusal message is built from the request as "PayloadFormatVersion <value> is not supported for integration of type <IntegrationType>" — recorded here as "PayloadFormatVersion 2.0 is not supported for integration of type HTTP_PROXY", and the same text is emitted on both create and update.
Emulator: BadRequestException (correct) with 'PayloadFormatVersion 2.0 is not supported for an integration of type HTTP_PROXY; supported values are 1.0' — same facts, different template ("for an integration of type" plus an extra supported-values clause).
Why: the message template is part of the refusal contract clients key on; a differing template means a caller cannot distinguish this cross-field refusal from any other BadRequestException by message.
Fix: the PayloadFormatVersion/IntegrationType cross-field validation message shared by the CreateIntegration and UpdateIntegration handlers.
Traces: apigatewayv2.http-proxy-payload-format-version-2-rejected (asserted on both the create and the update leg; the code, the no-orphan-integration effect and the AWS_PROXY 2.0 control all pass).

Cause 4 of 5: CreateIntegration(IntegrationType=MOCK) on an HTTP (non-WEBSOCKET) API.
AWS: Smithy: CreateIntegration errors BadRequestException (client, httpError 400); IntegrationType is an enum gated by the API's ProtocolType. Recorded behavior: the message is the constant "Currently, an API with a protocol type of HTTP may only be associated with proxy integrations (AWS_PROXY, HTTP_PROXY)", i.e. it names the allowed set rather than the rejected value.
Emulator: BadRequestException (correct) with 'Currently, IntegrationType MOCK is not permitted for APIs with a protocol type of HTTP' — the rejected value instead of the permitted set.
Why: the caller loses the actionable half of AWS's message (which integration types ARE allowed), and the text does not match the one AWS emits for the per-protocol attribute allowlist.
Fix: the ProtocolType attribute-allowlist validation message in the CreateIntegration handler (align with the already-correct CreateModel-on-HTTP message in the same validation layer).
Traces: apigatewayv2.protocol-type-illegal-attributes-rejected (failed at the MOCK leg; the CreateModel-on-HTTP leg before it passed with AWS's exact text).

Cause 5 of 5: UpdateRoute to AuthorizationType=JWT with no AuthorizerId.
AWS: Smithy: UpdateRoute errors BadRequestException (client, httpError 400); AuthorizationType is an enum and AuthorizerId targets Id. Recorded behavior: the message is the constant "Invalid authorizer ID specified. Setting the authorization type to JWT requires a valid JWT authorizer."
Emulator: BadRequestException (correct) with 'AuthorizerId is required for a route whose AuthorizationType is JWT'.
Why: AWS's text states the rule as an invalid/missing authorizer reference (the same message covers a bad id as well as none); the emulator's narrower "required" phrasing is a different contract for the same refusal.
Fix: the JWT authorizer-reference validation message in the UpdateRoute handler.
Traces: apigatewayv2.update-route-to-jwt-without-an-authorizer-rejected (the code and the "route unchanged" effect both pass).

All five message sites live under services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/ (create_model.rs, create_stage.rs, create_integration.rs, update_integration.rs, update_route.rs).

## Evidence
Status/code agree with AWS in all five; only Error.Message diverges.
1. CreateModel(Schema="{not json") — expected BadRequestException "Invalid model specified: Validation Result: warnings : [], errors : [Invalid model schema specified]"; actual BadRequestException 'Invalid model specified: Schema is not valid JSON: key must be a string at line 1 column 2'.
2. CreateStage(StageName="prod") twice — expected ConflictException "Stage already exists"; actual ConflictException 'Stage with name prod already exists for this API.'
3. CreateIntegration/UpdateIntegration(HTTP_PROXY, PayloadFormatVersion="2.0") — expected BadRequestException "PayloadFormatVersion 2.0 is not supported for integration of type HTTP_PROXY"; actual BadRequestException 'PayloadFormatVersion 2.0 is not supported for an integration of type HTTP_PROXY; supported values are 1.0'.
4. CreateIntegration(IntegrationType=MOCK) on an HTTP API — expected BadRequestException "Currently, an API with a protocol type of HTTP may only be associated with proxy integrations (AWS_PROXY, HTTP_PROXY)"; actual BadRequestException 'Currently, IntegrationType MOCK is not permitted for APIs with a protocol type of HTTP'.
5. UpdateRoute(AuthorizationType="JWT") with no AuthorizerId — expected BadRequestException "Invalid authorizer ID specified. Setting the authorization type to JWT requires a valid JWT authorizer."; actual BadRequestException 'AuthorizerId is required for a route whose AuthorizationType is JWT'.

## Steps to Reproduce
1. CreateApi(ProtocolType=WEBSOCKET, RouteSelectionExpression="$request.body.action"); CreateModel(ApiId=<id>, Name="Order", ContentType="application/json", Schema="{not json") and read Error.Message.
2. CreateApi(ProtocolType=HTTP); CreateStage(StageName="prod"); CreateStage(StageName="prod") again and read Error.Message.
3. On an HTTP API: CreateIntegration(IntegrationType=HTTP_PROXY, IntegrationUri=https://example.com/backend, IntegrationMethod=GET, PayloadFormatVersion="1.0"); then CreateIntegration(... PayloadFormatVersion="2.0") and UpdateIntegration(<good id>, PayloadFormatVersion="2.0"); read Error.Message on both.
4. On an HTTP API: CreateIntegration(IntegrationType=MOCK) and read Error.Message.
5. On an HTTP API: CreateRoute(RouteKey="GET /orders"); UpdateRoute(RouteId=<id>, AuthorizationType="JWT") with no AuthorizerId and read Error.Message.
Replay: run_behavioral_suite and re-run the five check ids in trace_ids.
