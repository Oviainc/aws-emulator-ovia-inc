Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected
Issue-Key: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__lambda: the stage-variable integration check hangs to the 60 s cap — a stage-variable-selected Lambda integration never answers (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a request to a stage whose integration uri is built from a stage variable never produces a response.
AWS: one deployment served by two stages with different stage variables resolves the integration uri per stage; the stage whose selected function has granted API Gateway lambda:InvokeFunction serves the function's 200 response, and the stage selecting an ungranted function is refused with an HTTP response (a 5xx integration failure). In both directions a well-formed HTTP status line arrives. Recorded behavior only for the exact statuses.
Emulator: the check reports exactly 'Check timed out after 60.0s' — neither stage request returns, so neither the stage-variable substitution nor the invoke-permission refusal is ever observed. Hypothesis: the dead execute-api data plane described in 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md; the sibling issues 20261023T170254Z-apigateway-lambda-three-integration-behaviour-checks-never-complete.md and 20261023T210711Z-apigateway-lambda-four-integration-authorizer-behaviour-checks-hang.md cover other keys in this service, not this one.
Why: a client cannot use stage variables to point one deployment at per-stage backends, and gets a hang rather than the denial AWS returns when the selected function has not granted invoke.
Fix: the execute-api data-plane request path — stage/route resolution, stage-variable substitution in the integration uri, and integration dispatch; it must always produce a response.
Traces: apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected.

## Evidence
Error: Check timed out after 60.0s — no HTTP status line from either stage's invoke URL.

## Steps to Reproduce
1. create_rest_api with an AWS_PROXY integration whose uri interpolates ${stageVariables.fn}; create one deployment and two stages with different variables values.
2. Grant lambda:InvokeFunction for only one of the two functions.
3. GET each stage's invoke URL.
4. Observe neither request returns within 60 s.
5. Or replay apigateway__lambda.deep-apigw-integration-stage-variable-selects-lambda-ungranted-function-rejected.
