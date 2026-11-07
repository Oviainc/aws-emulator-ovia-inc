Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-gateway-response-patch-update-semantics
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-gateway-response-patch-update-semantics apigateway-error-paths-invalid-references apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|$.message
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigateway|UpdateGatewayResponse|4xx->2xx|status
Issue-Key: apigateway|PutMethod|4xx->4xx|$.message
Issue-Key: apigateway|PutIntegration|4xx->4xx|$.message
Issue-Commit: 3971b4aa426682b1b07d577a2cfc2e75e38df2fe

# API Gateway REST refusals: UpdateGatewayResponse accepts an invalid patch path (200), and PutMethod / PutIntegration refusal messages diverge (3 causes)

> _Note: this issue was identified against the code base at commit `3971b4aa426682b1b07d577a2cfc2e75e38df2fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateGatewayResponse

## Description
Three net-new root causes, all apigateway, all in the refusal path of REST control-plane writes: one refusal is missing entirely and two carry the wrong message text.

Cause 1 of 3: UpdateGatewayResponse applies a patch naming an unknown parameter instead of refusing it with 404.
AWS: Smithy: UpdateGatewayResponse (PATCH /restapis/{restApiId}/gatewayresponses/{responseType}) takes `patchOperations` and models NotFoundException (httpError 404) among its errors; its output GatewayResponse exposes responseType, statusCode, responseParameters, responseTemplates, defaultResponse. A patch operation whose path does not name a patchable parameter of a gateway response is refused with 404 NotFoundException carrying the message "Invalid parameter name specified" — the message is entirely fixed text with no variable parts — and the error-type header x-amzn-errortype: NotFoundException. No part of the patch is applied.
Emulator: the request is answered 200 with a GatewayResponse body: `status: expected 404, got 200`, `$.message: missing (expected 'Invalid parameter name specified')`, `$headers.x-amzn-errortype: missing (expected 'NotFoundException')`. The unknown patch path is accepted silently.
Why: a client that mistypes a gateway-response patch path (or targets a parameter the service does not support) is told the update succeeded while nothing it asked for was configured, so the API's error responses keep their previous shape and the misconfiguration is invisible. Clients that branch on 404/NotFoundException to validate a patch before rollout take the success branch.
Fix: UpdateGatewayResponse's patch-operation validation in its handler — reject unknown patch paths with 404 NotFoundException and the fixed sentence above before touching state.
Traces: apigateway-gateway-response-patch-update-semantics#task#7.

Cause 2 of 3: PutMethod's 404 names the wrong entity and appends identifiers AWS does not emit.
AWS: Smithy: PutMethod (PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}) models NotFoundException (httpError 404). When the method's target resource cannot be resolved, the refusal is 404 with the message "Invalid Resource identifier specified" — fixed text in full, with NO interpolated identifier, account id or restApiId.
Emulator: the status is right (404) but the body says "Invalid API identifier specified <accountId>:<restApiId>" — it blames the API identifier rather than the Resource identifier, and it appends an "<account>:<id>" pair that AWS's sentence for this class does not contain at all.
Why: a client (or a human reading a CLI/SDK error) is pointed at the wrong request field: it will re-check and re-send the restApiId when the resource reference is what is wrong. Error-message matching in deployment tooling and test suites keyed on AWS's sentence does not match, and the leaked "<accountId>:<restApiId>" pair is response content AWS never produces.
Fix: PutMethod's reference-resolution/validation ordering and its NotFoundException message construction — report the unresolved entity actually being validated, with the fixed sentence and no interpolation.
Traces: apigateway-error-paths-invalid-references#task#3.

Cause 3 of 3: PutIntegration's enum-constraint validation message lists the IntegrationType values in the wrong order.
AWS: recorded behavior only for the ordering — Smithy: PutIntegration input member `type` targets the IntegrationType enum and PutIntegration models BadRequestException (httpError 400); the on-disk model declares the enum in the order [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY], but the recorded service response enumerates the allowed set as [HTTP, MOCK, AWS_PROXY, HTTP_PROXY, AWS]. The refusal message is the standard framework sentence: fixed text "1 validation error detected: Value '", the rejected value from the request, fixed text "' at '", the member path (here putIntegrationInput.type), fixed text "' failed to satisfy constraint: Member must satisfy enum value set: [", the allowed values in AWS's order, "]". Only the rejected value and the member path vary with the request; the allowed-value list and its ORDER are fixed service text.
Emulator: status 400 is correct and the sentence structure is correct, but the allowed-value list is emitted in the model's declaration order — "...enum value set: [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]" instead of AWS's "[HTTP, MOCK, AWS_PROXY, HTTP_PROXY, AWS]".
Why: the message is part of the contract — clients, snapshot tests and documentation that compare the validation sentence byte-for-byte against the real service fail against the emulator even though the request was rejected for the right reason.
Fix: the serializer/validation-message builder that renders enum-constraint violations for apigateway — the allowed-value list for IntegrationType must be emitted in the service's own order rather than the model's declaration order.
Traces: apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture apigateway-gateway-response-patch-update-semantics, step task#7, UpdateGatewayResponse, expected 404 / actual 200:
  status: expected 404, got 200
  $.message: missing (expected 'Invalid parameter name specified', got None)
  $headers.x-amzn-errortype: missing (expected 'NotFoundException', got None)
  Message is fixed text in full; no variable parts.

Cause 2 — fixture apigateway-error-paths-invalid-references, step task#3, PutMethod, expected 404 / actual 404:
  $.message: value_mismatch (expected 'Invalid Resource identifier specified', got 'Invalid API identifier specified <accountId>:<restApiId>')
  Expected message is fixed text with no interpolation; the emulator's trailing '<accountId>:<restApiId>' pair has no counterpart in AWS's sentence.

Cause 3 — fixture apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms, step task#5, PutIntegration, expected 400 / actual 400:
  $.message: value_mismatch (expected "1 validation error detected: Value '<rejected value>' at 'putIntegrationInput.type' failed to satisfy constraint: Member must satisfy enum value set: [HTTP, MOCK, AWS_PROXY, HTTP_PROXY, AWS]", got "... enum value set: [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]")
  Variable parts: the rejected value and the member path, both from the request. The bracketed allowed-value list and its order are fixed service text (and differ from the Smithy declaration order [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]).

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: CreateRestApi, then PATCH a gateway response (UpdateGatewayResponse on any responseType) with a patchOperation whose path names a parameter the gateway response does not have. The emulator answers 200 with a GatewayResponse body; AWS answers 404 NotFoundException 'Invalid parameter name specified' with x-amzn-errortype: NotFoundException.
3. Cause 2: call PutMethod with a resourceId (and restApiId) that do not resolve. The emulator answers 404 'Invalid API identifier specified <accountId>:<restApiId>'; AWS answers 404 'Invalid Resource identifier specified'.
4. Cause 3: call PutIntegration with type set to a lowercase/invalid value (e.g. 'aws_proxy'). Both answer 400, but compare the 'enum value set: [...]' ordering in the message.
5. Or replay the fixture steps directly: apigateway-gateway-response-patch-update-semantics#task#7, apigateway-error-paths-invalid-references#task#3, apigateway-sstr-traj-rest-api-lambda-proxy-uri-forms#task#5.
