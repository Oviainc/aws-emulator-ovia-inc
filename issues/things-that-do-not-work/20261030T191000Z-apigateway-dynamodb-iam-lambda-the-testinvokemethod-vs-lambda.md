Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_session#5|Timeout|c = sts.assume_role(RoleArn=role["arn"], RoleSessionName=_SESSION)["Credentials"]
Issue-Commit: 6b5a94ef1b44099f297a2ea92764072e31901e34

# apigateway__dynamodb__iam__lambda: the TestInvokeMethod-vs-Lambda-resource-policy check hangs to the 60 s cap and never produces a test-invoke result

> _Note: this issue was identified against the code base at commit `6b5a94ef1b44099f297a2ea92764072e31901e34`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: TestInvokeMethod on an AWS_PROXY (Lambda) integration never yields a result, and the AWS_IAM-authorized deployed stage that the same check must first exercise never answers, so the whole check exhausts the harness budget instead of returning a response. Failing check: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

AWS: TestInvokeMethod is a synchronous control-plane operation (Smithy: apigateway TestInvokeMethod, POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}, input restApiId/resourceId/httpMethod as @httpLabel @required plus pathWithQueryString/body/headers/stageVariables; output members status:Integer, body:String, headers, multiValueHeaders, log:String, latency:Long; modeled errors BadRequestException 400, NotFoundException 404, UnauthorizedException 401, TooManyRequestsException 429). It ALWAYS returns one of those: either a 200 response envelope carrying `status`, `body`, `headers`, `log` and `latency`, or one of the modeled errors. It never hangs. Recorded behavior only for the semantics this check grades: TestInvokeMethod bypasses the method's AWS_IAM authorization but NOT the integration target's own resource policy, and it invokes the function as if from the reserved stage name `test-invoke-stage`. With the Lambda function's resource policy granting apigateway.amazonaws.com only `SourceArn arn:aws:execute-api:<region>:<account>:<restApiId>/<stage>/GET/data-agreements/*` for the deployed stage, the recording shows the test invoke answering with envelope `status` 400-class-free but integration-failure 500 and body exactly `{"message": "Internal server error"}` — the fixed text is `Internal server error` under the JSON member `message`, with no variable part. After AddPermission adds a second statement whose SourceArn uses the reserved stage segment `test-invoke-stage` in place of the deployed stage name, the same test invoke answers envelope `status` 200 with the integration's JSON body. In the same recording, a SigV4-signed GET to the deployed stage invoke URL from a role allowed `execute-api:Invoke` on `<api-arn>/<stage>/GET/*` is served 200 throughout, independently of the test-invoke grant.

Emulator: nothing comes back at all. The replay detail is exactly `Error: Check timed out after 60.0s`. The check's first blocking wait is the signed AWS_IAM stage GET (`_await(... "the signed stage GET to be served")`, its own deadline 240 s), and its later waits poll TestInvokeMethod to 200; neither ever converges, so the harness cap fires before any status, error code or message is observed. No HTTP status and no error code were produced to compare.

Why: a client gets no answer where AWS always answers. Two distinct contracts become unobservable: (a) that TestInvokeMethod is synchronous and returns the `status`/`body`/`headers`/`log`/`latency` envelope (or a modeled 400/401/404/429) rather than stalling, and (b) that a test invoke is still gated by the integration target's resource policy and is attributed to the reserved `test-invoke-stage` stage segment — so a caller cannot tell a permission-scoped refusal (500 `{"message": "Internal server error"}`) from a success, and an SDK call blocks until its own socket timeout instead of being refused.

Fix: the apigateway TestInvokeMethod handler and the integration-invocation path it shares with the execute-api data plane — it must always synthesize and return the TestInvokeMethod response envelope, it must evaluate the Lambda function resource policy for the synthetic source ARN built with the reserved `test-invoke-stage` segment, and the execute-api data plane must always produce an HTTP response for a signed AWS_IAM request to a deployed stage instead of never replying.

Related but not covering this: open issue 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md describes the signed-AWS_IAM-stage hang for the sibling check e2e-debug-api-gateway-iam-authorization-lifecycle and carries only that check's replay key; it says nothing about TestInvokeMethod, the reserved test-invoke-stage source ARN, or the 500 `{"message": "Internal server error"}` refusal asserted here, and the replay report did not mark this check as owned by it.

Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Replay verdict: [mismatch] apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Detail, verbatim and complete: `Error: Check timed out after 60.0s`

Expected (AWS, per the check's recorded assertions and the Smithy model):
- The SigV4-signed GET to `<stage-invoke-url>/data-agreements/<id>` from the role allowed `execute-api:Invoke` on `<api-arn>/beta/GET/*` returns HTTP 200 (asserted for 2 consecutive probes).
- GetPolicy on the function lists exactly one statement, whose `Condition.ArnLike."AWS:SourceArn"` is `<api-arn>/beta/GET/data-agreements/*`.
- TestInvokeMethod(restApiId, resourceId, httpMethod=GET, pathWithQueryString=/data-agreements/<id>) returns an envelope with `status` == 500 and `body` == `{"message": "Internal server error"}` (fixed text `Internal server error` under member `message`; no variable part), because the only grant names the deployed stage and not the reserved `test-invoke-stage`.
- After AddPermission with `SourceArn <api-arn>/test-invoke-stage/GET/data-agreements/*`, TestInvokeMethod returns `status` == 200 and `body` equal to the stored item's JSON.

Actual (emulator): no response at any of those steps. The check produced no HTTP status, no error code and no body within the 60 s harness cap, so none of the four assertions above was ever reached. TestInvokeMethod is modeled to return 200 with the status/body/headers/log/latency envelope or one of BadRequestException(400) / UnauthorizedException(401) / NotFoundException(404) / TooManyRequestsException(429); it returned none of these.

## Steps to Reproduce
1. CreateTable with a single HASH key attribute, PAY_PER_REQUEST, and PutItem one item.
2. CreateRole trusting lambda.amazonaws.com with read access to that table; CreateFunction (python3.12) that reads the item keyed by the path parameter and returns a Lambda-proxy response.
3. CreateRestApi (REGIONAL), create nested resources `/data-agreements/{agreementId}`, PutMethod GET with authorizationType=AWS_IAM, PutIntegration type=AWS_PROXY integrationHttpMethod=POST pointing at the function.
4. Lambda AddPermission for Principal apigateway.amazonaws.com with SourceArn `arn:aws:execute-api:<region>:<account>:<restApiId>/beta/GET/data-agreements/*` ONLY (no `test-invoke-stage` grant). CreateDeployment with stageName=beta.
5. CreateRole trusted by the account, allowed `execute-api:Invoke` on `arn:aws:execute-api:<region>:<account>:<restApiId>/beta/GET/*`; AssumeRole and send a SigV4-signed GET to the stage invoke URL. Observe that no HTTP response arrives.
6. Call TestInvokeMethod(restApiId, resourceId of `{agreementId}`, httpMethod=GET, pathWithQueryString=/data-agreements/<id>). Observe that the call does not return the status/body envelope (AWS: status 500, body `{"message": "Internal server error"}`).
7. AddPermission a second statement with SourceArn `.../test-invoke-stage/GET/data-agreements/*` and repeat step 6 (AWS: status 200 with the item JSON).
8. Or replay apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.
