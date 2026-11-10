Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin
Issue-Key: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin|_attempt_handshake#30|Timeout|time.sleep(2)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2__iam__lambda__logs__sts: UpdateApi(DisableExecuteApiEndpoint=true) cannot be graded — the HTTP stages and the WebSocket twin never answer within the run budget (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApi

## Description
Cause 1 of 1: after UpdateApi(DisableExecuteApiEndpoint=true) neither the HTTP API's $default/manual stages nor the WebSocket twin produce any response, so the documented asymmetry is unobservable.
AWS: with the default execute-api endpoint disabled, a request to an HTTP API stage is refused 404 with the body {"message":"Not Found"} (both the AutoDeploy $default stage and a never-redeployed manual stage), while a WebSocket API's never-redeployed stage keeps accepting handshakes until that stage is itself redeployed, after which the handshake is refused 403 Forbidden with a body naming connectionId, message and requestId. Smithy: UpdateApi input member DisableExecuteApiEndpoint (boolean); errors NotFoundException 404, TooManyRequestsException 429, BadRequestException 400, ConflictException 409.
Emulator: the check reaches no verdict at all — 'Error: Check timed out after 60.0s'. Neither the 404 refusal, the still-serving WebSocket stage, nor the post-redeploy 403 is ever produced.
Why: a client cannot tell whether disabling the default endpoint took effect, and a deployment pipeline that relies on the 'disabled only after redeploy' semantics gets no signal at all.
Fix: the apigatewayv2 execute-api front door's DisableExecuteApiEndpoint enforcement, per stage and per protocol, and the stage-deployment snapshot it reads. The open issue 20261109T143515Z-apigatewayv2-iam-lambda-logs-sts-a-signed-websocket-handshake-to-a-manually-created.md covers a 500-answered handshake for this family but does not carry this check's current replay key (it now hangs), so it does not cover it.
Traces: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: HTTP 404 with body {"message":"Not Found"} on both HTTP stages after the update; the WebSocket stage still completing a handshake until redeployed, then 403 Forbidden whose body carries connectionId, message and requestId. Actual: no HTTP or WebSocket response is produced within the cap.

## Steps to Reproduce
Replay apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin: create an HTTP API with a $default AutoDeploy stage and a manual stage pinned to a deployment, plus a WebSocket API with its own stage; call UpdateApi(DisableExecuteApiEndpoint=True) on each and request both planes before and after a redeploy.
