Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_invoke#5|Timeout|return exc.code, {k.lower(): v for k, v in (exc.headers or {}).items()}, exc.read().decode()
Issue-Commit: 1db96a8030ce3dc5faa6b2c3a7e50004c443b0ec

# apigateway__dynamodb__iam__lambda: the test-invoke-vs-deployed-stage Lambda-permission check hangs to the 60 s cap — a signed AWS_IAM stage GET never reaches 200

> _Note: this issue was identified against the code base at commit `1db96a8030ce3dc5faa6b2c3a7e50004c443b0ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a SigV4-signed GET to a deployed REST API stage whose method is authorizationType=AWS_IAM with an AWS_PROXY Lambda integration never produces the served response, so the check exhausts the harness budget before it can reach its load-bearing TestInvokeMethod assertions.

AWS: a REST API deployed to a stage answers at its stage invoke URL. For a method with authorizationType=AWS_IAM and an AWS_PROXY integration, a request signed for the execute-api service by a principal whose identity policy allows execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/GET/* is authorized, the gateway invokes the target function with the proxy event, and the client receives the function's statusCode/headers/body — here 200 with the stored item. Recorded behavior only for the exact per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response, and an authorized signed request to a working proxy integration produces the integration's 200. Separately, Smithy (apigateway, com-amazonaws-apigateway-backplanecontrolservice): TestInvokeMethod POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}, output members status/body/headers/multiValueHeaders/log/latency, errors BadRequestException 400, NotFoundException 404, UnauthorizedException 401, TooManyRequestsException 429 — TestInvokeMethod bypasses the method's AWS_IAM authorizer but NOT the target function's resource policy, so with lambda:InvokeFunction granted only for SourceArn <api-arn>/beta/GET/data-agreements/* the test invoke must return status 200 with body {"message": "Internal server error"} carrying HTTP status 500 in the `status` member (fixed text "Internal server error"; no variable parts), and must return status 200 with the item's JSON once a second grant for SourceArn <api-arn>/test-invoke-stage/GET/data-agreements/* is added.

Emulator: the replayed check is reported as "Error: Check timed out after 60.0s". The check's first action after seeding is a bounded re-probe loop (_await, 240 s deadline, requires two consecutive 200s) around a SigV4-signed GET to the stage invoke URL; it never satisfies that predicate, so the signed stage GET either never answers or answers non-200 on every attempt, and the harness kills the check at its 60 s cap. Nothing downstream is reached: GetPolicy on the function, the no-grant TestInvokeMethod (expected 500/"Internal server error"), AddPermission for the test-invoke-stage source ARN, and the post-grant TestInvokeMethod (expected 200 + item) are never observed.

Why: a client that deploys an AWS_IAM-authorized Lambda-proxy method and signs a request to the stage gets no served response at all — it stalls rather than being refused, so retry/timeout logic cannot distinguish an authorization denial from an outage. The scope of a Lambda resource-policy grant (deployed stage vs the synthetic test-invoke-stage) is consequently unobservable, so code that relies on TestInvokeMethod failing with 500 until the test-invoke-stage grant exists silently appears to work either way.

Fix: the execute-api data-plane request path for REGIONAL REST APIs — SigV4 verification, AWS_IAM authorization at the stage, and dispatch of the AWS_PROXY integration must always terminate in an HTTP response; and the Lambda resource-policy evaluation applied to the integration's invoke (including the test-invoke-stage source ARN used by TestInvokeMethod) must be enforced in the TestInvokeMethod handler rather than left unmodelled. This is NOT covered by the open issues 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md (different check, different replay key) or 20261029T180026Z-apigateway-dynamodb-lambda-logs-api-gateway-deployed-stage-invoke-account-logging-role.md (different service group); the replay did not mark this check as owned by either.

Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Replay verdict: [mismatch] apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Detail is exactly: "Error: Check timed out after 60.0s".

Expected (AWS):
- Signed GET <stage-invoke-url>/data-agreements/<agreementId> by a role allowed execute-api:Invoke on <api-arn>/beta/GET/* -> HTTP 200 with the item JSON (two consecutive 200s inside a 240 s window).
- lambda GetPolicy on the function -> exactly one statement whose Condition.ArnLike."AWS:SourceArn" is <api-arn>/beta/GET/data-agreements/*.
- TestInvokeMethod(restApiId, resourceId, httpMethod=GET, pathWithQueryString=/data-agreements/<agreementId>) with only that grant -> response member status = 500 and body = {"message": "Internal server error"} (fixed text; no request- or state-derived parts).
- After AddPermission(StatementId=apigw-test-invoke-stage, Action=lambda:InvokeFunction, Principal=apigateway.amazonaws.com, SourceArn=<api-arn>/test-invoke-stage/GET/data-agreements/*) -> TestInvokeMethod status = 200 and body = the seeded agreement item.

Actual (emulator): the signed stage GET never yields the required 200 streak, so the check is still inside its first _await loop when the harness 60 s cap fires; no TestInvokeMethod response, no GetPolicy assertion and no post-grant behavior is ever produced.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST DynamoDB table keyed on agreementId and seed one item.
2. Create an IAM role trusted by lambda.amazonaws.com with read access to that table, and a python3.12 function reading the item by path parameter.
3. Create a REGIONAL REST API with GET /data-agreements/{agreementId}, authorizationType=AWS_IAM, type=AWS_PROXY integration pointing at the function.
4. Grant the function only lambda:InvokeFunction for Principal apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/beta/GET/data-agreements/* (deliberately omit the test-invoke-stage grant).
5. CreateDeployment to stage "beta".
6. Create a second IAM role allowed execute-api:Invoke on arn:aws:execute-api:<region>:<account>:<api-id>/beta/GET/*, AssumeRole it, and send a SigV4-signed GET to <stage-invoke-url>/data-agreements/<agreementId>. Observe that it does not return 200 (the check loops to the harness 60 s cap).
7. Then call TestInvokeMethod for that resource/method and observe whether status is 500 with body {"message": "Internal server error"}; add the test-invoke-stage AddPermission grant and observe whether TestInvokeMethod flips to status 200 with the item.
8. Or replay apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.
