Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses apigateway.e2e-usage-plan-key-throttle-consequence apigateway.authorizer-deletion-blocked-while-referenced-rejected apigateway.model-invalid-schema-json-rejected apigateway.op-contract-delete-model
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses|run#28|AssertionError|assert b_status == 400, (
Issue-Key: apigateway.e2e-usage-plan-key-throttle-consequence|_get_status#27|Timeout|with urllib.request.urlopen(req, timeout=10) as resp:
Issue-Key: apigateway.authorizer-deletion-blocked-while-referenced-rejected|run#16|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: apigateway.model-invalid-schema-json-rejected|run#8|AssertionError|assert message == (
Issue-Key: apigateway.op-contract-delete-model|run#13|AssertionError|assert err.get("Code") == "ConflictException", f"expected ConflictException, got {err.get('Code')!r}"
Issue-Commit: 3bfaf82953d93db9f40ee26ed0fb0b9c83dbff8e

# API Gateway: 7 net-new root causes — request validators are never enforced at a deployed stage, usage-plan throttle never answers 429, ImportDocumentationParts ignores failOnWarnings, and three refusals diverge in code or message

> _RESOLVED for five of the seven causes (1, 3, 5, 6, 7), whose recorded checks now pass; their keys and trace ids are the ones this report still carries. Causes 2 and 4 are NOT closed and were split out, with their keys, their trace ids and everything the fix established about them, into `issues/things-that-do-not-work/20261027T101902Z-apigateway-2-causes-of-the-request-validator-issue-remain-open.md` — so no key is carried by both reports._

