Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs
Issue-Key: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs|_wait_for_status#18|Timeout|return last
Issue-Commit: edd8d27229a8bc99e308a4b32d393f13d6f32cb7

# apigateway__dynamodb__logs: method-setting loggingLevel/dataTrace never gates execution logs — the check never completes

> _Note: this issue was identified against the code base at commit `edd8d27229a8bc99e308a4b32d393f13d6f32cb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: per-method settings that control execution logging have no observable effect, and the check hangs to the harness cap.
AWS: Smithy: apigateway UpdateStage/GetStage carry `methodSettings` (MapOfMethodSettings) whose members include `loggingLevel` (OFF|ERROR|INFO) and `dataTraceEnabled`. API Gateway writes execution-log events for a stage into the CloudWatch log group `API-Gateway-Execution-Logs_<api-id>/<stage>`: with loggingLevel=ERROR only error-level lines appear, with INFO the full per-request trace appears, and with dataTraceEnabled=true the request/response bodies are additionally logged. Turning the level OFF stops delivery.
Emulator: nothing is delivered for any setting, so the check polling for the gated log lines never reaches a verdict and is killed at the harness cap (`Check timed out after 180.0s`). No execution-log group content is observable at any loggingLevel.
Why: a client cannot use execution logging to diagnose a method, and cannot verify that a stage's logging level is in force — a security-relevant setting (dataTrace logs bodies) that reads the same whether it is on or off.
Fix: the apigateway stage execution-logging emitter and its methodSettings gate (the component that should write API-Gateway-Execution-Logs_<api-id>/<stage> events per request).
Traces: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs. Related open issue: 20261104T044826Z-apigateway-dynamodb-lambda-logs-api-gateway-stage-logging-checks-never-complete-and.md — it does not carry this check's replay key.

## Evidence
Expected: execution-log events in API-Gateway-Execution-Logs_<api-id>/<stage>, present at loggingLevel=INFO (with bodies when dataTraceEnabled=true), reduced at ERROR, absent at OFF. Actual: `Error: Check timed out after 180.0s` — the poll for the gated log lines never observes any execution-log content.

## Steps to Reproduce
Create a REST API with a mock/AWS method, deploy to a stage, UpdateStage with methodSettings {"*/*": {loggingLevel: INFO, dataTraceEnabled: true}}, invoke the method, then read the API-Gateway-Execution-Logs_<api-id>/<stage> log group; repeat with loggingLevel=ERROR and OFF. Observe no events at any level. Re-replay apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs.
