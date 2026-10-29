Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage
Issue-Key: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway|_poll_http#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# API Gateway stage variables and malformed-proxy-response handling never answer (2 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

_Status update:_ cause 2's rule is implemented. `src/service/invoke.rs::with_stage_variables`
resolves every `${stageVariables.<name>}` reference in an integration's configuration — its
`uri`, its `connectionId` and its `credentials` — against the INVOKED stage's own `variables`
map before the integration is read, so one deployment served by two stages addresses each
stage's own backend, and a name the invoked stage defines no variable for is answered through
the api's `API_CONFIGURATION_ERROR` gateway response instead of being addressed at the text
left behind. `TestInvokeMethod` resolves the same configuration against the `stageVariables`
map its caller asks it to simulate. The suite
`tests/all/stage_variable_integration_target.rs` asserts that over a table of reference
positions, members and per-stage maps, end to end on the invoke host.

Both replay keys above STILL FAIL, for one remaining reason that is not the substitution: each
recorded check's per-stage target is a LAMBDA PROXY integration, so both need the function's
deployment package to actually run — cause 1 needs the malformed proxy response a function
returns, and the recorded cause 2 needs the per-stage function's DynamoDB write. Nothing in
this emulator runs a deployment package (`Invoke` answers 501 for `RequestResponse`:
`services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs`), so there is no
backend proxy response to validate and no 502 is emitted for work nothing ran. That cause is
owned by the open issue
`issues/things-that-do-not-work/20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md`.

## Operation
TestInvokeMethod

## Description
Two net-new root causes in the apigateway__dynamodb__lambda group. Each needs its own fix.

Cause 1 of 2: a malformed Lambda-proxy response is not turned into a 502.
AWS: when the backend returns a body that is not a valid proxy response, API Gateway answers the caller `502 Bad Gateway` with `{"message": "Internal server error"}` and logs the mapping failure. (Recorded behavior only for the log text; the 502 status is the requirement.)
Emulator: the check never gets an answer — `Check timed out after 60.0s`.
Why: a client cannot observe the gateway's error contract for a broken backend; the request simply hangs instead of failing fast.
Fix: the apigateway integration-response mapper (proxy-response validation and the 502 gateway response).
Traces: apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway

Cause 2 of 2: stage variables do not isolate the integration target per stage.
AWS: `${stageVariables.<name>}` in an integration URI resolves per stage, so two stages of one API address two different DynamoDB tables.
Emulator: the check hangs to the 60s budget — no stage ever answers, so the per-stage isolation cannot be observed.
Why: the standard dev/prod stage-variable pattern is unusable, and a request intended for one stage's backend cannot be shown to stay out of the other's.
Fix: the apigateway invoke path's stage-variable substitution in the integration URI.
Traces: apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage

## Evidence
- apigw-lambda-proxy-malformed-response-bad-gateway: [mismatch] Error: Check timed out after 60.0s (expected HTTP 502 with body {"message": "Internal server error"}).
- apigw-stage-variables-isolate-ddb-target-per-stage: [mismatch] Error: Check timed out after 60.0s (expected each stage's write to land in its own table).

## Steps to Reproduce
Boot the emulator and replay apigateway__dynamodb__lambda.apigw-lambda-proxy-malformed-response-bad-gateway and apigateway__dynamodb__lambda.apigw-stage-variables-isolate-ddb-target-per-stage. Both currently consume the full 60s budget without an answer.