> _Note: this issue was identified against the code base at commit `3bfaf82953d93db9f40ee26ed0fb0b9c83dbff8e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRequestValidator

## Description
Seven independent root causes in the apigateway emulator, each with its own failing check ids. All seven must be closed before this issue is submitted. These are net-new: the ALREADY FILED / OWNED markers in this replay cover 148 other failures, and none of the issues named below carries any of these seven causes.

Cause 1 of 7: a method's request validator is not applied at the deployed stage, so a body that violates the declared model is forwarded to the integration and answered 200.
AWS: a Method that names a requestValidatorId whose RequestValidator has validateRequestBody=true (Smithy: CreateRequestValidator.validateRequestBody/validateRequestParameters, @default false; the validator id is carried on PutMethod.requestValidatorId) refuses a request whose body fails the method's requestModels JSON Schema BEFORE the integration runs. The refusal is the BAD_REQUEST_BODY gateway response: HTTP 400, header x-amzn-ErrorType: BadRequestException, body {"message":"Invalid request body"} — fixed text, no request-derived parts. A missing required query-string parameter under validateRequestParameters is the BAD_REQUEST_PARAMETERS response, HTTP 400, body {"message":"Missing required request parameters: [<parameter names>]"}, where only the bracketed parameter-name list comes from the request's method.request.querystring.* declarations.
Emulator: the stage admits the request and runs the integration. Recorded: a POST whose JSON body omits the model's required property, with the required query parameter present, got HTTP 200 and the integration's own body (the recording shows the MOCK integration's response document) instead of 400 {"message":"Invalid request body"}.
Why: a client that relies on API Gateway to screen malformed payloads sees them reach the backend; negative tests that assert a 400 BAD_REQUEST_BODY pass against AWS and fail here, and an over-permissive stage silently accepts data AWS rejects.
Fix: the execute-api data-plane request pipeline for a deployed stage — the request-validation step that resolves the method's requestValidatorId and requestModels before integration dispatch, and the BAD_REQUEST_BODY / BAD_REQUEST_PARAMETERS gateway-response serializer.
Traces: apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses

Cause 2 of 7: a POST method that declares requestModels plus a required querystring parameter never serves a fully valid request at the deployed stage, so the four validator modes can never be exercised.
AWS: with a validator in any of the four modes (body-only, params-only, both, none), a request that satisfies everything the method declares is served by the integration with HTTP 200 and the mapped integration response.
Emulator: the check never completes. It deploys four routes (body/params/both/none), then polls POST <invoke-url>/<route> with a valid body and the tenant query parameter until it answers 200 (its own budget is 150 s at 3 s intervals); no assertion is ever reached and the harness cuts the check at its 60 s cap. recorded behavior only: the replay output is "Check timed out after 60.0s" with no response captured, so the only established fact is that the positive control never reached 200 inside the budget. Hypothesis: a method carrying both requestModels and requestParameters is not servable at the stage (it answers a non-200 or does not answer), which is a distinct failure from Cause 1, where a validator-bearing route did answer 200.
Why: every declared-vs-enforced distinction in request validation (and the per-mode messages "Invalid request body" vs "Missing required request parameters: [tenant]") is unreachable for a client, and the mode matrix cannot be relied on at all.
Fix: the execute-api data-plane dispatch for a method whose integration request declares requestModels and requestParameters together (same pipeline as Cause 1, but the positive path).
Traces: apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare

Cause 3 of 7: a usage plan's throttle (rateLimit/burstLimit) is never enforced at the deployed stage, so a hammered api-key-protected method never gets a 429.
AWS: a usage plan with throttle {burstLimit: 1, rateLimit: 1} associated with an apiStage, whose API key is attached via CreateUsagePlanKey, admits the first request and then refuses the burst with HTTP 429, header x-amzn-ErrorType: TooManyRequestsException and body {"message":"Too Many Requests"} (fixed text; the plan's numeric limits come from the plan, not the message).
Emulator: no 429 is ever produced. The check hammers the invoke URL with the valid x-api-key in batches of five for up to 90 s waiting to observe both a 200 and a 429, so with the 429 never arriving it runs past the harness 60 s cap: recorded result "Check timed out after 60.0s". recorded behavior only for the limiter's existence: no throttled response was captured at all.
Why: a client cannot test or rely on usage-plan rate limiting; load that AWS sheds is accepted here, and the whole plan-level quota/throttle consequence (and the plan-delete cascade this check asserts after it) is unverifiable.
Fix: the execute-api stage request path's usage-plan throttle accounting (per-key rate/burst token bucket) and its 429 gateway response. Related but distinct, both open and neither covering the plan-level rate/burst limit: 20261025T081755Z-apigateway-api-gateway-a-usage-plan-s-day-quota.md (DAY quota) and 20261027T004324Z-apigateway-api-gateway-per-method-usage-plan-throttle-overrides.md (per-method overrides).
Traces: apigateway.e2e-usage-plan-key-throttle-consequence

Cause 4 of 7: ImportDocumentationParts ignores failOnWarnings, so a warning-producing payload is committed instead of refused.
AWS: Smithy ImportDocumentationParts (PUT /restapis/{restApiId}/documentation/parts, failOnWarnings @httpQuery failonwarnings, @default false; output DocumentationPartIds carries ids and warnings; errors BadRequestException 400). With mode=merge and failOnWarnings=true, an import whose parts produce a warning is refused with 400 BadRequestException and nothing is created — the import is atomic.
Emulator: the same call SUCCEEDED. Recorded: "expected a ClientError, but the call SUCCEEDED" for import_documentation_parts(mode="merge", failOnWarnings=True, body=<warning-producing body>).
Why: failOnWarnings is the only guard a caller has against partially-correct documentation being published; here it is a no-op, and the atomicity claim (nothing created on refusal) cannot hold because no refusal occurs.
Fix: the ImportDocumentationParts handler — warning collection and the failOnWarnings gate that must turn collected warnings into a BadRequestException and roll the import back.
Traces: apigateway.import-documentation-parts-bulk-atomic

Cause 5 of 7: DeleteModel on a model still referenced by a Method answers the unmodeled code NotImplemented instead of 409 ConflictException.
AWS: Smithy DeleteModel (DELETE /restapis/{restApiId}/models/{modelName}, success 202) models ConflictException with httpError 409; a model named in a Method's requestModels cannot be deleted and is refused with ConflictException, and the same delete succeeds once the Method reference is removed.
Emulator: the error code returned is 'NotImplemented' — a code the model does not declare for this operation. Recorded: "expected ConflictException, got 'NotImplemented'".
Why: a client cannot distinguish "still referenced" from "not supported", and the retry-after-detach workflow AWS guarantees is impossible; NotImplemented is not in DeleteModel's error set, so SDK error handling cannot branch on it.
Fix: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/delete_model.rs — the handler must exist and must check Method.requestModels references before deletion. Note the scaffold-stub issue filed for DeleteModel in this same replay (it blocks apigateway.model-schema-unicode-round-trip): implementing the stub is necessary but not sufficient — this cause additionally requires the referenced-by-Method ConflictException rule.
Traces: apigateway.op-contract-delete-model

Cause 6 of 7: DeleteAuthorizer's referenced-authorizer ConflictException carries invented message text.
AWS: Smithy DeleteAuthorizer models ConflictException with httpError 409. A DeleteAuthorizer blocked by a Method that points at the authorizer is refused with the message "Cannot delete authorizer '<authorizerName>', is referenced in method: <HTTP-METHOD>/<resource-path>" — the fixed parts are "Cannot delete authorizer '", "', is referenced in method: "; the variable parts are the authorizer name from state and the referencing method's HTTP verb and resource path.
Emulator: same code, different text — 'Active methods pointing to this authorizer must be updated or deleted'. Recorded expectation vs actual: expected "Cannot delete authorizer 'sim2real-authz-1d82f4d9d884', is referenced in method: GET//blocked", got 'Active methods pointing to this authorizer must be updated or deleted' (the name and GET//blocked are that run's values filling the template, not part of the requirement).
Why: the emulator's text names no authorizer and no method, so tooling that parses the refusal to find which method still references the authorizer gets nothing; log assertions keyed on AWS's wording fail.
Fix: the DeleteAuthorizer handler's conflict path (error construction). Same family as the fixed issue 20261025T014422Z-apigateway-api-gateway-control-plane-10-causes-of-invented.md, which does NOT include DeleteAuthorizer among its ten causes.
Traces: apigateway.authorizer-deletion-blocked-while-referenced-rejected

Cause 7 of 7: CreateModel's malformed-schema BadRequestException carries invented message text.
AWS: Smithy CreateModel (POST /restapis/{restApiId}/models, errors include BadRequestException 400). A schema string that is not a JSON document is refused with the constant message "Invalid model specified: Validation Result: warnings : [], errors : [Invalid model schema specified]" — entirely fixed text for this condition, with no part taken from the request — and the model is not persisted.
Emulator: same code, text 'Invalid model schema specified: the schema is not a JSON document'.
Why: the refusal is not attributable by text; callers matching AWS's validation-result wording (and anything parsing its warnings/errors lists) do not match.
Fix: the CreateModel handler's schema-validation refusal path. Same family as the fixed issue 20261025T014422Z-apigateway-api-gateway-control-plane-10-causes-of-invented.md, which does NOT include CreateModel's schema message.
Traces: apigateway.model-invalid-schema-json-rejected

## Evidence
Status/code/message as recorded by the replay (per-run names, ids and timestamps below are template fill, not requirements):

Cause 1 — apigateway.apigw-request-validator-selects-bad-request-body-vs-parameters-gateway-responses: "expected 400 for a body omitting the model's required property 'qty5fivwc' while the required query parameter is present, got 200 body '{\"integration\":\"sim2real-q03n3w8tty\"}'". Expected: HTTP 400, x-amzn-ErrorType: BadRequestException, body {"message":"Invalid request body"}.

Cause 2 — apigateway.deep-apigw-integration-request-validator-modes-validate-only-what-they-declare: "Check timed out after 60.0s". The check's first data-plane step is _poll(post(route, good), res[0] == 200, timeout_s=150.0, interval_s=3.0) for each of /body, /params, /both, /none; no assertion message was produced, so the positive control never reached 200.

Cause 3 — apigateway.e2e-usage-plan-key-throttle-consequence: "Check timed out after 60.0s" while looping (deadline 90 s) until both seen_200 and seen_429; expected a 429 / TooManyRequestsException from throttle {burstLimit: 1, rateLimit: 1}, none observed.

Cause 4 — apigateway.import-documentation-parts-bulk-atomic: at import_documentation_parts(restApiId=..., mode="merge", failOnWarnings=True, body=<warning body>) — "AssertionError: expected a ClientError, but the call SUCCEEDED". Expected HTTP 400 BadRequestException and no parts created.

Cause 5 — apigateway.op-contract-delete-model: "AssertionError: expected ConflictException, got 'NotImplemented'" for DeleteModel of a model referenced from a Method's /requestModels/application~1json. Expected HTTP 409 ConflictException (modeled); got the unmodeled NotImplemented.

Cause 6 — apigateway.authorizer-deletion-blocked-while-referenced-rejected: expected message "Cannot delete authorizer '<name>', is referenced in method: <METHOD>/<path>" (recorded instance: "Cannot delete authorizer 'sim2real-authz-1d82f4d9d884', is referenced in method: GET//blocked"); got 'Active methods pointing to this authorizer must be updated or deleted'. Code ConflictException matched.

Cause 7 — apigateway.model-invalid-schema-json-rejected: expected message "Invalid model specified: Validation Result: warnings : [], errors : [Invalid model schema specified]"; got 'Invalid model schema specified: the schema is not a JSON document' for CreateModel with schema '{"type": "object", "properties": {"a": }'. Code matched.

## Steps to Reproduce
1 (Cause 1 & 2). CreateRestApi (REGIONAL); CreateModel with a draft-04 object schema requiring two properties; CreateRequestValidator with validateRequestBody=true (and variants validateRequestParameters=true / both / none); PutMethod POST with requestModels={"application/json": <model>}, requestParameters={"method.request.querystring.tenant": true} and requestValidatorId; PutIntegration MOCK with requestTemplates {"application/json": '{"statusCode": 200}'}; PutMethodResponse/PutIntegrationResponse 200; CreateDeployment + CreateStage. Then POST the stage invoke URL with ?tenant=x and (a) a valid body — expect 200 from the integration; (b) a body omitting a required property — expect 400, x-amzn-ErrorType: BadRequestException, {"message":"Invalid request body"}; (c) a valid body with no tenant on a params validator — expect 400 {"message":"Missing required request parameters: [tenant]"}.
2 (Cause 3). Deploy a MOCK GET method with apiKeyRequired=true; CreateApiKey(enabled=true); CreateUsagePlan with apiStages=[{apiId, stage}] and throttle {burstLimit: 1, rateLimit: 1}; CreateUsagePlanKey. Send the invoke URL a burst of requests with x-api-key: expect at least one HTTP 429 TooManyRequestsException after the first 200.
3 (Cause 4). ImportDocumentationParts(restApiId, mode="merge", failOnWarnings=true, body=<payload that produces a warning>): expect 400 BadRequestException and GetDocumentationParts unchanged; with failOnWarnings=false the same payload must commit and report warnings.
4 (Cause 5). CreateModel, PutMethod POST + PutIntegration MOCK, UpdateMethod add /requestModels/application~1json=<model>, then DeleteModel: expect 409 ConflictException; remove the reference and DeleteModel again: expect 202.
5 (Cause 6). CreateAuthorizer, PutMethod with authorizationType=CUSTOM + that authorizerId, then DeleteAuthorizer: read Error.Message.
6 (Cause 7). CreateModel with schema='{"type": "object", "properties": {"a": }': read Error.Message and confirm the model was not created.
Then replay the check ids in trace_ids.
