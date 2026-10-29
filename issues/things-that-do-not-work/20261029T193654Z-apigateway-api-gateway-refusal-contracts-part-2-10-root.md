Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.delete-method-error-contract-no-bad-request apigateway.put-method-invalid-http-method-fails apigateway.delete-root-resource-rejected apigateway.get-sdk-type-not-found apigateway.get-sdk-type-unknown-id-rejected apigateway.op-contract-getSdkType apigateway.get-usage-plan-key-not-found-vs-badrequest apigateway.op-contract-getUsagePlanKey apigateway.import-api-keys-invalid-format-rejected apigateway.import-rest-api-invalid-openapi-fails apigateway.put-rest-api-malformed-body-rejected apigateway.op-contract-putMethodResponse apigateway.put-gateway-response-invalid-response-parameters-rejected apigateway.put-integration-invalid-type-fails apigateway.put-integration-response-invalid-content-handling-rejected apigateway.put-method-response-invalid-status-code-rejected apigateway.put-rest-api-invalid-mode-fails apigateway.create-documentation-part-invalid-location-type-rejected apigateway.put-integration-invalid-uri-scheme-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.delete-method-error-contract-no-bad-request|run#9|AssertionError|assert message == "Invalid HttpMethod specified. Valid options are GET,PUT,POST,DELETE,PATCH,OPTIONS,HEAD,ANY", f"unexpected message: {message!r}"
Issue-Key: apigateway.put-method-invalid-http-method-fails|run#6|AssertionError|assert message == (
Issue-Key: apigateway.delete-root-resource-rejected|run#12|AssertionError|assert message == "Root resource cannot be deleted", message
Issue-Key: apigateway.get-sdk-type-not-found|run#11|AssertionError|assert message == f"No SDK generator for '{bogus_id}'", f"unexpected message: {message!r}"
Issue-Key: apigateway.get-sdk-type-unknown-id-rejected|run#8|AssertionError|assert message == f"No SDK generator for '{bogus_id}'", f"unexpected message: {message!r}"
Issue-Key: apigateway.op-contract-getSdkType|run#11|AssertionError|assert message == "No SDK generator for 'not-a-real-sdk-type'", f"unexpected message: {message!r}"
Issue-Key: apigateway.get-usage-plan-key-not-found-vs-badrequest|run#16|AssertionError|assert unknown_plan_message == "Invalid API Key identifier specified", (
Issue-Key: apigateway.op-contract-getUsagePlanKey|run#21|AssertionError|assert message == (
Issue-Key: apigateway.import-api-keys-invalid-format-rejected|run#18|AssertionError|assert message == "Invalid API Key import content specified for format: csv", (
Issue-Key: apigateway.import-rest-api-invalid-openapi-fails|run#13|AssertionError|assert message == "Invalid OpenAPI input.", f"unexpected rejection message {message!r}"
Issue-Key: apigateway.put-rest-api-malformed-body-rejected|run#11|AssertionError|assert message == "Unable to determine OAS/Swagger version", f"unexpected message: {message!r}"
Issue-Key: apigateway.op-contract-putMethodResponse|run#15|AssertionError|assert message == (
Issue-Key: apigateway.put-gateway-response-invalid-response-parameters-rejected|run#7|AssertionError|assert message == (
Issue-Key: apigateway.put-integration-invalid-type-fails|run#7|AssertionError|assert message == (
Issue-Key: apigateway.put-integration-response-invalid-content-handling-rejected|run#10|AssertionError|assert message == (
Issue-Key: apigateway.put-method-response-invalid-status-code-rejected|run#10|AssertionError|assert message == (
Issue-Key: apigateway.put-rest-api-invalid-mode-fails|run#15|AssertionError|assert message == (
Issue-Key: apigateway.create-documentation-part-invalid-location-type-rejected|run#7|AssertionError|assert message == (
Issue-Key: apigateway.put-integration-invalid-uri-scheme-rejected|run#8|AssertionError|assert message == (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway refusal contracts, part 2: 10 root causes (method, resource, SDK type, usage-plan key, import, method-response, gateway-response and ValidationException enum ordering)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
This issue bundles 10 further NET-NEW root causes where an API Gateway refusal message diverges from AWS's. A fixer must close all 10 before submitting.

Cause 1 of 10: invalid HTTP method on PutMethod/DeleteMethod.
AWS: BadRequestException 'Invalid HttpMethod specified. Valid options are GET,PUT,POST,DELETE,PATCH,OPTIONS,HEAD,ANY' — entirely fixed text; the offending verb is NOT included and the verb list is comma-separated in that order.
Emulator: 'Invalid HTTP method specified: NOTAVERB123; expected one of ANY, DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT' and 'Invalid HTTP method specified: FETCH; expected one of ...' — different wording, alphabetised list, and it echoes the verb.
Why: clients matching the documented text cannot classify the refusal.
Fix: the httpMethod validation message shared by PutMethod/DeleteMethod.
Traces: apigateway.delete-method-error-contract-no-bad-request, apigateway.put-method-invalid-http-method-fails

