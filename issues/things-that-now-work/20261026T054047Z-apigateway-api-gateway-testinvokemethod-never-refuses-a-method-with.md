Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.test-invoke-method-missing-integration-rejected
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.test-invoke-method-missing-integration-rejected|_client_error#17|AssertionError|raise AssertionError(f"expected this call to fail, but it SUCCEEDED and returned {result!r}")
Issue-Commit: 946f146bc1cbb921fa2f003801745ea745af5a9e

# API Gateway: TestInvokeMethod never refuses a method with no integration

> _Note: this issue was identified against the code base at commit `946f146bc1cbb921fa2f003801745ea745af5a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution
Fixed. `TestInvokeMethod` now refuses a call whose addressed Method carries no Integration on the operation's own error channel — HTTP 400, `BadRequestException`, message `No integration defined for method` — instead of simulating a 500 through the API's `API_CONFIGURATION_ERROR` gateway response. The gateway-response path stays for integrations that EXIST and cannot be performed. Covered by `services/service-com-amazonaws-apigateway-backplanecontrolservice/tests/all/test_invoke_method_no_integration.rs` (a table over the verb catalog and over every member a simulated request may carry, plus a served -> deleted -> refused -> restored lifecycle and the 404 precedence rows).

This report was filed with TWO causes. The second — an execute-api request a TOKEN Lambda authorizer ALLOWS answering 500 instead of running the method's integration — is NOT fixed: it is gated on an in-process Lambda execution runtime that this emulator does not have, and it was re-filed, with its own `Issue-Key`, as `issues/things-that-do-not-work/20261027T030000Z-apigateway-a-token-lambda-authorizer-allow-decision-is-never-obtained.md` so that it stays open work rather than being recorded here as working. Only cause 1's trace and key are kept above.

## Operation
TestInvokeMethod

## Description
AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models TestInvokeMethod with errors BadRequestException (HTTP 400), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401); BadRequestException is documented "The submitted request is not valid, for example, the input is incomplete or incorrect. See the accompanying error message for details." Recorded behavior: when the addressed Method exists but carries no Integration, real AWS REFUSES the TestInvokeMethod call itself — HTTP 400, error code `BadRequestException`, message `No integration defined for method`. That message is entirely FIXED text: it names no api id, resource id or verb, so nothing in it is taken from the request or from state. The recording establishes this is a refusal of the control-plane call, not a simulated 5xx response: the surrounding GetIntegration read in the same recording returns 404 `NotFoundException` with message `Invalid Integration identifier specified` (also wholly fixed text), proving the integration is structurally absent, and the identical TestInvokeMethod call succeeds with status 200 once a MOCK integration is restored.

Emulator (before the fix): the call SUCCEEDED. HTTP 200 with a TestInvokeMethodResponse carrying `status: 500`, `body: {"message":"Internal server error"}`, `headers: {"Content-Type": "application/json"}` and an execution log containing `Integration type: none`, `Execution failed due to configuration error: the method carries no integration, so there is nothing to call` and `Answered through this API's API_CONFIGURATION_ERROR gateway response`. No modeled error was raised at all. Generally: a missing integration was treated as a data-plane configuration error to be rendered through the API's API_CONFIGURATION_ERROR gateway response, rather than as a precondition of the TestInvokeMethod operation.

Why: a client cannot tell "this method is unconfigured" from "this method's backend returned a 500". botocore raises no ClientError, so error-handling branches never run; CI and provisioning tools that assert on BadRequestException to detect a half-wired method see a successful call. Worse, an over-permissive accept hides a real misconfiguration, and the 500 is indistinguishable from a genuine backend failure.

Fix: the TestInvokeMethod handler's integration resolution. In services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs the `handle` body resolved the Method, then passed `method.integration` (which is `None` here) straight into `perform_integration`, and mapped the resulting `Err(unserved)` through `refusal_rendering` into a simulated gateway response. The absent-integration case is now split out BEFORE that call and returned as the modeled `BadRequestException` on this operation's error channel, so the operation answers HTTP 400 / `BadRequestException` / `No integration defined for method`. The gateway-response rendering path stays for integrations that exist but cannot be performed.

Traces: apigateway.test-invoke-method-missing-integration-rejected

## Evidence
Expected: ClientError, HTTP 400, Code `BadRequestException`, Message `No integration defined for method` (all fixed text).
Actual (before the fix): no exception. HTTP 200, response:
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'status': 500,
   'body': '{"message":"Internal server error"}',
   'headers': {'Content-Type': 'application/json'},
   'log': "... : Integration type: none\n... : Execution failed due to configuration error: the method carries no integration, so there is nothing to call\n... : Answered through this API's API_CONFIGURATION_ERROR gateway response\n... : Method completed with status: 500", 'latency': 0}
Harness assertion: AssertionError: expected this call to fail, but it SUCCEEDED and returned {...}
(The same recording's control reads pass: GetIntegration after DeleteIntegration returns 404 NotFoundException / `Invalid Integration identifier specified`, and TestInvokeMethod returns status 200 both before the delete and after the integration is restored.)

## Steps to Reproduce
1. CreateRestApi; read the root resource id via GetResources; CreateResource pathPart="bare".
2. PutMethod httpMethod=GET authorizationType=NONE; PutMethodResponse statusCode=200.
3. PutIntegration type=MOCK requestTemplates={"application/json": '{"statusCode": 200}'}; PutIntegrationResponse statusCode=200 responseTemplates={"application/json": '{"message": "ok"}'}.
4. TestInvokeMethod(restApiId, resourceId, httpMethod=GET, pathWithQueryString="/bare") -> expect status 200 (positive control; passes).
5. DeleteIntegration on that method. GetIntegration -> 404 NotFoundException `Invalid Integration identifier specified` (passes).
6. TestInvokeMethod with the identical arguments as step 4 -> AWS refuses with 400 BadRequestException `No integration defined for method`; the emulator now answers exactly that.
7. PutIntegration/PutIntegrationResponse again -> TestInvokeMethod returns status 200 (restore leg).
