Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs
Issue-Key: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs|_wait_for_status#17|Timeout|time.sleep(3)
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# API Gateway method-setting loggingLevel/dataTrace does not gate execution logs (check never completes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
One net-new root cause in the apigateway__dynamodb__logs group.

Cause 1 of 1: the per-method `loggingLevel` and `dataTrace` method settings do not gate execution logging.
AWS: a stage methodSetting with `loggingLevel=INFO`/`ERROR` and `dataTrace=true|false` controls whether execution log events (and request/response bodies) are written to the stage's execution log group; turning it off stops the events.
Emulator: the check gets no answer at all — `Check timed out after 60.0s` — so neither the enabled nor the disabled state is observable.
Why: an operator cannot verify that turning data tracing off actually stops request/response bodies from being logged, which is the control this setting exists for.
Fix: the apigateway stage method-settings handling and the execution-log emitter it gates.
Traces: apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs

## Evidence
[mismatch] apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs — Error: Check timed out after 60.0s.

## Steps to Reproduce
Boot the emulator and replay apigateway__dynamodb__logs.deep-apigateway-1-apigw-methodsetting-logging-level-and-data-trace-gate-execution-logs: it sets stage method settings and then polls the execution log group, which never receives events, so the check consumes the whole 60s budget.
