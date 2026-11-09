Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-stage-method-settings-throttle-returns-default-too-many-requests apigateway.canary-fifty-percent-split-serves-both-versions-within-tolerance apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping apigateway.deleting-one-stage-leaves-a-sibling-stage-of-the-same-deployment-serving apigateway.deployed-stage-serves-mock-for-a-path-parameter-put-with-its-mapped-status apigateway.import-rest-api-unknown-basepath-mode-rejected apigateway.request-validator-body-validation-skipped-for-unmapped-content-type
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-stage-method-settings-throttle-returns-default-too-many-requests|run#20|TimeoutError|raise TimeoutError(
Issue-Key: apigateway.canary-fifty-percent-split-serves-both-versions-within-tolerance|run#28|TimeoutError|raise TimeoutError("the canary warm-up: the run budget ended first; not graded")
Issue-Key: apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping|run#10|AssertionError|assert mapping_ok and delete_ok, (
Issue-Key: apigateway.deleting-one-stage-leaves-a-sibling-stage-of-the-same-deployment-serving|run#33|AssertionError|assert (gone[0], _body(gone[2])) == (403, {"message": "Forbidden"}), (
Issue-Key: api-gateway|GetResource|NotFoundException
Issue-Key: api-gateway|ImportRestApi|BadRequestException
Issue-Key: apigateway.request-validator-body-validation-skipped-for-unmapped-content-type|_poll_http#19|AssertionError|raise AssertionError(
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway: stage throttling is never enforced, deleted-API refusals are mis-routed, and 4 more control/data-plane divergences (7 causes)

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Seven net-new root causes in the API Gateway stage data plane and refusal routing. Numbered; each carries its own failing check ids.

Cause 1 of 7: stage/method-settings throttling is never enforced.
AWS: a stage methodSettings entry (set through UpdateStage patchOperations on /*/*/throttling/rateLimit and /*/*/throttling/burstLimit) caps requests through that stage; requests above the token bucket are shed with HTTP 429 and the stage's DEFAULT/THROTTLED gateway response, body {"message": "Too Many Requests"}.
Emulator: with a wildcard throttle of rate=1/burst=1, repeated 40-way concurrent bursts against the stage were ALL answered 200 — not one request was ever shed within 180s.
Why: a client validating its retry/backoff handling, or any test that depends on a 429 ever being produced, can never observe throttling; the configured limit is inert.
Fix: the execute-api invoke path's per-stage/per-method throttle enforcement (the token bucket that methodSettings throttling/rateLimit and throttling/burstLimit feed).
Traces: apigateway.apigw-stage-method-settings-throttle-returns-default-too-many-requests

