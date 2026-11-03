Issue-Severity: HIGH
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# A deployed REST stage with an IAM-authorized Lambda proxy integration never answers, so the backend-role-denied 500 error body is unreachable (check hangs to the 60 s cap)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
One net-new root cause.

Cause 1 of 1: a request to a deployed API Gateway REST stage whose AWS_PROXY integration targets a Lambda whose execution role lacks dynamodb:GetItem never produces an HTTP response at all, so the negative path cannot be observed.
AWS: a signed request to a deployed REST stage reaches the integration. When the backend function's execution role is not granted the DynamoDB read it attempts, the function raises, API Gateway maps that to an HTTP 500 response whose body is the gateway's integration-failure envelope, and the caller receives that status and body within the request timeout. The observable requirement is that the stage ANSWERS — a status and a body — not that it answers successfully.
Emulator: the request never completes. The check reaches no verdict and is terminated by the harness at its 60-second cap, so neither the 500 status nor the error body is ever produced. Nothing is returned to compare against AWS, which is why the divergence is "no response" rather than "wrong response".
Why: every negative-path contract behind a deployed stage is unobservable — a client cannot distinguish "the backend was denied" from "the gateway is down", retries and timeouts fire instead of error handling, and no authorization-failure behaviour can be developed or tested against the emulator. A hang is worse than an error: callers burn their full socket timeout.
Fix: the execute-api data plane for a deployed REST stage — the stage request dispatcher and its AWS_PROXY integration path must always complete a request, mapping an integration failure to the 500 response and body API Gateway serves, rather than blocking.
Traces: backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body. Several open issues describe deployed-stage non-response for other checks (20261102T090835Z-apigateway-dynamodb-lambda-logs-api-gateway-rest-proxy-integrations-never-reach-a.md, 20261102T121914Z-apigateway-dynamodb-iam-lambda-the-iam-authorization-e2e.md, 20261102T091623Z-apigateway-18-further-requests-to-a-deployed-rest.md); the replay reports this check as owned by none of them, so its replay key is carried here.

## Evidence
apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body — verdict: mismatch. Error: Check timed out after 60.0s. No HTTP status, no response body and no error code were produced by the deployed stage within the harness cap; AWS answers HTTP 500 with the gateway's integration-failure body for this request.

## Steps to Reproduce
Replay the behavioral suite and re-run apigateway__dynamodb__iam__lambda.backend-role-without-dynamodb-read-is-denied-and-surfaces-a-500-error-body. Minimal reproduction: create a REST API with a method whose integration type is AWS_PROXY targeting a Lambda whose execution role grants no dynamodb:GetItem on the table the handler reads; create a deployment and a stage; then issue a signed GET to https://<api-id>.execute-api.<region>.<host>/<stage>/<path> — expect an HTTP 500 response with the gateway's error body, observe that the request never completes.
