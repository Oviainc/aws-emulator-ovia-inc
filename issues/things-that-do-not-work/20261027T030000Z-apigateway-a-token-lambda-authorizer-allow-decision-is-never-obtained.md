Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.e2e-custom-authorizer-protected-method-flow
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.e2e-custom-authorizer-protected-method-flow|run#45|AssertionError|assert allowed_status == 200, f"expected 200 on the real endpoint once authorized, got {allowed_status}"
Issue-Commit: 946f146bc1cbb921fa2f003801745ea745af5a9e

# API Gateway: on the deployed execute-api endpoint, a request a TOKEN Lambda authorizer ALLOWS answers 500 instead of running the method's integration

> _Note: this cause was identified against the code base at commit `946f146bc1cbb921fa2f003801745ea745af5a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Provenance: this was cause 2 of 2 of `20261026T054047Z-apigateway-api-gateway-testinvokemethod-never-refuses-a-method-with.md`. Cause 1 of that issue (TestInvokeMethod answering 200/status=500 for a method with no integration) is fixed and that report now sits under `issues/things-that-now-work/`; this cause is unfixed and is re-filed here so it is not recorded as working. Its `Issue-Key` moved with it._

## Operation
The `execute-api` data plane (the deployed stage's invoke URL), authorizer stage

## Description
AWS: recorded behavior only (the data plane is not modeled in the Smithy file). The recording shows a REST API with a `CUSTOM` authorizationType method bound to a TOKEN authorizer whose `authorizerUri` names a Lambda function, `identitySource=method.request.header.Authorization`, `authorizerResultTtlInSeconds=0`, and a MOCK integration with requestTemplate `{"statusCode": 200}` and integration-response template `{"message": "secured-ok"}`. Against real AWS the deployed stage answers a request with NO Authorization header with 401/403 and an `Unauthorized` body, and answers the request carrying the allowed token with HTTP 200 and the MOCK integration's body — the authorizer's Allow policy is honored and the integration then runs.

Emulator: the denial leg is answered correctly (the 401/403 and `unauthorized`-body assertions pass), but the ALLOW leg answers HTTP 500. `authorization_refusal` in `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs` checks each declared identity source and then answers `AUTHORIZER_CONFIGURATION_ERROR` (500) for every authorizer-backed method, because the decision it needs is the RETURN VALUE of the authorizer's Lambda function and no such value exists here. Generally: a positive TOKEN Lambda-authorizer decision is never produced, so every authorizer-protected method is effectively fail-closed with a 500 even when its integration needs no backend at all.

Why: a client gets a 500 where AWS returns the integration's 200 body. Authorization becomes untestable in the only direction that matters — allow — so an application cannot be exercised past its front door, and a 500 is also the wrong class: it tells the caller the service failed rather than that the request was refused.

The ONE blocker, for whoever picks this up: nothing in this emulator can run a Lambda deployment package, so there is no policy document to evaluate and no honest way to produce an Allow. `services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs` answers 501 `NotImplemented` for `InvocationType=RequestResponse` for exactly that reason, and `services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_authorizer.rs` is a 501 stub for the same reason. Admitting any request that merely carries the identity source, or matching the recorded token, would fabricate the result of code nothing ran, and answering the allowed call with a different refusal is not a fix either. So this cause is gated on an in-process Lambda execution runtime — the subject of `20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md` and `20261024T055421Z-lambda-opensearch-s3-lambda-invoke-cannot-execute-a-deployment-package-so.md`. Once a function's handler can run, the work left in THIS crate is: resolve the function the `authorizerUri` names, call it with the TOKEN event (`type`, `authorizationToken`, `methodArn`), evaluate the returned policy document against the invoked method ARN, honour `authorizerResultTtlInSeconds` caching, and let an Allow fall through to `crate::service::invoke::perform_integration` — which already serves the MOCK integration behind it in full.

Traces: apigateway.e2e-custom-authorizer-protected-method-flow

## Evidence
Expected (deployed stage, GET /secure with header `Authorization: allow-me`): HTTP 200 with body {"message": "secured-ok"}.
Actual: HTTP 500.
Harness assertion: AssertionError: expected 200 on the real endpoint once authorized, got 500
The preceding no-identity-source leg passed (401/403 with an `unauthorized` body), isolating the divergence to the Allow decision and the integration run that must follow it.

## Steps to Reproduce
1. Create an IAM role trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole, and CreateFunction with a handler that returns an IAM policy with Effect=Allow when event["authorizationToken"] == "allow-me" and Deny otherwise; poll until State=Active.
2. CreateRestApi; CreateResource pathPart="secure".
3. CreateAuthorizer type=TOKEN, authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, identitySource=method.request.header.Authorization, authorizerResultTtlInSeconds=0; lambda AddPermission for principal apigateway.amazonaws.com scoped to <apiId>/authorizers/<authorizerId>.
4. PutMethod GET authorizationType=CUSTOM authorizerId=<id>; PutMethodResponse 200; PutIntegration type=MOCK requestTemplates={"application/json": '{"statusCode": 200}'}; PutIntegrationResponse 200 responseTemplates={"application/json": '{"message": "secured-ok"}'}.
5. CreateDeployment; CreateStage stageName=prod.
6. GET <stage-url>/secure with no Authorization header -> 401/403 with an `unauthorized` body (matches AWS).
7. GET <stage-url>/secure with header `Authorization: allow-me` -> AWS returns 200 and body {"message": "secured-ok"}; the emulator returns 500.
