Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle
Issue-Key: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle|_log_messages#9|Timeout|time.sleep(2)
Issue-Commit: fce90a673e7875f918905a481813c28a01bba620

# apigatewayv2__dynamodb__lambda__logs__s3: the WebSocket backend-response lifecycle never completes — the scenario hangs to the 60 s cap with no refusal

> _Note: this issue was identified against the code base at commit `fce90a673e7875f918905a481813c28a01bba620`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
Cause 1 of 1: a WEBSOCKET API's end-to-end backend-response lifecycle never reaches a terminal state.
  AWS: recorded behavior only — the recording drives a full WebSocket lifecycle (upgrade, $connect, a client frame, the backend's response pushed back through @connections, state written to DynamoDB/S3 and access/function logs in CloudWatch Logs) and every leg either completes or is refused with a status and error code within seconds.
  Emulator: the check reaches no verdict at all — 'Check timed out after 60.0s'. Nothing is answered and nothing is refused, so the whole lifecycle is ungradeable.
  Why: a client gets no response and no error: the call neither succeeds nor fails, so retry and error handling have nothing to act on, and the behaviour cannot be distinguished from a hung network.
  Fix: the apigatewayv2 WEBSOCKET execute-api data plane that this scenario drives — the post-upgrade frame loop and the @connections push must answer or refuse promptly instead of leaving the caller waiting. Related open issue 20261110T153046Z-apigatewayv2-dynamodb-lambda-logs-s3-the-websocket-backend.md describes a similar hang and does not carry this check's replay key.
  Traces: apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.

## Evidence
expected: the WebSocket backend-response lifecycle completes (or is refused with a modeled error) well inside the check budget.
actual: Error: Check timed out after 60.0s — no response, no refusal, no graded assertion.

## Steps to Reproduce
1. Deploy a WEBSOCKET API with Lambda integrations persisting to DynamoDB/S3 and logging to CloudWatch Logs, plus a stage and a deployment.
2. Upgrade a client socket and run the full lifecycle (connect, frame, backend push back via @connections, read the pushed frame).
3. Observe the scenario never terminates within 60 s.
4. Replay apigatewayv2__dynamodb__lambda__logs__s3.e2e-debug-websocket-api-backend-response-lifecycle.
