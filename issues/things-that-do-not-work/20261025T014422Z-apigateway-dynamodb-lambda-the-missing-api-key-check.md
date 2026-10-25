Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration
Issue-Key: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration|_poll_http#20|Timeout|time.sleep(interval_s)
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# apigateway__dynamodb__lambda: the missing-api-key check hangs to the 60 s cap — a deployed stage with apiKeyRequired never answers, so what reaches the integration cannot be observed

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose method has apiKeyRequired=true never produces a response, so the check polling the invoke URL runs to the harness 60 s cap.

AWS: a request that omits x-api-key against a method with apiKeyRequired=true is refused at the edge with HTTP 403 and the Lambda integration is NOT invoked — in particular the x-api-key header is never forwarded to the integration. Recorded behavior: a prompt 403 at the stage, and no invocation record for the backend function. Smithy: CreateDeployment (POST /restapis/{restApiId}/deployments, 201) publishes the stage whose invoke URL serves this traffic.

Emulator: the invoke never completes. The check hangs until the 60 s cap, so neither the 403 nor the "integration not invoked / key not delivered" half of the contract can be observed. This matches the emulator's general v1 data-plane gap (a stage invoke is not routed; elsewhere it answers 404 UnknownService), which is why a client that retries until the stage serves never converges.

Why: a client cannot verify that API-key enforcement happens before the integration, and a backend that reads x-api-key out of the forwarded headers cannot be tested at all. Any caller that waits for a deployed stage to start serving blocks indefinitely.

Fix: the execute-api data-plane listener for API Gateway v1 and the stage request pipeline's API-key gate, which must refuse with 403 before dispatching to the Lambda integration. The open issue 20261024T043741Z-apigateway-dynamodb-lambda-the-api-gateway-lambda-dynamodb.md reports 404 UnknownService for two other checks of this same tuple; it does not carry this check id.

Traces: apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration.

## Evidence
apigateway__dynamodb__lambda.apigw-missing-api-key-not-delivered-to-integration [mismatch]
Error: Check timed out after 60.0s

expected: HTTP 403 from the stage for a request with no x-api-key against a method with apiKeyRequired=true, and no invocation of the Lambda integration (the header is never delivered)
actual:   no response from the stage invoke URL within the 60 s harness cap

## Steps to Reproduce
1. CreateRestApi; PutMethod with apiKeyRequired=true and an AWS_PROXY Lambda integration; CreateDeployment with a stage.
2. Issue an HTTPS request to the stage invoke URL with no x-api-key header.
3. Observe that the request never completes (the check hits the 60 s cap) instead of a prompt 403 with the integration left uninvoked.
