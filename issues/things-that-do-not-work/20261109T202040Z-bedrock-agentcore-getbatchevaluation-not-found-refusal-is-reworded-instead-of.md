Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agentcore-batch-evaluation-not-found-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agentcore-batch-evaluation-not-found-errors
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|GetBatchEvaluation|4xx->4xx|$.message
Issue-Commit: 19c62db1539bfab1a25075fa0f8ae1a77a215876

# GetBatchEvaluation not-found refusal is reworded instead of AWS's fixed ResourceNotFoundException sentence

> _Note: this issue was identified against the code base at commit `19c62db1539bfab1a25075fa0f8ae1a77a215876`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBatchEvaluation

## Description
Cause 1 of 1 — GetBatchEvaluation on an unknown batchEvaluationId refuses with the right status and error code but an emulator-authored message.
AWS: Smithy (com.amazonaws.bedrockagentcore) — GetBatchEvaluation (GET /evaluations/batch-evaluate/{batchEvaluationId}) models ResourceNotFoundException (@error client, httpError 404, member `message`), documented "The exception that occurs when the specified resource does not exist." The recorded response for this error class is 404 ResourceNotFoundException with the message "Failed to retrieve the resource because it doesn't exist. Retry the request with a different resource identifier." That sentence is entirely FIXED text for this error class — it interpolates no part of the request: it names neither the identifier nor the resource type (the model states the error shape; the sentence itself is recorded behavior only).
Emulator: answers 404 ResourceNotFoundException, but the message is an emulator-authored sentence of the form "Batch Evaluation not found: <batchEvaluationId>" — a different fixed prefix plus the requested id interpolated, where AWS interpolates nothing.
Why: a client that matches, logs or surfaces AWS's not-found sentence (or asserts on it in a test, or routes on the documented retry guidance "Retry the request with a different resource identifier") sees text AWS never returns, and conversely a client is handed an identifier echo AWS deliberately does not emit. The status and error code are right, so only the human- and matcher-visible message diverges.
Fix: the GetBatchEvaluation handler's not-found refusal path — emit the service's fixed ResourceNotFoundException sentence rather than formatting the resource type and the requested id into a bespoke message.
Traces: fixture bedrock-agentcore-batch-evaluation-not-found-errors, step task#1.
This is net-new: no open issue carries this fixture step. The nearby open issues 20261102T113600Z-bedrock-agentcore-agentcore-not-found-refusals-diverge-wrong-status-wrong and 20261108T080658Z-bedrock-agentcore-batchcreatememoryrecords-not-found-refusal-reworded-emulator-authored-sentence cover other operations' not-found refusals (InvokeAgentRuntime, StopCodeInterpreterSession, BatchCreateMemoryRecords) and do not reach GetBatchEvaluation.

## Evidence
Fixture bedrock-agentcore-batch-evaluation-not-found-errors, step task#1, GetBatchEvaluation on an id that does not exist.

Status: expected 404, got 404 (matches).
$.message: value_mismatch — expected "Failed to retrieve the resource because it doesn't exist. Retry the request with a different resource identifier.", got 'Batch Evaluation not found: zfakebatchevalxxxxxxx-1234567890'.

Fixed vs variable: the expected message is fixed text end to end — no request or state value appears in it. In the emulator's message, "Batch Evaluation not found: " is the emulator's own fixed prefix and the trailing token is the batchEvaluationId taken from the request path (the id in this recording is just that run's probe value).

## Steps to Reproduce
1. Call GetBatchEvaluation GET /evaluations/batch-evaluate/{batchEvaluationId} with a well-formed batchEvaluationId (matching ^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$) that no batch evaluation uses.
2. Observe 404 / x-amzn-errortype ResourceNotFoundException with message "Batch Evaluation not found: <id>".
3. AWS answers 404 ResourceNotFoundException with the fixed message "Failed to retrieve the resource because it doesn't exist. Retry the request with a different resource identifier."
Replay: fixture bedrock-agentcore-batch-evaluation-not-found-errors, step task#1.