Cause 2 of 10: deleting the root resource.
AWS: BadRequestException 'Root resource cannot be deleted' (fixed text).
Emulator: 'The root resource cannot be deleted; delete the RestApi instead'.
Why: the wording is not AWS's.
Fix: DeleteResource's root-resource guard message.
Traces: apigateway.delete-root-resource-rejected

Cause 3 of 10: GetSdkType / GetSdk with an unknown SDK type id.
AWS: NotFoundException "No SDK generator for '<id>'" — fixed text "No SDK generator for '" + the id from the request + "'". Smithy: GetSdkType (GET /sdktypes/{id}) declares NotFoundException (404) and BadRequestException (400).
Emulator: 'Invalid SDK type identifier specified: sim2real-not-a-real-sdktype' / '... : not-a-real-sdk-type'.
Why: the documented refusal text is never produced.
Fix: GetSdkType/GetSdkTypes not-found message.
Traces: apigateway.get-sdk-type-not-found, apigateway.get-sdk-type-unknown-id-rejected, apigateway.op-contract-getSdkType

Cause 4 of 10: GetUsagePlanKey not-found messages are swapped.
AWS: an unknown usagePlanId refuses with NotFoundException 'Invalid Usage Plan ID specified'; an unknown keyId refuses with NotFoundException 'Invalid API Key identifier specified'. Both are fixed text.
Emulator: emits 'Invalid Usage Plan ID specified' where 'Invalid API Key identifier specified' is required and 'Invalid API Key identifier specified' where 'Invalid Usage Plan ID specified' is required — the two messages are attached to the wrong conditions.
Why: a client cannot tell whether the plan or the key was not found.
Fix: GetUsagePlanKey's lookup-order and not-found message selection.
Traces: apigateway.get-usage-plan-key-not-found-vs-badrequest, apigateway.op-contract-getUsagePlanKey

Cause 5 of 10: ImportApiKeys with a body that is not valid CSV.
AWS: BadRequestException 'Invalid API Key import content specified for format: csv' — fixed text ending in the requested format.
Emulator: 'body must be a UTF-8 encoded CSV document carrying the API keys to import'.
Why: the documented refusal text is never produced.
Fix: ImportApiKeys' body validation message.
Traces: apigateway.import-api-keys-invalid-format-rejected

Cause 6 of 10: OpenAPI body-parse refusals on ImportRestApi/PutRestApi.
AWS: ImportRestApi with an undiscernible document refuses with BadRequestException 'Invalid OpenAPI input.' (fixed text, with the trailing period); PutRestApi with a body whose version cannot be determined refuses with BadRequestException 'Unable to determine OAS/Swagger version' (fixed text).
Emulator: both return 'Invalid OAS input'.
Why: the two distinct AWS refusals collapse into one non-AWS message, so a client cannot distinguish a malformed document from an undetectable version.
Fix: the OpenAPI body parser's refusal messages in ImportRestApi and PutRestApi.
Traces: apigateway.import-rest-api-invalid-openapi-fails, apigateway.put-rest-api-malformed-body-rejected

Cause 7 of 10: PutMethodResponse conflict message.
AWS: ConflictException 'Response already exists for this resource' (fixed text). Smithy: PutMethodResponse declares ConflictException (409).
Emulator: 'Method response already exists for this method'.
Why: the wording is not AWS's.
Fix: PutMethodResponse's conflict message.
Traces: apigateway.op-contract-putMethodResponse

Cause 8 of 10: PutGatewayResponse with an invalid responseParameters key.
AWS: BadRequestException 'Invalid mapping expression specified: Validation Result: warnings : [], errors : [Invalid mapping expression specified: <key>]' — fixed text with the offending key from the request substituted twice as shown.
Emulator: 'Invalid mapping expression specified: invalid.param.name; a gateway response parameter must match gatewayresponse.header.{name} or gatewayresponse.headers.{name}, where name is a valid header name'.
Why: the structured Validation Result shape AWS returns for mapping expressions is absent.
Fix: the mapping-expression validation message used by PutGatewayResponse.
Traces: apigateway.put-gateway-response-invalid-response-parameters-rejected

Cause 9 of 10: ValidationException 'enum value set' lists are emitted in the wrong order.
AWS: constraint violations are reported as '1 validation error detected: Value '<v>' at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]' with AWS's own value order — PutIntegration.type as [HTTP, MOCK, AWS_PROXY, HTTP_PROXY, AWS]; PutIntegrationResponse.contentHandling as [CONVERT_TO_TEXT, CONVERT_TO_BINARY]; PutRestApi.mode as [overwrite, merge]. The path is also AWS's: 'statusCode' and 'mode', not 'putMethodResponseInput.statusCode' / 'putRestApiInput.mode'. Smithy: PutMethodResponse.statusCode carries pattern ^[1-5]\\d\\d$, and AWS reports it as 'Member must satisfy regular expression pattern: [1-5]\\d\\d' at path 'statusCode'.
Emulator: emits the model's declaration order ([HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY], [CONVERT_TO_BINARY, CONVERT_TO_TEXT], [merge, overwrite], [API, AUTHORIZER, MODEL, RESOURCE, METHOD, ...]) and prefixes the member path with the input-shape name.
Why: any client asserting on the validation message — including AWS's own documented examples — sees a different string for every constrained member.
Fix: the shared ValidationException message builder (value ordering and member-path naming).
Traces: apigateway.put-integration-invalid-type-fails, apigateway.put-integration-response-invalid-content-handling-rejected, apigateway.put-method-response-invalid-status-code-rejected, apigateway.put-rest-api-invalid-mode-fails, apigateway.create-documentation-part-invalid-location-type-rejected

