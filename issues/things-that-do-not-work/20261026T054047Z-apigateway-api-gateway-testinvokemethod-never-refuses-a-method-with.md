Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.test-invoke-method-missing-integration-rejected apigateway.e2e-custom-authorizer-protected-method-flow
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.test-invoke-method-missing-integration-rejected|_client_error#17|AssertionError|raise AssertionError(f"expected this call to fail, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.e2e-custom-authorizer-protected-method-flow|run#45|AssertionError|assert allowed_status == 200, f"expected 200 on the real endpoint once authorized, got {allowed_status}"
Issue-Commit: 946f146bc1cbb921fa2f003801745ea745af5a9e

# API Gateway: TestInvokeMethod never refuses a method with no integration, and an execute-api request allowed by a TOKEN Lambda authorizer answers 500 instead of running the integration

> _Note: this issue was identified against the code base at commit `946f146bc1cbb921fa2f003801745ea745af5a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Two distinct root causes in the API Gateway emulator's invocation path, both grounded in failing behavioral checks. 2 causes in this issue; both must be closed before submitting.

=== Cause 1 of 2: TestInvokeMethod answers 200/status=500 for a method that has NO integration, instead of refusing with BadRequestException ===

AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models TestInvokeMethod with errors BadRequestException (HTTP 400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401); BadRequestException is documented "The submitted request is not valid, for example, the input is incomplete or incorrect. See the accompanying error message for details." Recorded behavior: when the addressed Method exists but carries no Integration, real AWS REFUSES the TestInvokeMethod call itself — HTTP 400, error code `BadRequestException`, message `No integration defined for method`. That message is entirely FIXED text: it names no api id, resource id or verb, so nothing in it is taken from the request or from state. The recording establishes this is a refusal of the control-plane call, not a simulated 5xx response: the surrounding GetIntegration read in the same recording returns 404 `NotFoundException` with message `Invalid Integration identifier specified` (also wholly fixed text), proving the integration is structurally absent, and the identical TestInvokeMethod call succeeds with status 200 once a MOCK integration is restored.

Emulator: the call SUCCEEDS. HTTP 200 with a TestInvokeMethodResponse carrying `status: 500`, `body: {"message":"Internal server error"}`, `headers: {"Content-Type":"application/json"}` and an execution log containing `Integration type: none`, `Execution failed due to configuration error: the method carries no integration, so there is nothing to call` and `Answered through this API's API_CONFIGURATION_ERROR gateway response`. No modeled error is raised at all. Generally: a missing integration is treated as a data-plane configuration error to be rendered through the API's API_CONFIGURATION_ERROR gateway response, rather than as a precondition of the TestInvokeMethod operation.

Why: a client cannot tell "this method is unconfigured" from "this method's backend returned a 500". botocore raises no ClientError, so error-handling branches never run; CI and provisioning tools that assert on BadRequestException to detect a half-wired method see a successful call. Worse, an over-permissive accept hides a real misconfiguration, and the 500 is indistinguishable from a genuine backend failure.

Fix: the TestInvokeMethod handler's integration resolution. In services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs the `handle` body resolves the Method, then passes `method.integration` (which is `None` here) straight into `perform_integration`, and maps the resulting `Err(unserved)` through `refusal_rendering` into a simulated gateway response. The absent-integration case must be split out BEFORE that call and returned as the modeled `BadRequestException` on this operation's error channel, so the operation answers HTTP 400 / `BadRequestException` / `No integration defined for method`. The gateway-response rendering path stays for integrations that exist but cannot be performed.

Traces: apigateway.test-invoke-method-missing-integration-rejected

=== Cause 2 of 2: on the deployed execute-api endpoint, a request a TOKEN Lambda authorizer ALLOWS answers 500 instead of running the method's integration ===

AWS: recorded behavior only (the data plane is not modeled in the Smithy file). The recording shows a REST API with a `CUSTOM` authorizationType method bound to a TOKEN authorizer whose `authorizerUri` names a Lambda function, `identitySource=method.request.header.Authorization`, `authorizerResultTtlInSeconds=0`, and a MOCK integration with requestTemplate `{"statusCode": 200}` and integration-response template `{"message": "secured-ok"}`. Against real AWS the deployed stage answers a request with NO Authorization header with 401/403 and an `Unauthorized` body, and answers the request carrying the allowed token with HTTP 200 and the MOCK integration's body — the authorizer's Allow policy is honored and the integration then runs.

