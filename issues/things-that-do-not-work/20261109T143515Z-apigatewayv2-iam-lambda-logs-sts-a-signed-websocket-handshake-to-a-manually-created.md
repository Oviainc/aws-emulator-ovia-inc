Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin
Issue-Key: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin|run#31|AssertionError|assert pre_status == 101, f"precondition: the WebSocket stage must accept a signed handshake, got {pre_status!r}"
Issue-Commit: 31f7b518324c5238b816f5187d2718623b5667f6

# A signed WebSocket handshake to a manually created stage answers 500, blocking the disable-execute-api-endpoint twin

> _Note: this issue was identified against the code base at commit `31f7b518324c5238b816f5187d2718623b5667f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
Cause 1 of 1: the WebSocket stage refuses a correctly signed handshake with HTTP 500.
AWS: a deployed WEBSOCKET stage accepts a SigV4-signed upgrade with 'HTTP/1.1 101 Switching Protocols'; DisableExecuteApiEndpoint only governs whether the default execute-api host serves the API, not whether a signed handshake is accepted (Smithy apigatewayv2 Api.DisableExecuteApiEndpoint, CreateStage).
Emulator: the precondition of the check — that the stage accepts a signed handshake — fails with status 500, so the disable/enable twin can never be compared.
Why: a client cannot establish any WebSocket session against a manually created stage, and the DisableExecuteApiEndpoint behaviour it gates is unobservable.
Fix: the execute-api WebSocket upgrade handler in the apigatewayv2 crate (signed-handshake path for a manually created stage).
Note: related to the open 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md, which does not carry this check's replay key.
Traces: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.

## Evidence
Expected: the WebSocket stage accepts a signed handshake (101). Actual: 'AssertionError: precondition: the WebSocket stage must accept a signed handshake, got 500'.

## Steps to Reproduce
Replay apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin: it creates a WEBSOCKET API, a stage, signs an upgrade with STS credentials and opens it; the handshake answers 500.
