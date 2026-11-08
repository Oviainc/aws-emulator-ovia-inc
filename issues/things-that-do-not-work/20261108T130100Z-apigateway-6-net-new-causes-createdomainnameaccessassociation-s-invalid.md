Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-domain-name-access-association-invalid-source-rejected apigateway.deploy-stage-with-variables-cfn-behaves apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering apigateway.deploy-apigateway-1-cfn-deployment-without-method-rejected apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment apigateway.deploy-authorizer-cfn-enforces-identity-source
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-domain-name-access-association-invalid-source-rejected|run#15|AssertionError|assert message == "Invalid VPC endpoint ID specified.", f"unexpected BadRequestException message {message!r}"
Issue-Key: apigateway.deploy-stage-with-variables-cfn-behaves|run#9|AssertionError|assert method_settings is not None, f"no methodSettings for cached/GET: {stage.get('methodSettings')}"
Issue-Key: apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering|run#46|AssertionError|assert status == "UPDATE_COMPLETE", f"update 2 settled at {status!r}, expected UPDATE_COMPLETE"
Issue-Key: apigateway.deploy-apigateway-1-cfn-deployment-without-method-rejected|run#24|AssertionError|assert http_status is not None and http_status.group(1) == "400", (
Issue-Key: apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment|run#22|AssertionError|assert failed == [_BAD_LOGICAL_ID], (
Issue-Key: apigateway.deploy-authorizer-cfn-enforces-identity-source|run#20|AssertionError|assert status_missing == 401, (status_missing, body_missing)
Issue-Commit: 386e377a24364e2ae13f63408a712b163326bf5e

# apigateway: 6 net-new causes — CreateDomainNameAccessAssociation's invalid-VPCE sentence, CFN-managed Stage methodSettings keys, two CFN deploy/rollback outcomes, a missing CREATE_FAILED event, and an authorizer identity-source 500

