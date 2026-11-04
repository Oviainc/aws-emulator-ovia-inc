Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-function-invocation-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-function-invocation-paths lambda-invoke-modes-comparison
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Architectures
Issue-Key: lambda|CreateFunction|2xx->4xx|$.CodeSha256
Issue-Key: lambda|CreateFunction|2xx->4xx|$.CodeSize
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Description
Issue-Key: lambda|CreateFunction|2xx->4xx|$.EphemeralStorage
Issue-Key: lambda|CreateFunction|2xx->4xx|$.FunctionArn
Issue-Key: lambda|CreateFunction|2xx->4xx|$.FunctionName
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Handler
Issue-Key: lambda|CreateFunction|2xx->4xx|$.LastModified
Issue-Key: lambda|CreateFunction|2xx->4xx|$.LoggingConfig
Issue-Key: lambda|CreateFunction|2xx->4xx|$.MemorySize
Issue-Key: lambda|CreateFunction|2xx->4xx|$.PackageType
Issue-Key: lambda|CreateFunction|2xx->4xx|$.RevisionId
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Role
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Runtime
Issue-Key: lambda|CreateFunction|2xx->4xx|$.RuntimeVersionConfig
Issue-Key: lambda|CreateFunction|2xx->4xx|$.SnapStart
Issue-Key: lambda|CreateFunction|2xx->4xx|$.State
Issue-Key: lambda|CreateFunction|2xx->4xx|$.StateReason
Issue-Key: lambda|CreateFunction|2xx->4xx|$.StateReasonCode
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Timeout
Issue-Key: lambda|CreateFunction|2xx->4xx|$.TracingConfig
Issue-Key: lambda|CreateFunction|2xx->4xx|$.Version
Issue-Key: lambda|CreateFunction|2xx->4xx|status
Issue-Key: lambda|Invoke|2xx->4xx|$.statusCode
Issue-Key: lambda|Invoke|2xx->4xx|status
Issue-Key: lambda|Invoke|2xx->4xx|$
Issue-Key: lambda|InvokeAsync|2xx->4xx|$
Issue-Key: lambda|InvokeAsync|2xx->4xx|status
Issue-Key: lambda|Invoke|4xx->4xx|$.message
Issue-Key: lambda|Invoke|4xx->4xx|status
Issue-Key: lambda|DeleteFunction|2xx->4xx|status
Issue-Key: lambda|Invoke|2xx->4xx|$.body
Issue-Key: lambda|InvokeAsync|4xx->4xx|$.Type
Issue-Key: lambda|InvokeAsync|4xx->4xx|$.message
Issue-Key: lambda|InvokeAsync|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|InvokeAsync|4xx->4xx|status
Issue-Related: 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda CreateFunction refuses a request AWS accepts, breaking every dependent invoke step

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Shared fault: CreateFunction rejects a valid request with 400, so the function never exists and every step of the fixture that depends on it fails with a not-found. Eight net-new clusters (the first is the root; the rest are the operations it strands, each with its own fixture step a fixer must re-replay).

AWS: Smithy CreateFunction (POST /2015-03-31/functions, 201) requires FunctionName, Role and Code; `Role` is a RoleArn matching `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`. The recorded request uses a path-less execution role ARN of that form and AWS answers 201 with the full FunctionConfiguration (State `Pending`, StateReasonCode `Creating`, LoggingConfig, EphemeralStorage, Architectures, RuntimeVersionConfig, etc.). Hypothesis: the emulator's CreateFunction validation requires the execution role to be present in local IAM state, or rejects a role ARN without a path segment.

Emulator: CreateFunction answers 400 and no function is created, so the subsequent Invoke/InvokeAsync/DeleteFunction steps answer 404 ResourceNotFoundException `Function not found: <arn>` instead of their recorded outcomes.

Why: the whole function-invocation surface is unreachable for this (valid) shape of request — a client that creates a function with a plain execution-role ARN cannot create one at all, and the failure is reported as a caller error.

Fix: CreateFunction's request validation — it must accept a well-formed RoleArn per the modeled pattern without requiring the role to be resolvable locally — and then the handler must create the function and serialize the full FunctionConfiguration.

Cause 1 of 8: CreateFunction — fixture lambda-function-invocation-paths step setup#1; expected 201 with the FunctionConfiguration body, emulator answered 400 and created nothing.
Cause 2 of 8: Invoke — fixture lambda-function-invocation-paths step task#2; expected 200 with `statusCode: 200` from the handler, emulator answered 404 (function absent).
Cause 3 of 8: Invoke (Event) — fixture lambda-function-invocation-paths step task#3; expected 202 with an empty body, emulator answered 404 ResourceNotFoundException.
Cause 4 of 8: InvokeAsync — fixture lambda-function-invocation-paths step task#4; expected 202 empty body, emulator answered 404.
Cause 5 of 8: Invoke — fixture lambda-function-invocation-paths step task#5; expected 400 InvalidRequestContentException for an unparsable payload, emulator answered 404 ResourceNotFoundException — so the payload is never even parsed.
Cause 6 of 8: DeleteFunction — fixture lambda-function-invocation-paths step teardown#6; expected 204, emulator answered 404.
Cause 7 of 8: Invoke — fixture lambda-invoke-modes-comparison step task#2; expected 200 with `statusCode: 200` and `body: 'hello'`, emulator answered 404 (same stranded create in that fixture's setup).
Cause 8 of 8: InvokeAsync — fixture lambda-invoke-modes-comparison step task#5; expected 400 InvalidRequestContentException, emulator answered 404 ResourceNotFoundException.

## Evidence
Cluster details: setup#1 `status: expected 201, got 400` with the entire FunctionConfiguration body missing; task#2 `status: expected 200, got 404`; task#3/task#4 `status: expected 202, got 404` with body `{'__type': 'ResourceNotFoundException', 'Message': 'Function not found: <arn>'}` where `{}` was recorded; task#5 `status: expected 400, got 404` with `x-amzn-errortype: value_mismatch (expected 'InvalidRequestContentException', got 'ResourceNotFoundException')`; teardown#6 `status: expected 204, got 404`. Smithy: CreateFunction http code 201 and the RoleArn pattern quoted above.

## Steps to Reproduce
Replay the corpus. Fixture lambda-function-invocation-paths step setup#1: CreateFunction with Role `arn:aws:iam::<account>:role/<name>` (no path) and a zip Code — the emulator answers 400 where 201 is recorded; every later step of that fixture then answers 404.
