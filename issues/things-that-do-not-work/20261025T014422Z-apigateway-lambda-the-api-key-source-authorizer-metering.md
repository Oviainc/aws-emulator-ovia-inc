Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header
Issue-Key: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header|_poll_until#49|Timeout|time.sleep(interval_s)
Issue-Commit: 2c888af5aa33aca2a1e4835e22187ff259bac9bc

# apigateway__lambda: the API_KEY-source=AUTHORIZER metering check hangs to the 60 s cap — the deployed stage never answers, so the key the authorizer returns is never metered

> _Note: this issue was identified against the code base at commit `2c888af5aa33aca2a1e4835e22187ff259bac9bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a deployed REST API stage whose apiKeySource is AUTHORIZER never produces a response, so the check polling the invoke URL runs to the harness 60 s cap.

AWS: when a REST API's apiKeySource is AUTHORIZER, the API key used for usage-plan metering is the one the Lambda authorizer returns in its policy document's usageIdentifierKey, and the x-api-key request header is ignored for metering. Recorded behavior: the stage answers, and the usage recorded against the plan is attributed to the authorizer's key, not the header's. Smithy: CreateRestApi.apiKeySource targets the ApiKeySourceType enum (HEADER | AUTHORIZER); CreateDeployment (POST /restapis/{restApiId}/deployments, 201) publishes the stage that serves this traffic.

Emulator: the invoke never completes; the check hangs to the 60 s cap, so neither the authorizer's key being metered nor the header's key being ignored can be observed. This is the same v1 data-plane gap that elsewhere answers a stage invoke with 404 UnknownService.

Why: a client cannot verify which key its usage is billed/throttled against, and the AUTHORIZER key source — the whole point of per-caller metering behind a custom authorizer — is unobservable. Callers that retry until the stage serves block indefinitely.

Fix: the execute-api data-plane listener for API Gateway v1, plus the stage pipeline's apiKeySource=AUTHORIZER resolution (take usageIdentifierKey from the authorizer response and meter on it rather than on x-api-key). The open issues 20261023T170254Z-apigateway-lambda-three-integration-behaviour-checks-never-complete.md and 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md report hangs for other checks of this tuple; neither carries this check id.

Traces: apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header.

## Evidence
apigateway__lambda.apigw-api-key-source-authorizer-meters-the-key-from-the-authorizer-not-the-header [mismatch]
Error: Check timed out after 60.0s

expected: the stage answers, and usage is metered against the usageIdentifierKey returned by the Lambda authorizer (the x-api-key header being ignored)
actual:   no response from the stage invoke URL within the 60 s harness cap

## Steps to Reproduce
1. CreateRestApi with apiKeySource=AUTHORIZER; create a REQUEST/TOKEN Lambda authorizer that returns usageIdentifierKey in its policy; PutMethod with apiKeyRequired=true behind that authorizer; create an ApiKey, a UsagePlan and a UsagePlanKey for the authorizer's key.
2. CreateDeployment with a stage, then issue an HTTPS request to the stage invoke URL carrying a DIFFERENT x-api-key header value.
3. Observe that the request never completes (60 s cap) instead of being served and metered against the authorizer-supplied key.
