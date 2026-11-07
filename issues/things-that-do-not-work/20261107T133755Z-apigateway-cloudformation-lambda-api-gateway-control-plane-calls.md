Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-missing-permission-negative
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative|run#13|AssertionError|assert denied.get("error") == "AccessDeniedException", f"unexpected result: {denied}"
Issue-Key: apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-missing-permission-negative|run#13|AssertionError|assert denied.get("error") == "AccessDeniedException", f"unexpected result: {denied}"
Issue-Commit: d9e6cac456a6ddf38a11f2ddf4f109e13159427a

# apigateway__cloudformation__lambda: API Gateway control-plane calls are not IAM-authorized — CreateDeployment and GetRestApis succeed for a caller whose role grants no apigateway action (2 causes)

> _Note: this issue was identified against the code base at commit `d9e6cac456a6ddf38a11f2ddf4f109e13159427a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
The emulator's API Gateway control plane applies no IAM authorization to the calling identity: a Lambda execution role that carries no policy statement at all is served the operation's success response instead of being refused. Two operations are grounded here; each needs the authorization gate on its own handler path, so they are listed as two causes. This is the API Gateway control plane (apigateway:GET/apigateway:POST on /restapis...), NOT the execute-api data plane that the open issue 20261104T052719Z-apigateway-iam-aws-iam-authorization-at-a-deployed.md covers, and not the in-Lambda SDK availability that 20261105T111951Z-apigateway-cloudformation-lambda-the-lambda-python-runtime-has.md covers (in both checks the in-function SDK call reached API Gateway and got a 2xx answer back, which is exactly the defect).

Cause 1 of 2 — CreateDeployment is not authorized.
AWS: CreateDeployment is POST /restapis/{restApiId}/deployments (Smithy: service-com-amazonaws-apigateway-backplanecontrolservice; input restApiId @httpLabel @required, stageName; success 201 Deployment{id,...}; modeled errors BadRequestException 400, ConflictException 409, LimitExceededException 429, NotFoundException 404, ServiceUnavailableException 503, TooManyRequestsException 429, UnauthorizedException 401). AccessDeniedException is not modeled per-operation: it is the SigV4/IAM authorization refusal the request framework returns before the operation runs — HTTP 403 with error code AccessDeniedException (recorded behavior only; the recording asserts the code, not the message). AWS's implicit-deny message for this class has the fixed text "User: " … " is not authorized to perform: " … " on resource: " … " because no identity-based policy allows the " … " action", whose variable parts come from the request and the caller's identity: the caller ARN (the assumed-role session of the function's execution role), the action apigateway:POST, and the resource ARN arn:aws:apigateway:<region>::/restapis/<restApiId>/deployments. Because the call is refused, no deployment is created, so GetDeployments for that API still returns an empty items list.
Emulator: a function whose execution role has an empty policy set calls CreateDeployment and is answered the 201 success body — the handler returned {"deployment_id": "<minted id>"} to the in-function SDK (replay detail: AssertionError: unexpected result: {'deployment_id': 'br3zrg'}; the id value itself is per-run). No 403/AccessDeniedException is produced and the deployment lands on the API.
Why: a client cannot test or rely on least-privilege. Every negative-path test of an apigateway grant passes vacuously, a role that must not be able to publish a stage can publish one, and code that branches on AccessDeniedException never takes that branch — the emulator reports a permissive world that real AWS refuses.
Fix: the caller-identity authorization gate for the API Gateway control plane — CreateDeployment's handler/request-authorization path in the apigateway service, evaluating the invoking principal's identity policy for apigateway:POST on the deployments resource ARN before the operation mutates state.
Traces: apigateway__cloudformation__lambda.lambda-sdk-apigateway-create-deployment-missing-permission-negative.

Cause 2 of 2 — GetRestApis is not authorized.
AWS: GetRestApis is GET /restapis (Smithy: paginated on position/limit; success 200 RestApis{items}; modeled errors BadRequestException 400, NotFoundException 404, TooManyRequestsException 429, UnauthorizedException 401). As above, an identity with no apigateway:GET grant is refused by IAM with HTTP 403, error code AccessDeniedException (recorded behavior only), message of the same implicit-deny form with action apigateway:GET and resource arn:aws:apigateway:<region>::/restapis, and the response carries no API data at all.
Emulator: the policy-less function's GetRestApis call is answered 200 with the full items list, leaking every REST API's name to an identity that is granted nothing (replay detail: AssertionError: unexpected result: {'names': ['sim2real-cfa-api-3adf65e38092']}; the name is per-run).
Why: a read that AWS refuses returns the account's API inventory, so any permission-boundary or data-exposure assertion built on apigateway:GET is unenforceable against the emulator.
Fix: the same caller-identity authorization gate, applied on GetRestApis's handler/read path in the apigateway service.
Traces: apigateway__cloudformation__lambda.lambda-sdk-apigateway-get-rest-apis-missing-permission-negative.

## Evidence
Cause 1 — CreateDeployment.
Expected (AWS): the call from a role with no apigateway grant is refused HTTP 403, Error.Code "AccessDeniedException" (message of the form "User: <caller arn> is not authorized to perform: apigateway:POST on resource: arn:aws:apigateway:<region>::/restapis/<restApiId>/deployments because no identity-based policy allows the apigateway:POST action" — fixed text as quoted, with caller ARN, action and resource ARN taken from the request/identity), no deployment created, GetDeployments(restApiId) -> items == [].
Actual (emulator): 201 success; the in-function handler returned {'deployment_id': '<id>'} (replay: "AssertionError: unexpected result: {'deployment_id': 'br3zrg'}" at check line 57, assert denied.get("error") == "AccessDeniedException"), i.e. the deployment was created for an unauthorized caller. A sibling function whose role DOES grant apigateway:POST succeeded identically, so the two arms are indistinguishable.

Cause 2 — GetRestApis.
Expected (AWS): HTTP 403, Error.Code "AccessDeniedException" (same message form, action apigateway:GET, resource arn:aws:apigateway:<region>::/restapis), and no items in the response.
Actual (emulator): 200 with the API inventory; the handler returned {'names': [<every rest api name>]} (replay: "AssertionError: unexpected result: {'names': ['sim2real-cfa-api-3adf65e38092']}" at check line 56).

Both checks' positive controls (identical call from a role granted the one action) succeed, and the restore beat (granting the same action to the previously denied role) is never reached because the denial never happens.

## Steps to Reproduce
1. Create a REST API (apigateway CreateRestApi) with a root resource and a MOCK method so it is deployable.
2. Create two IAM roles trusted by lambda.amazonaws.com: role A with an inline statement {Effect: Allow, Action: "apigateway:POST", Resource: "*"} (resp. "apigateway:GET" for cause 2), role B with no policies at all.
3. Create two Lambda functions from the same zip, one per role, whose handler calls apigateway.create_deployment(restApiId=<api>, stageName="prod") (cause 1) or apigateway.get_rest_apis(limit=500) (cause 2) and returns {"error": <ClientError code>} on failure.
4. Invoke the function on role A: it succeeds (positive control).
5. Invoke the function on role B.
   Expected: the SDK raises ClientError with HTTP 403 and code AccessDeniedException, so the handler returns {"error": "AccessDeniedException"}; for cause 1 GetDeployments on the target API still returns zero deployments.
   Actual: the handler returns {"deployment_id": "<id>"} (cause 1) / {"names": [...]} (cause 2) — the unauthorized call is served, and for cause 1 the deployment is really created.
