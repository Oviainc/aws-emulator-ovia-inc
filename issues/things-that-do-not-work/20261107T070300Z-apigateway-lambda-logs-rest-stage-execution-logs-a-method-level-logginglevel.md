Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged
Issue-Key: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level|_http#34|Timeout|with urllib.request.urlopen(req, timeout=20) as resp:
Issue-Key: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged|_http#36|Timeout|with urllib.request.urlopen(urllib.request.Request(url), timeout=20) as resp:
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# REST stage execution logs: a method-level loggingLevel override emits nothing, and a stage-scoped Lambda permission neither refuses nor logs the other stage (2 causes)

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Two net-new root causes, both on the execute-api REST data plane's CloudWatch execution logging. Both checks are killed by the harness at its 60 s cap because the behaviour they wait for never occurs — the suite reports only `Check timed out after 60.0s` for each.

Cause 1 of 2 — a per-method `loggingLevel` override is accepted by the control plane but produces no execution-log events, so it neither overrides nor is overridden by the stage-wide level.
AWS: Smithy `com.amazonaws.apigateway#MethodSetting.loggingLevel` — "Specifies the logging level for this method... Valid values are OFF, ERROR, and INFO. Choose ERROR to write only error-level entries to CloudWatch Logs, or choose INFO to include all ERROR events as well as extra informational events." With `UpdateStage` patches setting `/*/*/logging/loglevel` = ERROR and `/~1echo/POST/logging/loglevel` = INFO, a successful POST to `/echo` is traced at INFO into the stage's execution log group `API-Gateway-Execution-Logs_<api-id>/<stage>`: each line is prefixed with `(<request-id>)` where `<request-id>` is the value returned in the response's `x-amzn-RequestId` header, and the trace includes a line containing `HTTP Method: POST, Resource Path: /echo` and one containing `Method completed with status: 200`. A successful GET on the same resource falls under the stage-wide ERROR level and must log nothing. The fixed text of those lines is `HTTP Method: <verb>, Resource Path: <path>` and `Method completed with status: <code>`; the verb, path, status and the parenthesised request id are values taken from the request and the response. Recorded behavior only for the exact line set (the execute-api data plane has no modeled operation).
Emulator: no event is ever written to `API-Gateway-Execution-Logs_<api-id>/<stage>`, so the check's poll for a POST trace sandwiched between logged requests never terminates and the harness kills it at 60 s. Neither half of the contract is observable: the INFO override emits nothing, and the ERROR suppression is therefore unproven.
Why: raising one method to INFO is the documented way to troubleshoot a single API operation; a client that does it gets an empty log group and cannot tell a working override from a broken one, and cannot rely on the stage-wide ERROR level to keep successful traffic out of its logs.
Fix: the API Gateway execute-api invoke path's execution-log emitter and its per-method `methodSettings` resolution (the apigateway service crate's stage-invoke/logging component) must create the `API-Gateway-Execution-Logs_<apiId>/<stage>` group and write the request trace gated by the effective `loggingLevel` for the MATCHED method, not only by the stage-wide `*/*` entry. Related open issue 20261107T035448Z-apigateway-lambda-logs-a-rest-stage-s-access.md names access logs and method-level overrides but does not carry this check's replay key.
Traces: apigateway__lambda__logs.apigw-method-level-log-level-overrides-the-stage-wide-level.

Cause 2 of 2 — a Lambda resource policy scoped to one stage is not enforced for the other stage, and the refusal is not logged either.
AWS: when `AddPermission` grants `lambda:InvokeFunction` to `apigateway.amazonaws.com` with `SourceArn` `arn:aws:execute-api:<region>:<account>:<api-id>/allowed/*`, a request through stage `allowed` reaches the function and is answered with the integration's response, while a request through stage `denied` is refused — the gateway answers 500 `{"message":"Internal server error"}` and the stage's execution log records the line `Execution failed due to configuration error: Invalid permissions on Lambda function` (fixed text; the surrounding record carries the request id and the function ARN from the request and state). Recorded behavior only for that sentence.
Emulator: the check reaches no verdict and is killed at the 60 s cap — the `denied` stage is neither refused nor logged, so neither the stage-scoped grant nor its boundary can be observed.
Why: an over-permissive emulator lets a caller invoke a function through a stage the resource policy excludes; a client testing a per-stage permission boundary sees it pass when it does not hold, and gets no log record of the configuration error.
Fix: the AWS_PROXY integration's Lambda resource-policy evaluation must match the invoking stage against the grant's `SourceArn` (`<api-id>/<stage>/<method>/<path>` wildcards) and, on a mismatch, answer the gateway 500 and write the `Execution failed due to configuration error: Invalid permissions on Lambda function` execution-log record. Related open issue 20261104T044826Z-apigateway-lambda-logs-a-stage-scoped-lambda-permission-check-never-completes.md describes a similar symptom but does not carry this check's replay key.
Traces: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged.

## Evidence
apigw-method-level-log-level-overrides-the-stage-wide-level — Error: Check timed out after 60.0s.
Expected: after `UpdateStage` patches `/*/*/logging/loglevel`=ERROR and `/~1echo/POST/logging/loglevel`=INFO, a 200 POST /echo produces lines in `API-Gateway-Execution-Logs_<api-id>/s` prefixed `(<x-amzn-RequestId>)` containing `HTTP Method: POST, Resource Path: /echo` and `Method completed with status: 200`, while a 200 GET on the same resource produces none.
Actual: no events are delivered to that log group at all, so the check's presence poll never terminates.

apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged — Error: Check timed out after 60.0s.
Expected: with the Lambda permission's SourceArn scoped to `<api-id>/allowed/*`, the `allowed` stage answers the integration's 200 and the `denied` stage answers 500 `{"message":"Internal server error"}` with the execution-log line `Execution failed due to configuration error: Invalid permissions on Lambda function`.
Actual: no verdict within the harness budget — the denied stage is neither refused nor logged.

## Steps to Reproduce
Cause 1:
1. CreateRestApi (REGIONAL), add a resource `/echo` with GET and POST methods, both AWS_PROXY at a Lambda function, AddPermission for apigateway.amazonaws.com with SourceArn `<api-id>/*/*`.
2. CreateDeployment + CreateStage `s`, then UpdateStage with patchOperations replacing `/*/*/logging/loglevel` = ERROR and `/~1echo/POST/logging/loglevel` = INFO (GetStage must report methodSettings keys `*/*` and `~1echo/POST` only).
3. Send a 200 POST and a 200 GET to the stage, note each response's `x-amzn-RequestId`.
4. FilterLogEvents on `API-Gateway-Execution-Logs_<api-id>/s`: AWS shows the POST's `(request-id) ... HTTP Method: POST, Resource Path: /echo` and `Method completed with status: 200` lines and nothing for the GET; the emulator shows no events.

Cause 2:
1. Create a REST API with an AWS_PROXY Lambda integration and two stages, `allowed` and `denied`.
2. AddPermission granting lambda:InvokeFunction to apigateway.amazonaws.com with SourceArn `arn:aws:execute-api:<region>:<account>:<api-id>/allowed/*` only.
3. Invoke the method through `allowed` (expect the integration's 200) and through `denied` (expect 500 `{"message":"Internal server error"}`).
4. FilterLogEvents on the `denied` stage's execution log group for `Execution failed due to configuration error: Invalid permissions on Lambda function` — absent in the emulator.
