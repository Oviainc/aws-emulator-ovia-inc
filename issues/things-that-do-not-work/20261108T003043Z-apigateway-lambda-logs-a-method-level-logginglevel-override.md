Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level
Issue-Key: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level|run#34|Timeout|raise AssertionError(
Issue-Commit: 755fa21f4b5f658e5c42cafb46f764c1d29f1673

# apigateway__lambda__logs: a method-level loggingLevel override never produces execution logs, so the method-vs-stage logging precedence can never be observed

> _Note: this issue was identified against the code base at commit `755fa21f4b5f658e5c42cafb46f764c1d29f1673`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Cause 1 of 1: a per-method `loggingLevel` override on a REST stage delivers no execution log events, so the override can never beat the stage-wide level.
AWS: `UpdateStage` accepts per-method settings via patch paths of the form `/<resourcePath>/<httpMethod>/logging/loglevel`, and those settings take precedence over the stage-wide `/*/*/logging/loglevel`. With the stage-wide level `ERROR` and an `INFO` override on `POST`, a successful POST writes execution-log events into `API-Gateway-Execution-Logs_<api-id>/<stage>` — including the fixed line `Method completed with status: 200` — while a successful GET on the same resource writes none. The log-group name, the stage and the api id are values taken from the request; the `Method completed with status: <code>` sentence is fixed text with the status code from the response. Smithy: apigateway `UpdateStage` (PATCH /restapis/{restApiId}/stages/{stageName}) returns `Stage` with `methodSettings`; execution-log content itself is recorded behavior only.
Emulator: no execution-log event ever appears for the overridden method, so the check polls to its deadline and reports `Error: Check timed out after 180.0s`. Nothing distinguishes the INFO-overridden POST from the ERROR-level GET.
Why: a client that raises logging on one noisy method to diagnose it gets no lines at all; method-level `methodSettings` are stored but inert, so troubleshooting and any assertion about per-method logging precedence is impossible.
Fix: the REST stage execution-logging emitter must resolve the effective level per method (method-level `methodSettings` first, stage-wide `/*/*` second) and write the resulting events to `API-Gateway-Execution-Logs_<api-id>/<stage>`.
Traces: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level. Related but NOT covering this check: open issues 20261107T070300Z-apigateway-lambda-logs-rest-stage-execution-logs-a-method-level-logginglevel.md and 20261029T182816Z-apigateway-lambda-logs-proxy-integrations-answer-500-the.md, which the suite attributes to other checks.

## Evidence
Expected: after setting the stage-wide loggingLevel to ERROR and a method-level INFO override on POST, the POST's execution log in `API-Gateway-Execution-Logs_<api-id>/<stage>` contains `Method completed with status: 200` while the GET's does not.
Actual: `Error: Check timed out after 180.0s` — no execution-log event is delivered for either method within the budget, so neither the override nor the suppression can be observed.

## Steps to Reproduce
1. Create a REST API with a Lambda-proxy POST and GET on the same resource, set the account CloudWatch role, and deploy to a stage.
2. `UpdateStage` with `/*/*/logging/loglevel` = `ERROR` and `/<resource>/POST/logging/loglevel` = `INFO`.
3. Issue a successful POST and a successful GET through the stage.
4. Read `API-Gateway-Execution-Logs_<api-id>/<stage>` — the POST must have logged `Method completed with status: 200` and the GET must have logged nothing; the emulator delivers no events at all.
