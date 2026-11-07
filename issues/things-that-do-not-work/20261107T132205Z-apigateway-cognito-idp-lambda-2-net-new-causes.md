Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative|run#9|AssertionError|assert denied["ok"] is False, f"expected CreateDeployment to be denied, got: {denied}"
Issue-Key: apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative|run#6|AssertionError|assert denied["ok"] is False, f"expected the call to be denied without apigateway:GET, got: {denied}"
Issue-Commit: e4e3e5befb4453536c7145f8d5553fe1cd0770b8

# apigateway__cognito-idp__lambda: 2 net-new causes — API Gateway control-plane calls made with a Lambda execution role are never authorized, so CreateDeployment and GetRestApis succeed with no apigateway:POST / apigateway:GET grant

> _Note: this issue was identified against the code base at commit `e4e3e5befb4453536c7145f8d5553fe1cd0770b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
The API Gateway control plane performs no identity-based IAM authorization for requests signed with a Lambda execution role's credentials. A function whose role carries only AWSLambdaBasicExecutionRole (no apigateway:* statement at all) is served 2xx for API Gateway management calls that AWS refuses, so a negative permission-boundary scenario is indistinguishable from the granted positive control and the mutation actually lands.

This is NOT covered by the open issues that look adjacent. 20261106T143312Z-apigateway-cognito-idp-lambda-an-api-gateway-sdk.md and 20261105T111951Z-apigateway-cognito-idp-lambda-the-lambda-python-runtime.md describe in-function API Gateway SDK calls that FAIL (no AWS SDK / non-service exception with no Error.Code); here the in-function SDK call SUCCEEDS and returns a real API Gateway response — the defect is the missing authorization gate, not the runtime. 20261104T052719Z-apigateway-iam-aws-iam-authorization-at-a-deployed.md concerns AWS_IAM authorization on the execute-api DATA plane at a deployed stage, a different surface from the apigateway control-plane management API addressed here.

Cause 1 of 2: CreateDeployment is served with no apigateway:POST grant.
AWS: CreateDeployment is an IAM-authorized control-plane action (action apigateway:POST on resource arn:aws:apigateway:<region>::/restapis/<restApiId>/deployments). With no identity-based policy granting it, AWS refuses the request before any effect: HTTP 403 with x-amzn-ErrorType / Error.Code `AccessDeniedException` and the standard authorization message — fixed text "User: ", " is not authorized to perform: ", " on resource: ", " because no identity-based policy allows the ", " action"; variable parts are the caller's assumed-role session ARN, the action (apigateway:POST), and the resource ARN, all taken from the request's credentials and target. Smithy: CreateDeployment models BadRequestException(400), ConflictException(409), LimitExceededException(429), NotFoundException(404), ServiceUnavailableException(503), TooManyRequestsException(429), UnauthorizedException(401) — AccessDeniedException is not modeled on the operation, it is the control plane's framework-level authorization refusal; recorded behavior only for the 403/AccessDeniedException pairing, which is what the check asserts (error_code == "AccessDeniedException").
Emulator: the call is served 201 Created and returns a deployment id, so the handler's reported result is {'ok': True, 'deployment_id': '<id>'} instead of {'ok': False, 'error_code': 'AccessDeniedException'}. The deployment is also really created, so the follow-on GetDeployments is non-empty where AWS leaves it empty.
Why: a client that relies on IAM to contain a function's blast radius sees a deployment pushed to a stage by a role that was never granted apigateway:POST. Least-privilege tests pass against the emulator and fail in AWS, and the emulator cannot be used to validate a permission boundary at all — the negative arm and the granted positive arm of the same scenario are identical.
Fix: the CreateDeployment handler's authorization gate in the apigateway service — the control plane must resolve the signing identity, evaluate the identity-based policies attached to it for apigateway:POST on the deployments resource ARN, and refuse with the 403 AccessDeniedException envelope before the deployment is created in state.
Traces: check apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative — AssertionError: expected CreateDeployment to be denied, got: {'ok': True, 'deployment_id': ...}.

Cause 2 of 2: GetRestApis is served with no apigateway:GET grant.
AWS: GetRestApis is authorized as apigateway:GET on arn:aws:apigateway:<region>::/restapis. With no identity-based policy granting it, AWS refuses with HTTP 403 and Error.Code `AccessDeniedException`, same message template as Cause 1 with the action apigateway:GET and the /restapis resource ARN. Smithy: GetRestApis models BadRequestException(400), NotFoundException(404), TooManyRequestsException(429), UnauthorizedException(401); AccessDeniedException is the framework authorization refusal — recorded behavior only for the status/code pairing asserted by the check.
Emulator: the call is served 200 with the full item list, so the handler reports {'ok': True, 'ids': [...]} — the ungranted caller enumerates every REST API in the account/region.
Why: an ungranted principal reads the whole REST-API inventory. Besides breaking the refusal contract, this is an information-disclosure divergence: discovery calls that AWS blocks are answered in full, so a client cannot verify that a read-scoped role is actually read-scoped.
Fix: the GetRestApis handler's authorization gate in the apigateway service — evaluate apigateway:GET for the signing identity before listing, and refuse with the 403 AccessDeniedException envelope instead of serving items.
Traces: check apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative — AssertionError: expected the call to be denied without apigateway:GET, got: {'ok': True, 'ids': [...]}.

## Evidence
Cause 1 — check apigateway__cognito-idp__lambda.lambda-apigateway-create-deployment-missing-permission-negative:
  Setup: IAM role trusted by lambda.amazonaws.com with ONLY arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole attached (no inline apigateway statement). A python3.12 function with that role calls apigateway.create_deployment(restApiId=<api>, stageName='prod').
  Expected (AWS): HTTP 403, Error.Code AccessDeniedException, message "User: <assumed-role session ARN> is not authorized to perform: apigateway:POST on resource: arn:aws:apigateway:<region>::/restapis/<restApiId>/deployments because no identity-based policy allows the apigateway:POST action" (fixed text as quoted; ARN, action and resource are request/state values). No deployment created: GetDeployments items == [].
  Actual (emulator): HTTP 201 with a deployment id; handler returns {'ok': True, 'deployment_id': 'ab2cjd'} (id is a per-run value). Failure: `assert denied["ok"] is False` -> AssertionError: expected CreateDeployment to be denied, got: {'ok': True, 'deployment_id': 'ab2cjd'}. The deployment is also present in GetDeployments afterwards, so the refused mutation took effect.
  Positive control in the same check (put_role_policy granting apigateway:POST, then the identical call) succeeds — i.e. granted and ungranted are indistinguishable.

Cause 2 — check apigateway__cognito-idp__lambda.lambda-apigateway-get-rest-apis-missing-permission-negative:
  Setup: identical role (AWSLambdaBasicExecutionRole only); the function calls apigateway.get_rest_apis(limit=500).
  Expected (AWS): HTTP 403, Error.Code AccessDeniedException, message "User: <assumed-role session ARN> is not authorized to perform: apigateway:GET on resource: arn:aws:apigateway:<region>::/restapis because no identity-based policy allows the apigateway:GET action".
  Actual (emulator): HTTP 200 with the full items list; handler returns {'ok': True, 'ids': ['eocd2yo7iw']} (the id is a per-run value). Failure: `assert denied["ok"] is False` -> AssertionError: expected the call to be denied without apigateway:GET, got: {'ok': True, 'ids': ['eocd2yo7iw']}.

## Steps to Reproduce
1. Create an IAM role trusted by lambda.amazonaws.com and attach ONLY arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole — no apigateway statement, inline or attached.
2. Create a REST API (CreateRestApi) and give its root resource a MOCK GET method with a method response and integration response.
3. Create a python3.12 Lambda function with that execution role whose handler uses the AWS SDK to call, in two separate functions:
   a. apigateway.create_deployment(restApiId=<api id>, stageName='prod')
   b. apigateway.get_rest_apis(limit=500)
   returning {'ok': True, ...} on success and {'ok': False, 'error_code': <Error.Code>} on failure.
4. Invoke each function and read the payload.
   Expected: {'ok': False, 'error_code': 'AccessDeniedException'} for both (HTTP 403 on the wire), and GetDeployments(restApiId=<api id>) still returns items == [].
   Observed: {'ok': True, 'deployment_id': '<id>'} and {'ok': True, 'ids': [...]}, and the deployment is present in GetDeployments.
5. As the positive control, put_role_policy on the SAME role granting apigateway:POST (resource arn:aws:apigateway:<region>::/restapis/<api id>/deployments*) resp. apigateway:GET (arn:aws:apigateway:<region>::/restapis*) and re-invoke: the emulator's answer is unchanged, confirming the authorization decision is never taken.
