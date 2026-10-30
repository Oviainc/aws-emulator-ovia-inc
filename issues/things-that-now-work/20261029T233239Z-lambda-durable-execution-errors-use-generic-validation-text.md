Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-durable-execution-list-and-callback-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-durable-execution-list-and-callback-errors
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|ListDurableExecutionsByFunction|4xx->4xx|$.Message
Issue-Key: lambda|ListDurableExecutionsByFunction|4xx->4xx|$.Type
Issue-Key: lambda|SendDurableExecutionCallbackSuccess|4xx->4xx|$.message
Issue-Key: lambda|SendDurableExecutionCallbackSuccess|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|SendDurableExecutionCallbackFailure|4xx->4xx|$.message
Issue-Key: lambda|SendDurableExecutionCallbackFailure|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|SendDurableExecutionCallbackHeartbeat|4xx->4xx|$.message
Issue-Key: lambda|SendDurableExecutionCallbackHeartbeat|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda durable execution errors use generic validation text instead of AWS's refusals

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDurableExecutionsByFunction

## Description
Shared fault: the durable-execution operations refuse bad input with the emulator's generic constraint-violation machinery instead of AWS's fixed refusal texts and error classes. Four net-new clusters.

AWS: ListDurableExecutionsByFunction for a function that does not exist is refused 404 ResourceNotFoundException (Smithy: httpError 404, members {Type, Message}) with Message `Function not found` — wholly fixed text, the function identifier is NOT appended. The three callback operations (SendDurableExecutionCallbackSuccess / ...Failure / ...Heartbeat) refuse an unusable callback id 400 with message `Invalid callback id` — wholly fixed text, the supplied id is not echoed and no constraint pattern is disclosed.

Emulator: the not-found Message is `Function not found: <functionArn>` and omits `Type`; all three callback operations answer 400 with `1 validation error detected: Value '<callbackId>' at 'callbackId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/]+={0,2}` under x-amzn-errortype `ValidationException`, a different error class and message from the recorded refusal.

Why: a client matching AWS's refusal text or error code to decide whether to re-issue a callback or treat the token as expired matches neither, and the emulator leaks the internal id pattern in a message AWS keeps opaque.

Fix: the durable-execution handlers' validation — raise the recorded 400 with the fixed `Invalid callback id` message for an unusable callback id rather than letting the shared constraint validator answer, and build the not-found message without the identifier while emitting `Type`.

Cause 1 of 4: ListDurableExecutionsByFunction, 404 — fixture lambda-durable-execution-list-and-callback-errors step task#1; Message must be `Function not found`, emulator appended `: <arn>`; `Type` missing.
Cause 2 of 4: SendDurableExecutionCallbackSuccess, 400 — fixture lambda-durable-execution-list-and-callback-errors step task#2; message must be `Invalid callback id`, emulator emitted the ValidationException constraint text.
Cause 3 of 4: SendDurableExecutionCallbackFailure, 400 — fixture lambda-durable-execution-list-and-callback-errors step task#3; same defect.
Cause 4 of 4: SendDurableExecutionCallbackHeartbeat, 400 — fixture lambda-durable-execution-list-and-callback-errors step task#4; same defect.

## Evidence
Cluster details: task#1 `$.Type: missing (expected 'User', got None)` and `$.Message: value_mismatch (expected 'Function not found', got 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-probe-nonexistent-fn')`; task#2/3/4 `$.message: value_mismatch (expected 'Invalid callback id', got "1 validation error detected: Value '...' at 'callbackId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/]+={0,2}")` with `$headers.x-amzn-errortype: shape_mismatch (... got 'ValidationException')`. Expected status == actual status (404, 400, 400, 400).

## Steps to Reproduce
Replay the corpus. Fixture lambda-durable-execution-list-and-callback-errors step task#2: SendDurableExecutionCallbackSuccess with a callback id that is not a valid token — expect 400 `Invalid callback id`, observe the ValidationException constraint message.
