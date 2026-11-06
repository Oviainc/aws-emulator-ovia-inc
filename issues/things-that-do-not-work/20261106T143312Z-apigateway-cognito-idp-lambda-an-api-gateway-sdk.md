Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-positive apigateway__cognito-idp__lambda.lambda-apigateway-get-resources-positive apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-positive apigateway__cognito-idp__lambda.lambda-apigateway-update-stage-variables-positive apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-positive|run#7|AssertionError|assert body["ok"] is True, f"lambda call failed unexpectedly: {body}"
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-resources-positive|run#10|AssertionError|assert body["ok"] is True, f"lambda call to GetResources failed unexpectedly: {body}"
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-positive|run#9|AssertionError|assert body["ok"] is True, f"lambda-triggered deployment failed unexpectedly: {body}"
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-update-stage-variables-positive|run#11|AssertionError|assert body["ok"] is True, f"lambda-triggered UpdateStage failed unexpectedly: {body}"
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative|run#7|AssertionError|assert denied["error_code"] == "AccessDeniedException", denied
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative|run#10|AssertionError|assert denied["error_code"] == "AccessDeniedException", denied
Issue-Commit: b3aa6e7333d27a7c3c5b409bc7409fef676e9fb0

# apigateway__cognito-idp__lambda: an API Gateway SDK call made from inside a Lambda handler fails with a non-service exception (no Error.Code), so neither the success path nor the AccessDenied path can be observed

