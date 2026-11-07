Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation
Issue-Key: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation|_request_until_logged#23|Timeout|status, _, text = _poll_http(
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigateway__lambda__logs: the Lambda-authorizer deny check never completes — the deny path and the "backend was not invoked" evidence in CloudWatch Logs are never observable within the check's budget

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke (execute-api data plane, TOKEN authorizer deny)

## Description
Cause 1 of 1: the scenario that proves a Lambda TOKEN authorizer's Deny decision blocks the backend invocation never reaches a verdict against this emulator — it runs to the suite's 180 s per-check cap with the asserted condition never observed.

AWS: a method with authorizationType=CUSTOM and a TOKEN authorizer whose policy Denies the request is refused at the stage with 403 / `x-amzn-errortype: AccessDeniedException`, and the backend integration is NOT invoked — which AWS makes observable through the absence of the backend function's invocation record in its CloudWatch Logs log group, while the authorizer function's own log group does receive a record. Recorded behavior only for the exact timings.

Emulator: the check makes no progress to its assertion inside 180 s (`Error: Check timed out after 180.0s`), so neither the 403 deny nor the "the backend's log group stayed empty" evidence is ever established. This sits on top of the authorizer-decision and execution-log defects this suite reports elsewhere for API Gateway: a stage whose authorizer/integration path or whose log delivery never settles leaves the observation the check depends on permanently unavailable.

Why: a client cannot verify the single most important property of a custom authorizer — that a Deny stops the request before the backend runs. An emulator that neither refuses nor records leaves an authorizer-protected API unverifiable, and a scenario that hangs instead of answering is worse than a wrong answer because callers get no status at all.

Fix: the execute-api authorizer path must reach a terminal decision (403 AccessDeniedException on Deny, without dispatching the integration) and the stage/Lambda execution-log delivery into CloudWatch Logs must settle, so the backend's log group can be read and shown to be empty.

Traces: apigateway__lambda__logs.apigw-lambda-authorizer-deny-blocks-backend-invocation.

## Evidence
Expected: the stage refuses the request with 403 (`x-amzn-errortype: AccessDeniedException`) and the backend function's CloudWatch Logs log group records no invocation for it, while the authorizer function's log group does.
Actual: `Error: Check timed out after 180.0s` — the check never reached its assertion, so no status, error code or log evidence was produced at all.

## Steps to Reproduce
1. Create a Lambda authorizer function returning a Deny policy and a backend Lambda that logs each invocation.
2. CreateRestApi + CreateResource + CreateAuthorizer(type="TOKEN", identitySource="method.request.header.Authorization"); PutMethod(authorizationType="CUSTOM", authorizerId=<id>); PutIntegration(AWS_PROXY to the backend); add the lambda:InvokeFunction permissions; CreateDeployment(stageName=s).
3. Call the stage URL with the deny-triggering Authorization value and then read both functions' CloudWatch Logs log groups.
4. Observe that the scenario does not complete within 180 s: the 403 refusal and the empty backend log group are never both observable.
