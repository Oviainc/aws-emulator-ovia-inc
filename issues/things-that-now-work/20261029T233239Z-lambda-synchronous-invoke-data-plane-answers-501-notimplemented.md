Issue-Source: trace-sweep-divergence
Issue-Trace: lam-invoke-alias-executed-version
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-invoke-alias-executed-version lam-invoke-dry-run lam-invoke-log-type-tail lam-invoke-unhandled-function-error lam-invoke-event-payload-too-large lam-invoke-kms-disabled lambda-create-function-invoke-response-stream-lifecycle lambda-invoke-response-stream-missing-version lambda-publish-version-then-invoke-pinned-version lambda-zip-function-invoke-publish-lifecycle lambda-invoke-modes-comparison lambda-invoke-response-stream-deleted-version-error lambda-invoke-response-stream-function-not-found lambda-invoke-stream-nonexistent-function lambda-invoke-with-response-stream-nonexistent-function-error lambda-invoke-with-response-stream-not-found
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|Invoke|2xx->5xx|$.ok
Issue-Key: lambda|Invoke|2xx->5xx|$headers.x-amz-executed-version
Issue-Key: lambda|Invoke|2xx->5xx|status
Issue-Key: lambda|Invoke|4xx->4xx|$.Type
Issue-Key: lambda|Invoke|4xx->4xx|$.Message
Issue-Key: lambda|Invoke|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|Invoke|2xx->5xx|$.env
Issue-Key: lambda|Invoke|5xx->5xx|$.Message
Issue-Key: lambda|Invoke|5xx->5xx|$.Type
Issue-Key: lambda|Invoke|5xx->5xx|$headers.x-amzn-errortype
Issue-Key: lambda|Invoke|5xx->5xx|status
Issue-Key: lambda|Invoke|2xx->5xx|$headers.x-amz-log-result
Issue-Key: lambda|Invoke|2xx->5xx|$.errorMessage
Issue-Key: lambda|Invoke|2xx->5xx|$.errorType
Issue-Key: lambda|Invoke|2xx->5xx|$.requestId
Issue-Key: lambda|Invoke|2xx->5xx|$.stackTrace
Issue-Key: lambda|Invoke|2xx->5xx|$headers.x-amz-function-error
Issue-Key: lambda|InvokeWithResponseStream|2xx->5xx|$.__wire_eventstream__
Issue-Key: lambda|InvokeWithResponseStream|2xx->5xx|$headers.content-type
Issue-Key: lambda|InvokeWithResponseStream|2xx->5xx|$headers.x-amz-executed-version
Issue-Key: lambda|InvokeWithResponseStream|2xx->5xx|status
Issue-Key: lambda|InvokeWithResponseStream|4xx->4xx|$.Type
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda synchronous invoke data plane answers 501 NotImplemented instead of running the function

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Shared fault: the Invoke / InvokeWithResponseStream data plane is not served — the emulator answers 501 NotImplemented where AWS runs the function — and the invoke error paths that do answer carry the wrong body. Ten net-new clusters.

AWS: Smithy Invoke (POST /2015-03-31/functions/{FunctionName}/invocations, 200) returns the handler's payload as the HTTP payload plus the headers `X-Amz-Executed-Version` (the version actually run), `X-Amz-Log-Result` (base64 tail when LogType=Tail) and `X-Amz-Function-Error` (`Unhandled` when the handler raised); an unhandled handler error is still HTTP 200 with the error payload (errorMessage/errorType/requestId/stackTrace). InvokeWithResponseStream returns `content-type: application/vnd.amazon.eventstream` carrying PayloadChunk then InvokeComplete events. Invoke binds RequestTooLargeException (413, {Type, message}) — an Event-type payload over the quota is refused with `<n> byte payload is too large for the Event invocation type (limit 1048576 bytes)` (fixed text; the actual size and the limit are the variables) — and KMSDisabledException (502) — a function whose environment-variable key is disabled fails 502 with the `Lambda was unable to decrypt the environment variables because the KMS key used is disabled...` message. ResourceNotFoundException (404) carries `Type` and, for a qualified lookup, a Message naming the function ARN with its qualifier (`...:$LATEST`).