Cause 2 of 7: a canary stage never begins serving, so canary traffic splitting is unobservable.
AWS: once a deployment with canarySettings is attached to a stage, both the base and the canary snapshot answer through the stage URL, and repeated requests are split according to canarySettings.percentTraffic.
Emulator: the canary warm-up never completes — the stage never answers, so no split is ever observable (the check's whole budget elapses without a graded response).
Why: a client cannot verify a canary release at all; the canary appears deployed but serves nothing.
Fix: the execute-api invoke path's stage/canary snapshot selection.
Traces: apigateway.canary-fifty-percent-split-serves-both-versions-within-tolerance

Cause 3 of 7: DeleteRestApi is refused while a base path mapping references the API.
AWS: DeleteRestApi succeeds even when a custom-domain base path mapping points at the API; the mapping is left behind (orphaned) rather than blocking the delete. Smithy DeleteRestApi models NotFoundException/TooManyRequestsException/UnauthorizedException/BadRequestException, but a referencing base path mapping is not a precondition for it.
Emulator: DeleteRestApi answers 400 BadRequestException, so a CreateBasePathMapping + DeleteRestApi sequence can never both succeed.
Why: a client tearing down an API behind a custom domain cannot delete it without first discovering and deleting every mapping — a step real AWS does not require.
Fix: the DeleteRestApi handler's base-path-mapping precondition (remove it) and the mapping state it leaves behind.
Traces: apigateway.delete-rest-api-racing-base-path-mapping-leaves-orphaned-mapping

Cause 4 of 7: a request to a deleted API's stage URL is answered by the emulator's service router, not by execute-api.
AWS: an execute-api request for a REST API that no longer exists is answered by API Gateway with HTTP 403 and body {"message": "Forbidden"}.
Emulator: the request is answered HTTP 404 with {"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"} — the generic service-router error, not an API Gateway response at all.
Why: a client cannot tell a deleted API from a mis-addressed call, and the status (404 vs 403), error type and body all differ from AWS.
Fix: the execute-api front door's routing for a stage path whose API id is unknown (it must reach the API Gateway data plane and emit its 403 Forbidden).
Traces: apigateway.deleting-one-stage-leaves-a-sibling-stage-of-the-same-deployment-serving

Cause 5 of 7: GetResource on a deleted REST API reports the wrong not-found subject and an account-prefixed id.
AWS: Smithy GetResource models NotFoundException (httpError 404); its message is the fixed text "Invalid Resource identifier specified".
Emulator: NotFoundException with message 'Invalid API identifier specified 000000000000:<restApiId>' — it names the API instead of the resource, and appends an account-qualified id ("<account>:<apiId>") that AWS never puts in this message.
Why: a client branching on which identifier AWS reported invalid takes the wrong branch, and a message match fails.
Fix: the GetResource handler's not-found path and the shared NotFoundException message builder (drop the "<account>:<id>" suffix).
Traces: apigateway.deployed-stage-serves-mock-for-a-path-parameter-put-with-its-mapped-status

Cause 6 of 7: ImportRestApi's unrecognised basepath mode refusal uses emulator wording.
AWS: ImportRestApi takes parameters as httpQueryParams and models BadRequestException (httpError 400). An unrecognised `basepath` value is refused 400 BadRequestException with the fixed text "Unrecognized base path option: " followed by the value supplied in the request.
Emulator: BadRequestException with 'parameters basepath must be one of ignore, prepend, split, got wrap' — emulator-authored prose that enumerates the accepted values instead of AWS's sentence.
Why: the status and code match but the message does not, so clients and tooling matching AWS's wording do not match.
Fix: the ImportRestApi handler's `basepath` parameter validation message.
Traces: apigateway.import-rest-api-unknown-basepath-mode-rejected

Cause 7 of 7: a request to a method with authorizationType NONE is refused 403 by an IAM-style denial at the stage.
AWS: a method whose authorizationType is NONE admits an unsigned/anonymous request; the request reaches the request validator and the integration, and a terminal response (e.g. 200, or 415 for an unmapped content type under a strict passthrough) is served.
Emulator: the stage answers 403 with {"Message":"User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:<region>:<account>:<apiId>/<stage>/<METHOD>/<path>"} — the execute-api IAM denial — so the request never reaches validation or the integration and no terminal response is ever produced.
Why: every anonymous call to a NONE-authorized method is refused, which makes an open REST API unusable and hides every downstream behaviour (request validation, passthrough, integration result).
Fix: the execute-api invoke path's authorization stage — authorizationType NONE must skip the execute-api:Invoke identity check.
Traces: apigateway.request-validator-body-validation-skipped-for-unmapped-content-type

## Evidence
Cause 1: "TimeoutError: a wildcard method-settings throttle of rate=1/burst=1 never shed a single request out of repeated 40-way concurrent bursts against https://<host>/_aws/execute-api/<apiId>/test/ within 180s; last burst statuses [200 x40]".
Cause 2: "TimeoutError: the canary warm-up: the run budget ended first; not graded".
Cause 3: "expected a round where both CreateBasePathMapping and DeleteRestApi succeed, got (basePath, mapping error, delete error) per round: [('v1', None, 'BadRequestException')]".
Cause 4: "GET on the deleted API's stage: expected (403, {'message': 'Forbidden'}), got (404, '{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}')".
Cause 5: "GetResource on the deleted API: expected ('NotFoundException', 'Invalid Resource identifier specified'), got ('NotFoundException', 'Invalid API identifier specified 000000000000:<apiId>')".
Cause 6: "expected ('BadRequestException', 'Unrecognized base path option: wrap'), got ('BadRequestException', 'parameters basepath must be one of ignore, prepend, split, got wrap')".
Cause 7: "POST https://<host>/_aws/execute-api/<apiId>/<stage>/items with an unmapped text/plain Content-Type to reach a terminal response not observed within 150.0s; last=(status=403, body='{\"Message\":\"User: anonymous is not authorized to perform: execute-api:Invoke on resource: arn:aws:execute-api:us-west-2:000000000000:<apiId>/<stage>/POST/items\"}')".

## Steps to Reproduce
1. Cause 1: create + deploy a REST API with a MOCK method; UpdateStage with patchOperations setting /*/*/throttling/rateLimit=1 and /*/*/throttling/burstLimit=1; fire 40 concurrent GETs at the stage URL. Expect some 429 with body {"message": "Too Many Requests"}.
2. Cause 2: CreateDeployment with canarySettings against a live stage and poll the stage URL. Expect both snapshots to answer.
3. Cause 3: CreateDomainName + CreateBasePathMapping pointing at the API, then DeleteRestApi. Expect the delete to succeed.
4. Cause 4: deploy a stage, DeleteRestApi, then GET the stage URL. Expect 403 {"message": "Forbidden"}.
5. Cause 5: DeleteRestApi then GetResource(restApiId, resourceId). Expect NotFoundException "Invalid Resource identifier specified".
6. Cause 6: ImportRestApi(body=<valid OpenAPI>, parameters={'basepath': 'wrap'}). Expect 400 "Unrecognized base path option: wrap".
7. Cause 7: PutMethod with authorizationType='NONE' plus a request validator, deploy, and POST unsigned with Content-Type: text/plain. Expect a terminal API Gateway response, not a 403 execute-api denial.