> _Note: this issue was identified against the code base at commit `386e377a24364e2ae13f63408a712b163326bf5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainNameAccessAssociation

## Description
Six independent net-new root causes in the API Gateway control plane and its CloudFormation provisioning path. Each is listed with its own failing check ids; all six must be closed before this issue is submitted.

Cause 1 of 6: CreateDomainNameAccessAssociation refuses a malformed VPC endpoint id with an emulator-authored sentence.
AWS: Smithy: CreateDomainNameAccessAssociation (POST /domainnameaccessassociations, success 201) models BadRequestException (httpError 400) for a malformed accessAssociationSource. Recorded behavior: the 400 BadRequestException message is the fixed sentence "Invalid VPC endpoint ID specified." — the whole message is fixed text, it interpolates NO part of the request (the rejected value never appears).
Emulator: answers BadRequestException whose message interpolates the submitted value, e.g. message '<submitted-source> is not a valid VPC endpoint id' (replay saw 'not-a-valid-vpce-id is not a valid VPC endpoint id').
Why: a client that branches on, logs, or asserts the documented refusal sentence sees an unknown message; the value-interpolating form also leaks the request value into a message AWS keeps constant.
Fix: the CreateDomainNameAccessAssociation handler's source validation — the BadRequestException it raises for a non-VPCE-shaped accessAssociationSource must carry AWS's fixed sentence.
Traces: apigateway.create-domain-name-access-association-invalid-source-rejected.

Cause 2 of 6: GetStage reports a CloudFormation-managed Stage.MethodSettings entry under the JSON-Pointer-escaped patch path instead of the method path.
AWS: Smithy: Stage.methodSettings is a MapOfMethodSettings whose keys are "method paths defined as {resource_path}/{http_method} for an individual method override, or /*/* for overriding all methods in the stage". For an AWS::ApiGateway::Stage whose MethodSettings entry declares ResourcePath "/cached" and HttpMethod "GET", the recorded key is "cached/GET" (the resource path without its leading slash, then the verb). The '~1' escaping belongs to UpdateStage patch paths, not to the GetStage representation.
Emulator: keys the same entry "~1cached/GET", so a lookup of the documented method path finds nothing; the settings payload itself (cachingEnabled true, cacheTtlInSeconds 300, ...) is correct.
Why: a client reading per-method cache or logging settings by method path from a CFN-provisioned stage sees an empty map entry and concludes caching is unconfigured.
Fix: the stage representation/serializer for methodSettings (the CloudFormation Stage provisioner's translation of MethodSettings into stored keys), which must store/emit the method-path key rather than the escaped patch path.
Traces: apigateway.deploy-stage-with-variables-cfn-behaves.

Cause 3 of 6: a stack UPDATE that re-snapshots an AWS::ApiGateway::Deployment rolls back instead of completing.
AWS: a second UpdateStack that changes the API's methods/integrations and the Deployment resource settles UPDATE_COMPLETE, and the stage then serves the new snapshot (recorded behavior only — CloudFormation models no status for this).
Emulator: the second update settles UPDATE_ROLLBACK_COMPLETE, so the new deployment is never published.
Why: the documented "versioned deployment" pattern (the only template-level way to force a new API Gateway deployment snapshot) cannot be used against the emulator; every redeploy of an API through CloudFormation appears to fail.
Fix: the CloudFormation provisioner for AWS::ApiGateway::Deployment/Method/Resource on the update path (the resource handler whose failure triggers the rollback).
Traces: apigateway.deploy-apigateway-1-cfn-deployment-snapshot-ordering.

Cause 4 of 6: the CREATE_FAILED reason for a Deployment on a method-less REST API omits AWS's service/status trailer.
AWS: when AWS::ApiGateway::Deployment is created for a REST API that has no methods, the resource event's ResourceStatusReason is the handler sentence plus a trailer naming the calling service and the API Gateway HTTP status, of the form: Resource handler returned message: "The REST API doesn't contain any methods" (Service: ApiGateway, Status Code: 400, Request ID: <per-run id>, HandlerErrorCode: InvalidRequest). Fixed text: 'Resource handler returned message:', the quoted API Gateway sentence, 'Service: ApiGateway', 'Status Code: 400', 'HandlerErrorCode: InvalidRequest'; variable parts: only the request id.
Emulator: the reason is 'Resource handler returned message: "The REST API doesn\'t contain any methods" (RequestToken: <id>, HandlerErrorCode: InvalidRequest)' — no Service and no Status Code member, so the underlying 400 is unrecoverable from the event.
Why: tooling that parses the failure reason to recover the originating service and HTTP status (the standard way CDK/CFN users triage a failed resource) finds neither.
Fix: the CloudFormation event/reason formatter for a failed API Gateway resource handler (the component that wraps a downstream client error into ResourceStatusReason).
Traces: apigateway.deploy-apigateway-1-cfn-deployment-without-method-rejected.

Cause 5 of 6: a stack update that rolls back records no CREATE_FAILED event naming the offending resource.
AWS: when an UpdateStack adds a resource that cannot be created (here a duplicate API Gateway resource/method), DescribeStackEvents carries a CREATE_FAILED event for exactly that resource's LogicalResourceId before the rollback events (recorded behavior only).
Emulator: the update rolls back but DescribeStackEvents lists no CREATE_FAILED event at all (observed: the set of CREATE_FAILED logical ids was empty where exactly the one failing logical id was expected).
Why: the only machine-readable statement of WHICH resource failed is absent, so a client cannot attribute a rollback to a resource.
Fix: the CloudFormation stack-event recorder on the update/rollback path — the per-resource CREATE_FAILED event must be emitted before rollback begins.
Traces: apigateway.deploy-apigateway-2-update-rollback-keeps-serving-good-deployment.

Cause 6 of 6: a request missing a REQUEST authorizer's identity source answers 500 instead of 401.
AWS: when a method's authorizer declares an IdentitySource and the request does not carry it, API Gateway short-circuits and answers HTTP 401 with body {"message":"Unauthorized"} and never invokes the authorizer.
Emulator: answers HTTP 500 with body {"message":"Internal server error"}.
Why: a client cannot distinguish "you did not authenticate" from "the API is broken"; retry/refresh logic keyed on 401 never fires. Related open issue 20261107T191316Z-apigateway-4-net-new-causes-non-proxy-http reports 500s from non-proxy integrations, but does not cover the authorizer identity-source pre-check, which is a separate decision made before any integration runs.
Fix: the execute-api authorizer gate (identity-source extraction and its 401 refusal) in the stage invoke path.
Traces: apigateway.deploy-authorizer-cfn-enforces-identity-source.

## Evidence
Cause 1 — expected BadRequestException message "Invalid VPC endpoint ID specified."; got 'not-a-valid-vpce-id is not a valid VPC endpoint id' (AssertionError: unexpected BadRequestException message).
Cause 2 — GetStage returned methodSettings {'~1cached/GET': {...cachingEnabled: True, cacheTtlInSeconds: 300...}}; the documented key "cached/GET" is absent (AssertionError: no methodSettings for cached/GET).
Cause 3 — AssertionError: update 2 settled at 'UPDATE_ROLLBACK_COMPLETE', expected UPDATE_COMPLETE.
Cause 4 — AssertionError: Deployment CREATE_FAILED carried API Gateway status None, expected '400'; reason 'Resource handler returned message: "The REST API doesn\'t contain any methods" (RequestToken: ..., HandlerErrorCode: InvalidRequest)' — no 'Service: ApiGateway, Status Code: 400' members.
Cause 5 — AssertionError: CREATE_FAILED events during the update named [], expected exactly ['EchoDuplicate'].
Cause 6 — AssertionError: (500, '{"message":"Internal server error"}') where (401, {"message":"Unauthorized"}) was required for the request that omits the authorizer's identity source.

## Steps to Reproduce
1. Replay the five checks named in trace_ids against the emulator.
2. Cause 1: create a private custom domain, then CreateDomainNameAccessAssociation with accessAssociationSourceType=VPCE and a non-VPCE-shaped accessAssociationSource; read the 400 BadRequestException message.
3. Cause 2: deploy a stack with AWS::ApiGateway::Stage carrying MethodSettings[{ResourcePath:/cached, HttpMethod:GET, CachingEnabled:true, CacheTtlInSeconds:300}], then GetStage and inspect the methodSettings keys.
4. Cause 3: deploy a RestApi+Resource+Method+Deployment+Stage stack, then UpdateStack twice changing the integration and the Deployment logical id; read the final StackStatus.
5. Cause 4: deploy a stack whose AWS::ApiGateway::Deployment depends only on a Resource (no Method); read the CREATE_FAILED event's ResourceStatusReason.
6. Cause 5: UpdateStack adding a duplicate API Gateway resource so the update rolls back; list DescribeStackEvents and look for a CREATE_FAILED event naming the new logical id.
7. Cause 6: deploy a REQUEST authorizer with an IdentitySource, then call the stage without that header; read the status and body.
