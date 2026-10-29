Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-default-gateway-response-override
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-default-gateway-response-override apigateway-gateway-response-custom-lifecycle apigateway-gateway-response-customization-lifecycle apigateway-gateway-response-default-override apigateway-gateway-response-error-paths apigateway-gateway-response-full-crud-lifecycle apigateway-gateway-response-lifecycle apigateway-gateway-response-multiple-types apigateway-gateway-response-multiple-types-and-errors apigateway-gateway-responses-lifecycle apigateway-gateway-responses-listing-and-notfound-errors apigateway-gatewayresponse-cascade-delete-rest-api apigateway-gatewayresponse-error-invalid-statuscode apigateway-gatewayresponse-lifecycle apigw-patch-gateway-response-statuscode-op-support apigateway-gateway-response-multiple-types-isolation apigateway-gatewayresponse-multiple-types apigateway-isolated-multi-response-type-customization apigateway-list-gateway-responses apigateway-gateway-response-patch-update-semantics
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetGatewayResponse|2xx->2xx|$.responseParameters
Issue-Key: apigateway|GetGatewayResponse|2xx->2xx|$.responseTemplates
Issue-Key: apigateway|PutGatewayResponse|2xx->2xx|$.responseTemplates
Issue-Key: apigateway|PutGatewayResponse|2xx->2xx|$.responseParameters
Issue-Key: apigateway|UpdateGatewayResponse|2xx->2xx|$.responseParameters
Issue-Key: apigateway|UpdateGatewayResponse|2xx->2xx|$.responseTemplates
Issue-Key: apigateway|GetGatewayResponses|2xx->2xx|$.item[*]
Issue-Related: 20261029T200808Z-apigateway-api-gateway-patchoperations-grammar-op-enum-precedence-and.md
Issue-Commit: e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a

# API Gateway GatewayResponse representation and refusals (10 causes)

> _Note: this issue was identified against the code base at commit `e22fc32f4dd53ee37fe3b4cc7fa000d8c822ec8a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetGatewayResponse

## Description
The GatewayResponse surface drops two modeled members from every representation, serves a default catalogue whose content and order are not AWS's, and accepts a remove AWS refuses. Smithy: GatewayResponse = {responseType (GatewayResponseType enum, 21 values), statusCode (pattern ^[1-5]\\d\\d$), responseParameters (MapOfStringToString), responseTemplates (MapOfStringToString), defaultResponse (Boolean, default false)}; PutGatewayResponse is 201, Get/UpdateGatewayResponse are 200, NotFoundException is @httpError 404.

Causes 1-7 share one fault: responseParameters and responseTemplates are omitted from the serialised GatewayResponse. AWS always emits them - responseParameters as `{}` when none are configured, and responseTemplates carrying API Gateway's default body template `{"application/json": "{\"message\":$context.error.messageString}"}` for any response the caller has not given its own template. Emulator: both members are absent. Why: a client reading a gateway response cannot tell whether parameters/templates are configured, and a read-modify-write of a gateway response silently drops the default template. Fix: the GatewayResponse serialiser (and the default-template seeding in gateway-response state).
  Cause 1 of 10 - GetGatewayResponse, both members missing. Trace: apigateway-default-gateway-response-override#task#2 (26 steps in cluster).
  Cause 2 of 10 - PutGatewayResponse, responseTemplates missing. Trace: apigateway-default-gateway-response-override#task#3.
  Cause 3 of 10 - GetGatewayResponse, responseTemplates missing. Trace: apigateway-default-gateway-response-override#task#4.
  Cause 4 of 10 - PutGatewayResponse, responseParameters missing. Trace: apigateway-gateway-response-default-override#task#3 (14 steps).
  Cause 5 of 10 - GetGatewayResponse, responseParameters missing. Trace: apigateway-gateway-response-default-override#task#4.
  Cause 6 of 10 - PutGatewayResponse, both missing. Trace: apigateway-gateway-response-error-paths#task#4.
  Cause 7 of 10 - UpdateGatewayResponse, both missing. Trace: apigw-patch-gateway-response-statuscode-op-support#task#6.

Cause 8 of 10 - GetGatewayResponses default catalogue. AWS returns all 21 default gateway responses, each with defaultResponse=true, responseParameters {} and the default responseTemplates entry, in API Gateway's own order (the recording starts INTEGRATION_FAILURE/504, RESOURCE_NOT_FOUND/404, REQUEST_TOO_LARGE/413, THROTTLED/429, UNSUPPORTED_MEDIA_TYPE/415, AUTHORIZER_CONFIGURATION_ERROR/500, ...). Emulator returns the types sorted alphabetically (ACCESS_DENIED/403 first) and without responseParameters/responseTemplates. Why: a client that pages or indexes the catalogue positionally reads a different response than AWS would return. Fix: the default gateway-response catalogue and its ordering in GetGatewayResponses. Trace: apigateway-gateway-response-customization-lifecycle#task#6 (5 steps).

Cause 9 of 10 - the same catalogue defect after customisation, where the customised entry must appear in AWS's position with its own statusCode. Trace: apigateway-gateway-response-multiple-types#task#4 (11 steps).

Cause 10 of 10 - UpdateGatewayResponse removing a responseParameter that is not configured. AWS: 404 NotFoundException `Invalid parameter name specified` (fixed text). Emulator: 200, a silent no-op. Why: a client is told a removal succeeded that never happened. Fix: UpdateGatewayResponse's remove-op validation. Trace: apigateway-gateway-response-patch-update-semantics#task#7.

No open issue owns these clusters.

## Evidence
Causes 1-7: $.responseParameters missing (expected {}) and/or $.responseTemplates missing (expected {'application/json': '{"message":$context.error.messageString}'}) on 200/201 responses that otherwise matched.
Cause 8: $.item[0] expected {defaultResponse: true, responseParameters: {}, responseTemplates: {'application/json': '{"message":$context.error.messageString}'}, responseType: 'INTEGRATION_FAILURE', statusCode: '504'}, got {responseType: 'ACCESS_DENIED', statusCode: '403', defaultResponse: true}; 21 item slots diverge.
Cause 9: same, with the customised ACCESS_DENIED entry misplaced.
Cause 10: status expected 404 with $.message 'Invalid parameter name specified' and x-amzn-errortype NotFoundException, got 200.

## Steps to Reproduce
1. Replay the corpus (replay_wire_corpus).
2. Causes 1-7: PUT/GET/PATCH a gateway response (e.g. fixture apigateway-default-gateway-response-override, steps task#2-task#4) and check that responseParameters and responseTemplates are present.
3. Cause 8/9: GET /restapis/{id}/gatewayresponses on a fresh API and compare the item list's order and per-item members (apigateway-gateway-response-customization-lifecycle#task#6).
4. Cause 10: PATCH a gateway response with op=remove on an unset responseParameter (apigateway-gateway-response-patch-update-semantics#task#7).
