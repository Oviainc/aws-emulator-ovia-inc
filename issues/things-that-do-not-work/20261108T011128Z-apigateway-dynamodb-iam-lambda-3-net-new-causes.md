Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body|_invoke#8|Timeout|with urllib.request.urlopen(request, timeout=35) as resp: # noqa: S310 - https stage URL
Issue-Key: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage|run#10|AssertionError|assert headers.get("x-amzn-errortype") == "InternalServerErrorException", (
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|run#11|AssertionError|assert (got["status"], _json(got["body"])) == (500, {"message": "Internal server error"}), (
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# apigateway__dynamodb__iam__lambda: 3 net-new causes — a signed AWS_PROXY stage GET never answers, the stage's 500 carries no x-amzn-ErrorType, and TestInvokeMethod ignores the backend function's resource policy

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
This issue carries 3 net-new root causes in the API Gateway REST data plane and its test-invoke surface.

Cause 1 of 3: a SigV4-signed GET of a deployed REST stage wired to an AWS_PROXY (Lambda proxy) integration never produces an HTTP response, so neither the admitted 200 nor the 500 that follows the removal of the function role's DynamoDB read grant is ever observed.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body.
AWS: recorded behavior only — the execute-api data plane is not in the API Gateway control-plane Smithy model, which covers only the setup calls (CreateRestApi, PutMethod authorizationType=AWS_IAM, PutIntegration type=AWS_PROXY, CreateDeployment/CreateStage, lambda AddPermission for apigateway.amazonaws.com, iam DeleteRolePolicy). The recording shows every request that reaches a deployed stage terminating in a response: a caller allowed execute-api:Invoke on <api-arn>/<stage>/GET/* is answered HTTP 200 with the handler's JSON body, and after the function role's table-read policy is deleted the same signed GET is answered HTTP 500 whose JSON `error` field holds the handler's botocore text — fixed text "An error occurred (AccessDeniedException) when calling the GetItem operation: User: <principal> is not authorized to perform: dynamodb:GetItem on resource: <table-arn> because no identity-based policy allows the dynamodb:GetItem action", with <principal> the assumed-role session arn:aws:sts::<account>:assumed-role/<function-role>/<function-name> and <table-arn> the table from the request.
Emulator: no status line, headers or body are ever produced on the proxy path; the check exhausts the replay budget and the suite reports 'Error: Check timed out after 180.0s'.
Why: a client of a proxy-integrated REST API hangs until its own socket timeout instead of receiving a status, and cannot distinguish a served request from one whose backend was IAM-denied.
Fix: the execute-api data-plane request path of the API Gateway service — deployed stage/route resolution, SigV4 verification at the stage, the AWS_PROXY invoke of the target function, and mapping the handler's proxy response back to the HTTP response.
Related open issue on this surface that the suite does NOT report as owning this check, so this cause is filed here and should be closed together with it: 20261107T043755Z-apigateway-dynamodb-iam-lambda-a-signed-get-to.md (and 20261103T014646Z-apigateway-dynamodb-iam-lambda-a-deployed-rest-stage.md).

Cause 2 of 3: once API Gateway may no longer invoke the backend, the stage's 500 is produced without the x-amzn-ErrorType response header.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.removing-the-invoke-permission-is-rejected-with-a-500-at-the-stage.
AWS: recorded behavior only. After lambda RemovePermission drops the function's apigateway.amazonaws.com grant for the stage, the same signed GET is answered HTTP 500 with the response header x-amzn-ErrorType: InternalServerErrorException and body {"message": "Internal server error"}. Both the header value and the body are fixed text for this error class; nothing in them is run-specific.
Emulator: the 500 status is reached but x-amzn-ErrorType is absent — the check reads it as None (AssertionError: x-amzn-ErrorType was None).
Why: SDKs and clients classify API Gateway failures from x-amzn-ErrorType; with the header missing the refusal is indistinguishable from any other 500 and error-type-based retry/alerting never triggers.
Fix: the API Gateway stage-invoke error serializer — the integration-failure (backend not permitted) path must emit x-amzn-ErrorType alongside the 500 body.

Cause 3 of 3: TestInvokeMethod does not evaluate the backend function's resource policy, so a test invoke succeeds where the policy does not grant the test-invoke stage.
Failing check ids for this cause: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.
AWS: Smithy: TestInvokeMethod (apigateway) returns TestInvokeMethodResponse with `status` and `body`. Recorded behavior: TestInvokeMethod bypasses the method's AWS_IAM authorization but NOT the target function's resource policy. With the only lambda permission scoped to SourceArn <api-arn>/<stage>/GET/<resource-path>/*, the test invoke is reported as status 500 with body {"message": "Internal server error"} (fixed text for this class); only after an AddPermission whose SourceArn names the <api-arn>/test-invoke-stage/... path does the test invoke report 200 with the handler's item.
Emulator: the test invoke reports status 200 and the handler's item body while the function's policy grants only the deployed stage — the function's resource policy is never consulted for the test-invoke pseudo-stage.
Why: the whole negative class is unobservable: a user testing a method in the console/CLI is told the wiring works when a real invoke from the test-invoke stage would be refused, so a missing lambda:InvokeFunction grant ships undetected.
Fix: the TestInvokeMethod handler — resolve the test-invoke-stage source ARN and evaluate the target function's resource policy before invoking, returning status 500 with the generic body when it does not allow it.

Traces: the three failing check ids above, as examples.

## Evidence
Cause 1 — Expected: signed GET on the deployed stage -> 200 with the seeded item; after iam DeleteRolePolicy -> 500 with the handler's AccessDeniedException text in the JSON `error`. Actual: no HTTP response at all; 'Error: Check timed out after 180.0s'.

Cause 2 — Expected: HTTP 500 with header x-amzn-ErrorType: InternalServerErrorException and body {"message": "Internal server error"}. Actual: AssertionError: x-amzn-ErrorType was None (the 500 was reached, the header was absent).

Cause 3 — Expected: (status, body) == (500, {"message": "Internal server error"}) for TestInvokeMethod while lambda GetPolicy shows the single statement with ArnLike AWS:SourceArn = <api-arn>/beta/GET/data-agreements/*. Actual: AssertionError: the test invoke without a test-invoke-stage grant returned 200 with the handler's item JSON.

## Steps to Reproduce
1. Create a DynamoDB table and a python3.12 function that GetItems it; create a REGIONAL REST API with GET /data-agreements/{agreementId}, authorizationType=AWS_IAM, AWS_PROXY integration to the function; lambda AddPermission for apigateway.amazonaws.com with SourceArn <api-arn>/beta/GET/data-agreements/*; deploy stage beta; seed one item.
2. Cause 1: assume a role allowed execute-api:Invoke on <api-arn>/beta/GET/* and send a SigV4-signed GET to the stage URL — no response arrives; then iam DeleteRolePolicy the function role's table-read policy and repeat — again no response.
3. Cause 2: with the stage answering, call lambda RemovePermission for the apigw-beta statement and repeat the signed GET — the 500 arrives without x-amzn-ErrorType: InternalServerErrorException.
4. Cause 3: with the permission scoped to the beta stage only, call apigateway TestInvokeMethod on the GET method — it answers 200 with the item instead of 500 {"message": "Internal server error"}; adding a permission whose SourceArn names <api-arn>/test-invoke-stage/GET/data-agreements/* must be what flips it to 200.
5. Or replay the check ids in trace_ids.
