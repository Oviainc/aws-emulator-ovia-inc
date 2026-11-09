Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response
Issue-Key: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response|_poll#30|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# apigateway__ecr__lambda: a GET through a deployed API Gateway stage backed by a container-image Lambda never returns, so the proxy-response contract cannot be graded

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestApis

## Description
Cause 1 of 1: the deployed stage never answers the proxied request.
AWS: once a REST API with an AWS_PROXY integration to a Lambda function is deployed to a stage, a GET of the stage's invoke URL returns the function's API Gateway proxy response (statusCode/headers/body) within seconds, including when the function is packaged as a container image with a custom runtime. Recorded behavior only for the data plane.
Emulator: the polled GET through the deployed stage never produces a response — the check consumes its entire run budget and reports 'GET through the deployed stage: the run budget ended first; not graded', so the proxy-response contract (status, headers, body translation) is never exercised. The harness ran against the emulator and the emulator served nothing in the window; no runner tool or name-resolution problem is involved.
Why: a client that deploys a stage over a container-image Lambda gets no response at all: every request hangs instead of returning the handler's proxy response, and nothing downstream (status code, headers, base64 body handling) can be relied on.
Fix: the API Gateway stage data plane (execute-api request routing and the AWS_PROXY integration invoke of a container-image function, including its response translation). Related to the open issue 20261102T121914Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-e2e.md, which reports a stage GET producing no HTTP response for a different scenario and does not carry this check's replay key.
Traces: apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response.

## Evidence
TimeoutError: GET through the deployed stage: the run budget ended first; not graded
  at _poll(... 'GET through the deployed stage' ...)

Expected: the GET through the deployed stage returns the handler's API Gateway proxy response (status/headers/body) well inside the polling window.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Re-replay apigateway__ecr__lambda.container-image-custom-runtime-apigateway-proxy-response.
3. Or: push a container image to ECR, CreateFunction with PackageType=Image, wire a REST API resource/method with an AWS_PROXY integration to it, CreateDeployment to a stage, then GET the stage's invoke URL.