> _Note: this issue was identified against the code base at commit `b3aa6e7333d27a7c3c5b409bc7409fef676e9fb0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two net-new causes, both reached from the same shape of check: a python Lambda function whose handler builds a boto3 `apigateway` client under its execution role and calls one API Gateway control-plane operation, returning {'ok': True, ...} on success or {'ok': False, 'error_code': <Error.Code>} on a service error. The handler now runs (it is past the Runtime.ImportModuleError that open issue 20261105T111951Z-apigateway-cognito-idp-lambda-the-lambda-python-runtime.md describes for the boto3 import, and which still owns the lambda-cognito-* checks), but every API Gateway call it makes comes back as an exception that carries no `response['Error']['Code']` — i.e. not a modeled service error at all.

Cause 1 of 2: an API Gateway control-plane call issued from inside the Lambda runtime never reaches API Gateway, so a call that AWS answers 2xx produces a client-side exception instead.
AWS: Smithy (service-com-amazonaws-apigateway-backplanecontrolservice) models GetRestApis GET /restapis -> 200 RestApis{items,position}; GetResources GET /restapis/{restApiId}/resources -> 200 Resources{items,position}; CreateDeployment POST /restapis/{restApiId}/deployments -> 201 Deployment{id,description,createdDate,apiSummary}; UpdateStage PATCH /restapis/{restApiId}/stages/{stageName} -> 200 Stage{...,variables}. A handler in a managed python runtime whose role grants the action gets that modeled success response back, so the handler returns {'ok': True, ...} and the effect (a new deployment id visible to GetDeployments, the stage's updated `variables`) is observable from outside.
Emulator: the handler's boto3 call raises an exception with no `response.Error.Code`, so the handler returns {'ok': False, 'error_code': None} — no HTTP status, no error code, no error message is produced at all. Hypothesis: the runtime does not point the in-function boto3 client at the emulator's endpoint (or its bundled botocore has no apigateway service data / no credentials in the invocation environment), so the failure is a client-side botocore exception rather than a service response — the same class of defect as the bundled-botocore gap filed for bedrock-agent in 20261106T140826Z.
Why: a client that performs API Gateway control-plane work from a Lambda (CI/CD redeploys, stage-variable flips, resource discovery) sees the call fail with an exception it cannot classify and no effect lands, while on AWS the call succeeds; and because the exception is not a service error, retry/error-handling code keyed on an error code takes the wrong branch.
Fix: the Lambda runtime component that prepares the invocation environment for the bundled AWS SDK (endpoint/region/credential wiring and the bundled botocore's apigateway service data), not the apigateway handlers themselves — the same operations answer correctly when called directly by the suite.
Traces: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-positive, apigateway__cognito-idp__lambda.lambda-apigateway-get-resources-positive, apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-positive, apigateway__cognito-idp__lambda.lambda-apigateway-update-stage-variables-positive.

Cause 2 of 2: the denied path is not refused as an authorization failure — an in-function API Gateway call made under a role that lacks the action yields no AccessDeniedException.
AWS: recorded behavior only for the error shape (AccessDeniedException is not a modeled member of these operations in the apigateway Smithy model; it is the IAM authorization refusal the service returns ahead of the operation). AWS refuses the call with HTTP 403 and error code `AccessDeniedException`, whose message is the fixed template `User: <caller-arn> is not authorized to perform: <action> on resource: <resource-arn>` — the fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, and the variable parts are the assumed-role session ARN of the function's execution role, the action (`apigateway:POST`, `apigateway:GET`), and the resource ARN from the request (e.g. the `/restapis/<id>/deployments` ARN). No effect lands: GetDeployments still returns zero deployments. Granting the same action on the same role then makes the identical call succeed.
Emulator: the handler returns {'ok': False, 'error_code': None} both before and after the grant — there is no 403, no `AccessDeniedException` code and no message, so the refusal is indistinguishable from the cause-1 transport failure, and the before/after grant transition is unobservable. Note this overlaps the open IAM gap filed as 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md, which does not carry these checks; once cause 1 is fixed these two checks still need the authorization decision to be evaluated for an in-function caller and surfaced as the 403 AccessDeniedException above.
Why: a client relying on least-privilege behavior cannot tell a permission problem from an outage, and a negative test that asserts a denial passes or fails for the wrong reason; worse, a caller could conclude the action was denied when it was merely unreachable, or vice versa.
Fix: the authorization evaluation applied to API Gateway requests originating from a Lambda execution role, plus the serializer that renders the 403 AccessDeniedException body.
Traces: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative, apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative.

## Evidence
Cause 1 — expected: handler returns {'ok': True, ...} and the effect is visible (the returned deployment id appears in GetDeployments; the stage's `variables` reflect the PATCH). Actual (replay):
  lambda-apigateway-get-rest-apis-positive: AssertionError: lambda call failed unexpectedly: {'ok': False, 'error_code': None}
  lambda-apigateway-get-resources-positive: AssertionError: lambda call to GetResources failed unexpectedly: {'ok': False, 'error_code': None}
  lambda-apigateway-create-deployment-positive: AssertionError: lambda-triggered deployment failed unexpectedly: {'ok': False, 'error_code': None}
  lambda-apigateway-update-stage-variables-positive: AssertionError: lambda-triggered UpdateStage failed unexpectedly: {'ok': False, 'error_code': None}
The handler's own `except Exception` branch read `getattr(exc,'response',{}).get('Error',{}).get('Code')` and got None, so the exception raised inside the function was not a service error response (no HTTP status / error code / message was returned by the emulator at all).

Cause 2 — expected: HTTP 403 with error code AccessDeniedException and the message `User: <execution-role session ARN> is not authorized to perform: apigateway:GET|apigateway:POST on resource: <request's /restapis... ARN>`, hence error_code == 'AccessDeniedException'; then, after put_role_policy grants the same action on the same role, the identical call returns ok:True. Actual (replay):
  lambda-apigateway-get-rest-apis-missing-permission-negative: AssertionError: {'ok': False, 'error_code': None}
  lambda-apigateway-create-deployment-missing-permission-negative: AssertionError: {'ok': False, 'error_code': None}
No error code and no message were produced in either the denied or the granted phase.

## Steps to Reproduce
1. CreateRestApi (REGIONAL), read its root resource via GetResources, and add a MOCK GET method on the root.
2. CreateRole trusting lambda.amazonaws.com with NO apigateway permissions, and CreateFunction (python3.x) whose handler does: `import boto3; c = boto3.client('apigateway')` and then one of `c.get_rest_apis()`, `c.get_resources(restApiId=...)`, `c.create_deployment(restApiId=..., stageName='prod')`, `c.update_stage(restApiId=..., stageName='prod', patchOperations=[...variables...])`, returning {'ok': True, ...} or, on exception, {'ok': False, 'error_code': exc.response['Error']['Code']}.
3. Invoke the function. Observed: payload {'ok': False, 'error_code': None} — expected error_code 'AccessDeniedException' from a 403 refusal (cause 2).
4. PutRolePolicy granting the exact action (e.g. apigateway:POST on arn:aws:apigateway:<region>::/restapis/<id>/deployments*) on the SAME role, then Invoke again (poll a few times for propagation). Observed: still {'ok': False, 'error_code': None} — expected {'ok': True, ...} with the modeled success payload, and GetDeployments / GetStage showing the effect (cause 1).
5. For contrast, call the same four operations directly against the emulator with the suite's own client: they answer normally, which isolates the defect to API Gateway calls issued from inside the Lambda runtime.