Emulator: the denial leg is answered correctly (the check's 401/403 and `unauthorized`-body assertions pass), but the ALLOW leg answers HTTP 500. The emulator does not invoke the authorizer Lambda, so it cannot obtain the Allow policy document, and the request is terminated as a configuration error rather than being forwarded to the (purely in-process) MOCK integration. Generally: a positive TOKEN Lambda-authorizer decision is never produced, so every authorizer-protected method is effectively fail-closed with a 500 even when its integration needs no backend at all.

Why: a client gets a 500 where AWS returns the integration's 200 body. Authorization becomes untestable in the only direction that matters — allow — so an application cannot be exercised past its front door, and a 500 is also the wrong class: it tells the caller the service failed rather than that the request was refused.

Hypothesis: this shares a cause with the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (Lambda Invoke cannot run a deployment package). That issue is NOT marked as owning this check and is scoped to the lambda service's Invoke operation; this finding is the API Gateway authorizer path's consequence and additionally covers the part that needs no Lambda at all — the MOCK integration behind the authorizer, which the emulator never reaches.

Fix: the execute-api data plane's authorizer stage — the authorizer evaluation that feeds `crate::service::invoke::perform_integration` in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs, reached from the stage request handler. An Allow decision must let the request proceed to the method's integration; a decision the emulator cannot compute must not be collapsed into a 500 for an integration that is servable in process.

Traces: apigateway.e2e-custom-authorizer-protected-method-flow

## Evidence
Cause 1 — apigateway.test-invoke-method-missing-integration-rejected
Expected: ClientError, HTTP 400, Code `BadRequestException`, Message `No integration defined for method` (all fixed text).
Actual: no exception. HTTP 200, response:
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'status': 500,
   'body': '{"message":"Internal server error"}',
   'headers': {'Content-Type': 'application/json'},
   'log': "... : Integration type: none\n... : Execution failed due to configuration error: the method carries no integration, so there is nothing to call\n... : Answered through this API's API_CONFIGURATION_ERROR gateway response\n... : Method completed with status: 500", 'latency': 0}
Harness assertion: AssertionError: expected this call to fail, but it SUCCEEDED and returned {...}
(The same recording's control reads pass: GetIntegration after DeleteIntegration returns 404 NotFoundException / `Invalid Integration identifier specified`, and TestInvokeMethod returns status 200 both before the delete and after the integration is restored.)

Cause 2 — apigateway.e2e-custom-authorizer-protected-method-flow
Expected (deployed stage, GET /secure with header `Authorization: allow-me`): HTTP 200 with body {"message": "secured-ok"}.
Actual: HTTP 500.
Harness assertion: AssertionError: expected 200 on the real endpoint once authorized, got 500
The preceding no-identity-source leg passed (401/403 with an `unauthorized` body), isolating the divergence to the Allow decision and the integration run that must follow it.

## Steps to Reproduce
Cause 1 (apigateway.test-invoke-method-missing-integration-rejected):
1. CreateRestApi; read the root resource id via GetResources; CreateResource pathPart="bare".
2. PutMethod httpMethod=GET authorizationType=NONE; PutMethodResponse statusCode=200.
3. PutIntegration type=MOCK requestTemplates={"application/json": '{"statusCode": 200}'}; PutIntegrationResponse statusCode=200 responseTemplates={"application/json": '{"message": "ok"}'}.
4. TestInvokeMethod(restApiId, resourceId, httpMethod=GET, pathWithQueryString="/bare") -> expect status 200 (positive control; passes).
5. DeleteIntegration on that method. GetIntegration -> 404 NotFoundException `Invalid Integration identifier specified` (passes).
6. TestInvokeMethod with the identical arguments as step 4 -> AWS refuses with 400 BadRequestException `No integration defined for method`; the emulator returns HTTP 200 with status=500 and an API_CONFIGURATION_ERROR log.
7. PutIntegration/PutIntegrationResponse again -> TestInvokeMethod returns status 200 (restore leg).

Cause 2 (apigateway.e2e-custom-authorizer-protected-method-flow):
1. Create an IAM role trusting lambda.amazonaws.com, attach AWSLambdaBasicExecutionRole, and CreateFunction with a handler that returns an IAM policy with Effect=Allow when event["authorizationToken"] == "allow-me" and Deny otherwise; poll until State=Active.
2. CreateRestApi; CreateResource pathPart="secure".
3. CreateAuthorizer type=TOKEN, authorizerUri=arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>/invocations, identitySource=method.request.header.Authorization, authorizerResultTtlInSeconds=0; lambda AddPermission for principal apigateway.amazonaws.com scoped to <apiId>/authorizers/<authorizerId>.
4. PutMethod GET authorizationType=CUSTOM authorizerId=<id>; PutMethodResponse 200; PutIntegration type=MOCK requestTemplates={"application/json": '{"statusCode": 200}'}; PutIntegrationResponse 200 responseTemplates={"application/json": '{"message": "secured-ok"}'}.
5. CreateDeployment; CreateStage stageName=prod.
6. GET <stage-url>/secure with no Authorization header -> 401/403 with an `unauthorized` body (matches AWS).
7. GET <stage-url>/secure with header `Authorization: allow-me` -> AWS returns 200 and body {"message": "secured-ok"}; the emulator returns 500.
