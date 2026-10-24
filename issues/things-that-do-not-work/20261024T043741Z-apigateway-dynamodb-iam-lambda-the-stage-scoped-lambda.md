Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage
Issue-Key: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage|_await#5|Timeout|time.sleep(3 if streak == 0 else 1)
Issue-Commit: 3ecafc116f2b44ac9fb7ddf17b83f092c778a529

# apigateway__dynamodb__iam__lambda: the stage-scoped lambda-permission rejection check hangs to the 60 s cap at the deployed stage (1 cause)

> _Note: this issue was identified against the code base at commit `3ecafc116f2b44ac9fb7ddf17b83f092c778a529`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: a SigV4-signed request to the deployed stage never produces a response.
AWS: when a lambda:InvokeFunction resource policy is scoped to one stage's source ARN, a request through a DIFFERENT stage is refused by API Gateway with an HTTP response (a 5xx integration failure, since the integration cannot be invoked) while the granted stage serves 200 — in both directions a well-formed HTTP status line arrives. Recorded behavior only for the exact statuses; the structural claim is that the stage answers.
Emulator: the check reports exactly 'Check timed out after 60.0s' — neither the granted nor the ungranted stage request returns, so the permission boundary is never exercised. Hypothesis: the dead execute-api data plane described in 20261023T210711Z-apigateway-api-gateway-data-plane-is-dead-30-checks.md; the sibling issue 20261023T210711Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-lifecycle.md covers a different key in this service.
Why: Lambda resource-policy scoping — the mechanism that keeps one stage from invoking another stage's function — is unobservable, and the caller hangs rather than receiving a denial.
Fix: the execute-api data-plane request path for a deployed stage (it must always produce a response) and the source-ARN check it must perform before invoking the integration's function.
Traces: apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.

## Evidence
Error: Check timed out after 60.0s — no HTTP status line from either the beta or the other stage's invoke URL.

## Steps to Reproduce
1. create_rest_api with an AWS_PROXY integration to a Lambda; deploy two stages (e.g. beta and prod).
2. lambda.add_permission with a SourceArn scoped to the beta stage only.
3. Issue SigV4-signed requests at both stages' invoke URLs.
4. Observe neither request returns within 60 s.
5. Or replay apigateway__dynamodb__iam__lambda.test-invoke-is-rejected-by-a-lambda-permission-scoped-to-the-beta-stage.