Cause 10 of 10: PutIntegration with a non-HTTP(S) URI for type HTTP.
AWS: BadRequestException 'Invalid HTTP endpoint specified for URI' (fixed text, the URI is not echoed). Smithy: PutIntegration declares BadRequestException (400) and models uri as String.
Emulator: 'Invalid integration URI specified: an integration of type HTTP takes a fully formed HTTP(S) URL, got ftp://example.com/resource'.
Why: the wording is not AWS's and echoes the URI AWS omits.
Fix: PutIntegration's uri validation message.
Traces: apigateway.put-integration-invalid-uri-scheme-rejected

## Evidence
Representative replay output (verbatim):
- 'AssertionError: unexpected message: 'Invalid HTTP method specified: NOTAVERB123; expected one of ANY, DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT'' against 'Invalid HttpMethod specified. Valid options are GET,PUT,POST,DELETE,PATCH,OPTIONS,HEAD,ANY' (Cause 1)
- 'AssertionError: The root resource cannot be deleted; delete the RestApi instead' against 'Root resource cannot be deleted' (Cause 2)
- 'AssertionError: unexpected message: 'Invalid SDK type identifier specified: sim2real-not-a-real-sdktype'' against "No SDK generator for '<id>'" (Cause 3)
- 'AssertionError: unexpected unknown-usagePlanId message: 'Invalid Usage Plan ID specified'' and 'unexpected NotFoundException message: 'Invalid API Key identifier specified'' (Cause 4)
- 'AssertionError: unexpected rejection message 'body must be a UTF-8 encoded CSV document carrying the API keys to import'' against 'Invalid API Key import content specified for format: csv' (Cause 5)
- 'AssertionError: unexpected rejection message 'Invalid OAS input'' against 'Invalid OpenAPI input.'; 'unexpected message: 'Invalid OAS input'' against 'Unable to determine OAS/Swagger version' (Cause 6)
- 'AssertionError: unexpected ConflictException message: 'Method response already exists for this method'' against 'Response already exists for this resource' (Cause 7)
- 'AssertionError: unexpected BadRequestException message: 'Invalid mapping expression specified: invalid.param.name; a gateway response parameter must match gatewayresponse.header.{name} ...'' against 'Invalid mapping expression specified: Validation Result: warnings : [], errors : [Invalid mapping expression specified: invalid.param.name]' (Cause 8)
- 'AssertionError: unexpected ValidationException message: "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'putIntegrationInput.type' failed to satisfy constraint: Member must satisfy enum value set: [HTTP, AWS, MOCK, HTTP_PROXY, AWS_PROXY]"' against '... at 'putIntegrationInput.type' ... [HTTP, MOCK, AWS_PROXY, HTTP_PROXY, AWS]'; same shape for contentHandling, statusCode and mode (Cause 9)
- 'AssertionError: unexpected BadRequestException message: 'Invalid integration URI specified: an integration of type HTTP takes a fully formed HTTP(S) URL, got ftp://example.com/resource'' against 'Invalid HTTP endpoint specified for URI' (Cause 10)

## Steps to Reproduce
1. PutMethod with httpMethod='FETCH' and DeleteMethod with httpMethod='NOTAVERB123' (Cause 1); DeleteResource on the root resource id (Cause 2).
2. GetSdkType with an unknown id (Cause 3); GetUsagePlanKey with an unknown usagePlanId and with an unknown keyId (Cause 4).
3. ImportApiKeys with a non-CSV body (Cause 5); ImportRestApi with an unparsable document and PutRestApi with a JSON body that is neither Swagger nor OpenAPI (Cause 6).
4. PutMethodResponse twice for the same statusCode (Cause 7); PutGatewayResponse with responseParameters={'invalid.param.name': "'foo'"} (Cause 8).
5. PutIntegration with type='NOT_A_REAL_TYPE', PutIntegrationResponse with contentHandling='NOT_A_REAL_CONTENT_HANDLING', PutMethodResponse with statusCode='not-a-status', PutRestApi with mode='sideways', CreateDocumentationPart with location.type='NOT_A_REAL_LOCATION_TYPE' (Cause 9).
6. PutIntegration with type='HTTP' and uri='ftp://example.com/resource' (Cause 10).
