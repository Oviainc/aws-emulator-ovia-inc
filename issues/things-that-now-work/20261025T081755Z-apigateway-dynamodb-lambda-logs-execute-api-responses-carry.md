Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive|run#17|AssertionError|assert request_id, "response did not carry an x-amzn-RequestId header"
Issue-Key: apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive|_invoke_until_logged#10|AssertionError|assert request_id, "response did not carry an x-amzn-RequestId header"
Issue-Commit: ffece56e68e804f0450b8a83661b5d243139afbd

# apigateway__dynamodb__lambda__logs: execute-api responses carry no x-amzn-RequestId header, so access logs and execution logs can never be correlated (1 cause)

> _Note: this issue was identified against the code base at commit `ffece56e68e804f0450b8a83661b5d243139afbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: the REST data plane omits the x-amzn-RequestId response header, so no request id exists to look for in CloudWatch access or execution logs.

AWS: every response served by an API Gateway execute-api endpoint carries the request's id in the x-amzn-RequestId response header, and that same id is what the access-log $context.requestId variable and the execution-log stream record for the request. Recorded behavior only — the data plane is not described by the control-plane Smithy model — but both checks depend on nothing more than the header being present and matching the logged id, not on any particular id value (ids are generated per request and are never themselves a requirement).

Emulator: the response from the deployed stage carries no x-amzn-RequestId header at all, so both checks fail at the point where they capture the id ('response did not carry an x-amzn-RequestId header'), before any log assertion is reached.

Why: a client cannot correlate a response with its access-log or execution-log record, which is the only supported way to trace a single API Gateway request. Any tooling or test that reads x-amzn-RequestId (support workflows, log-based assertions, request tracing) gets nothing, and the logging features behind it are unverifiable even if the log records themselves are correct.

Fix: the execute-api data plane's response serialization — the per-request id must be generated for every request and emitted as the x-amzn-RequestId response header, and the same value must be the one used for $context.requestId in access-log formatting and in the execution-log records.

Traces: apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive, apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive.

## Evidence
Check apigateway__dynamodb__lambda__logs.apigateway-access-logs-custom-format-positive, line 79:
AssertionError: response did not carry an x-amzn-RequestId header

Check apigateway__dynamodb__lambda__logs.apigateway-execution-logging-to-cloudwatch-positive, line 209 (_invoke_until_logged):
AssertionError: response did not carry an x-amzn-RequestId header

Expected (AWS): the stage's HTTP response includes header x-amzn-RequestId whose value also appears as $context.requestId in the access-log record and in the execution-log stream.
Actual (emulator): the response headers contain no x-amzn-RequestId at all.

## Steps to Reproduce
1. Create a REST API with a working method/integration; UpdateAccount with an IAM role carrying AmazonAPIGatewayPushToCloudWatchLogs.
2. CreateDeployment and CreateStage, then UpdateStage to set accessLogSettings (custom format using $context.requestId) and/or methodSettings loggingLevel=INFO with dataTrace.
3. Send a request to the stage invoke URL and read the response headers.
4. Observe that x-amzn-RequestId is absent, so there is no id to match against the CloudWatch Logs access/execution records.