Emulator: Invoke and InvokeWithResponseStream answer 501 with x-amzn-errortype `NotImplemented`; the 413 refusal omits its Message; the 404 Message drops the `:$LATEST` qualifier and `Type`.

Why: nothing that invokes a function works — no payload, no ExecutedVersion, no log tail, no function-error signal and no event-stream framing — so every client and every downstream integration that calls a function observes a service that cannot run code, and the KMS-disabled failure is reported as NotImplemented rather than the modeled 502.

Fix: implement the Invoke and InvokeWithResponseStream handlers (payload execution, the three response headers, the eventstream framing) and their pre-invoke validation (payload-size 413, KMS state 502) rather than returning NotImplemented; the invoke not-found path must render the qualified ARN and `Type`.

Cause 1 of 10: Invoke — fixture lam-invoke-alias-executed-version step task#7; expected 200 with the handler payload and `x-amz-executed-version`, emulator answered 501.
Cause 2 of 10: Invoke, 404 — fixture lam-invoke-alias-executed-version step task#11; `Type` missing.
Cause 3 of 10: Invoke, 413 — fixture lam-invoke-event-payload-too-large step task#7; the payload-too-large Message is absent from the body.
Cause 4 of 10: Invoke — fixture lam-invoke-kms-disabled step task#7; expected 200 echoing the decrypted environment variable, emulator answered 501.
Cause 5 of 10: Invoke — fixture lam-invoke-kms-disabled step task#9; expected 502 KMSDisabledException with the KMS-disabled Message, emulator answered 501 NotImplemented.
Cause 6 of 10: Invoke — fixture lam-invoke-log-type-tail step task#6; expected 200 with `x-amz-log-result` and `x-amz-executed-version`, emulator answered 501.
Cause 7 of 10: Invoke — fixture lam-invoke-unhandled-function-error step task#6; expected 200 with `x-amz-function-error: Unhandled` and the errorMessage/errorType/requestId/stackTrace payload, emulator answered 501.
Cause 8 of 10: InvokeWithResponseStream — fixture lambda-create-function-invoke-response-stream-lifecycle step task#4; expected 200 `application/vnd.amazon.eventstream` with PayloadChunk then InvokeComplete, emulator answered 501 `application/json`.
Cause 9 of 10: Invoke, 404 — fixture lambda-invoke-modes-comparison step task#4; Message must name the function ARN with its `:$LATEST` qualifier; emulator dropped the qualifier and `Type`.
Cause 10 of 10: InvokeWithResponseStream, 404 — fixture lambda-invoke-modes-comparison step task#6; `Type` missing.

## Evidence
Cluster details: five Invoke clusters `status: expected 200, got 501` with `$.ok`/`$.env`/`$.errorMessage` and `x-amz-executed-version`/`x-amz-log-result`/`x-amz-function-error` missing; kms task#9 `status: expected 502, got 501` with `x-amzn-errortype ... got 'NotImplemented'`; response-stream cluster `status: expected 200, got 501` with `$.__wire_eventstream__: missing` and `content-type: value_mismatch (expected 'application/vnd.amazon.eventstream', got 'application/json')`; payload-too-large cluster `$.Message: missing (expected '1048577 byte payload is too large for the Event invocation type (limit 1048576 bytes)')`; modes task#4 `$.Message: value_mismatch` over the missing `:$LATEST` qualifier. Smithy: Invoke's output headers X-Amz-Executed-Version/X-Amz-Log-Result/X-Amz-Function-Error and its RequestTooLargeException (413) and KMSDisabledException (502) bindings.

## Steps to Reproduce
Replay the corpus. Fixture lam-invoke-alias-executed-version step task#7: Invoke the alias and read the response — 501 NotImplemented instead of 200 with the payload. Fixture lambda-create-function-invoke-response-stream-lifecycle step task#4: InvokeWithResponseStream — 501 application/json instead of an eventstream.
