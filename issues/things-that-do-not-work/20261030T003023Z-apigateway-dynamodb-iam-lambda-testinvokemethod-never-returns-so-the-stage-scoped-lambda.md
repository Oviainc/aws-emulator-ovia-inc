Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_invoke#5|Timeout|with urllib.request.urlopen(request, timeout=35) as resp: # noqa: S310 - https stage URL
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# TestInvokeMethod never returns, so the stage-scoped Lambda-permission rejection path cannot complete

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: ApiGateway TestInvokeMethod against a deployed AWS_PROXY method never produces a response, so the check that exercises the test-invoke permission boundary is killed at the 60 s check budget.

AWS: Smithy (com.amazonaws.apigateway, TestInvokeMethod) models POST /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod} returning HTTP 200 with a TestInvokeMethodResponse whose members are status (Integer), body (String), headers, multiValueHeaders, log (String) and latency (Long); its only modeled failures are BadRequestException (400), NotFoundException (404), TooManyRequestsException (429) and UnauthorizedException (401). TestInvokeMethod bypasses the method's AWS_IAM authorizer but NOT the integration target's resource policy: when the Lambda resource policy grants apigateway.amazonaws.com only <execute-api-api-arn>/<stage>/GET/<path>, the test invoke (which presents the synthetic stage name test-invoke-stage) is not authorized to invoke the function, and API Gateway reports that as a translation failure in the response envelope — status 500 with body {"message": "Internal server error"} — not as a modeled exception. The fixed text there is the JSON object with the single member "message" whose value is the literal "Internal server error"; nothing in it is request-derived. After lambda:AddPermission adds a statement whose SourceArn is <execute-api-api-arn>/test-invoke-stage/GET/<path>, the same TestInvokeMethod call returns status 200 and body carrying the integration's JSON document (here the DynamoDB item the handler read). In every case AWS answers within one request; there is no asynchronous settling for TestInvokeMethod itself.

Emulator: the call never completes. The replayed check is reported as `mismatch` with the single detail `Error: Check timed out after 60.0s` — no status, no body, no error code was ever produced for either the un-granted (expected 500) or the granted (expected 200) test invoke, and the preceding signed stage GET poll likewise never converges. Hypothesis: the emulator's TestInvokeMethod path blocks on an integration invocation that never settles rather than returning an envelope.

Why: a client cannot use TestInvokeMethod at all — no response, not even an error, so SDK callers hang until their own socket/retry deadline. Worse, the behaviour erases the distinction this call exists to show: whether a 500 came from a resource-policy refusal of the integration or from a missing method, since neither verdict is ever delivered. Permission-boundary testing of REST APIs (the standard pre-deploy smoke of a Lambda-proxied method) is impossible.

Fix: the apigateway TestInvokeMethod handler and the integration-dispatch path it calls — it must synthesise the request against the named resource/method, evaluate the Lambda resource policy for the test-invoke-stage source ARN, and always return a TestInvokeMethodResponse (status/body/headers/log/latency) rather than awaiting an invocation that never resolves.

Related open issue 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md covers the IAM-authorization lifecycle check in this same service group; it does not carry this check's replay key, and this failure is on TestInvokeMethod rather than on the signed data-plane GET, so it is filed separately.

Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage (verdict mismatch, `Check timed out after 60.0s`).

## Evidence
Expected (real AWS): TestInvokeMethod returns HTTP 200 with status=500 and body={"message": "Internal server error"} while the Lambda resource policy grants only <api-arn>/beta/GET/<path>; after AddPermission for SourceArn <api-arn>/test-invoke-stage/GET/<path>, the identical TestInvokeMethod returns status=200 with the integration's JSON body. A signed GET on the deployed beta stage returns 200 throughout.

Actual (emulator): no response is ever produced. Suite verdict for apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage is `mismatch` with detail `Error: Check timed out after 60.0s`; the check is terminated by the harness budget before any status/body/error is observed.

## Steps to Reproduce
1. Create a PAY_PER_REQUEST DynamoDB table and seed one item.
2. Create a python3.12 Lambda that reads the item, and a REGIONAL REST API with GET /data-agreements/{agreementId}, authorizationType=AWS_IAM, integration type AWS_PROXY pointing at the function.
3. lambda:AddPermission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<api-id>/beta/GET/data-agreements/* only; deploy to stage 'beta'.
4. Call apigateway TestInvokeMethod(restApiId=<api-id>, resourceId=<child-id>, httpMethod='GET', pathWithQueryString='/data-agreements/<id>') — observe that the call never returns instead of answering status 500 / {"message": "Internal server error"}.
5. AddPermission for SourceArn .../test-invoke-stage/GET/data-agreements/* and repeat step 4 — observe that it still never returns instead of answering status 200 with the item.
