Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-invocation-positive
Issue-Key: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-invocation-positive|run#20|AssertionError|assert resp["status"] == 200, resp
Issue-Commit: 946f146bc1cbb921fa2f003801745ea745af5a9e

# API Gateway AWS_PROXY integration pointing at a Lambda alias cannot invoke the backend, so TestInvokeMethod returns status 500 API_CONFIGURATION_ERROR instead of the function's 200 response

> _Note: this issue was identified against the code base at commit `946f146bc1cbb921fa2f003801745ea745af5a9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
1 cause in this issue.

=== Cause 1 of 1: an AWS_PROXY integration whose uri names a Lambda ALIAS is never invoked; the simulated invocation is refused as a configuration error ===

AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models TestInvokeMethod as returning `status` (Integer), `body`, `headers`, `multiValueHeaders`, `log` and `latency`; `status` is "The HTTP status code" of the simulated response. The model says nothing about Lambda invocation, so for the behavior itself: recorded behavior only. The recording shows a Lambda function published to version <v1>, its code then updated and published again as <v2>, an alias `live` created pointing at <v1>, and a REST API method with PutIntegration type=AWS_PROXY, integrationHttpMethod=POST and uri `arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<functionArn>:live/invocations`. Against real AWS, TestInvokeMethod on that method returns `status` 200 with the body the aliased function VERSION produces — i.e. the version the alias points at, not whatever $LATEST has moved on to.

Emulator: TestInvokeMethod returns HTTP 200 but with `status: 500`, `body: {"message":"Internal server error"}`, `headers: {"Content-Type":"application/json"}` and an execution log containing `Integration type: AWS_PROXY`, `Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map`, and `Answered through this API's API_CONFIGURATION_ERROR gateway response`. Generally: every AWS_PROXY integration naming a Lambda target is unserviceable, so neither the happy path nor alias-vs-$LATEST version resolution can be observed at all.

Why: a client sees a 500 Internal server error where AWS returns the function's own 200 and payload. Any serverless application whose front door is a Lambda proxy integration is unreachable end to end, and the specific guarantee this check exists for — that an alias ARN pins the invocation to the alias's target version even after $LATEST advances — is untestable, so a regression in alias resolution would go unnoticed.

Hypothesis: the underlying gap is the same one the open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md describes for the lambda service's own Invoke operation (it cannot execute a deployment package). That issue is not marked as owning this check and does not cover the API Gateway side: the integration's URI parsing, alias/version resolution and proxy request/response mapping are API Gateway's, and must produce the function's status, body and headers once an invocation is possible.

Fix: the integration-performing component shared by TestInvokeMethod and the execute-api data plane — `crate::service::invoke::perform_integration` in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/service/invoke.rs, called from services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/test_invoke_method.rs. It must resolve an `arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<fnArn>[:<alias|version>]/invocations` URI to the aliased function target, invoke it, and map the proxy-shaped result ({statusCode, body, headers}) onto the response, rather than classifying the integration as an uncallable backend and rendering the API_CONFIGURATION_ERROR gateway response.

Traces: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-invocation-positive

## Evidence
Check: apigateway__cloudformation__lambda.apigw-integration-lambda-alias-invocation-positive
Expected: TestInvokeMethod response with status == 200 and the aliased function version's body (marker "v1", the version alias `live` points at, even though $LATEST has advanced to v2).
Actual:
  {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'status': 500,
   'body': '{"message":"Internal server error"}',
   'headers': {'Content-Type': 'application/json'},
   'multiValueHeaders': {'Content-Type': ['application/json']},
   'log': "... : Integration type: AWS_PROXY\n... : Execution failed due to configuration error: the AWS_PROXY integration names a backend this emulator cannot call: no deployment package runs in process and no outbound connection is made, so no backend answer exists to map\n... : Answered through this API's API_CONFIGURATION_ERROR gateway response\n... : Method completed with status: 500",
   'latency': 0}
Harness assertion: AssertionError at `assert resp["status"] == 200`.
Status/code/message detail: the diverging values are the simulated `status` 500 and body `{"message":"Internal server error"}` — `{"message":"Internal server error"}` is API Gateway's fixed DEFAULT_5XX/API_CONFIGURATION_ERROR body text; no part of it comes from the request. AWS emits no such response here at all: it emits the Lambda's own statusCode and body.

## Steps to Reproduce
1. Create an IAM role for Lambda; CreateFunction with handler code returning {"statusCode": 200, "body": json.dumps({"marker": "v1"})}.
2. PublishVersion -> v1.
3. UpdateFunctionCode with handler code returning marker "v2"; poll GetFunction until Configuration.LastUpdateStatus == "Successful"; PublishVersion -> v2 (assert v1 != v2).
4. CreateAlias FunctionName=<fn> Name=live FunctionVersion=<v1>.
5. CreateRestApi + CreateResource pathPart="test"; PutMethod GET authorizationType=NONE.
6. PutIntegration type=AWS_PROXY, integrationHttpMethod=POST, uri="arn:aws:apigateway:<region>:lambda:path/2015-03-31/functions/<functionArn>:live/invocations"; lambda AddPermission for apigateway.amazonaws.com with SourceArn arn:aws:execute-api:<region>:<account>:<apiId>/*/*/*.
7. TestInvokeMethod(restApiId, resourceId, httpMethod=GET) -> AWS returns status 200 with body {"marker": "v1"}; the emulator returns status 500 with body {"message":"Internal server error"} and an API_CONFIGURATION_ERROR execution log.
