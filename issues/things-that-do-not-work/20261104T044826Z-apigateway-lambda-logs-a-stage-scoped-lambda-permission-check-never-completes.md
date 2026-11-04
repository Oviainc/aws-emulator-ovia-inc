Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged
Issue-Key: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged|run#36|Timeout|assert rid, f"response from stage {st!r} carried no x-amzn-RequestId header"
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# A stage-scoped Lambda permission check never completes: the other stage is neither denied nor logged

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Cause 1 of 1: a Lambda resource policy whose SourceArn is scoped to one stage must admit that stage and refuse the other, and the refusal must appear in the stage's execution log.
AWS: recorded behavior only — the recording shows lambda:AddPermission with a SourceArn of the form arn:aws:execute-api:<region>:<account>:<api-id>/<stage>/<method>/<path>; a request on that stage reaches the function, a request on a second stage is refused at the edge with HTTP 500 and the execution log records the AccessDeniedException the integration got, and the whole scenario completes well inside 60s.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — neither the admitted leg nor the denied leg settles and no execution log line is delivered, so the poll never terminates.
Why: a client relying on per-stage SourceArn scoping gets no observable difference between a permitted and an unpermitted stage, and no log evidence of the decision; a wait-for-log loop hangs rather than failing.
Fix: the execute-api integration authorization path that evaluates the Lambda resource policy's SourceArn against the invoking stage, and the stage execution-log writer that records the refusal.
Traces: apigateway__lambda__logs.apigw-stage-scoped-lambda-permission-other-stage-denied-and-logged.

## Evidence
Expected (AWS): the granted stage invokes the function; the other stage is refused and the refusal is written to the stage execution log.
Actual (emulator): 'Error: Check timed out after 60.0s' — the scenario never settles and no log line is delivered.

## Steps to Reproduce
1. Create a REST API with two stages and a Lambda proxy integration.
2. lambda:AddPermission with Principal apigateway.amazonaws.com and SourceArn scoped to stage A only.
3. Enable execution logging on both stages, request the path on stage A and on stage B, and poll the execution log groups. Observe the scenario never settles within 60s.
