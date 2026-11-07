Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.code-signing-enforce-rejects-unsigned-package-rejected lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code lambda.event-invoke-failure-not-surfaced lambda.invoke-refuses-a-function-arn-from-another-region lambda.custom-runtime-streaming-response-mode-delivers-chunks-via-invoke-with-response-stream lambda.invoke-with-response-stream-managed-streaming lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes lambda.external-extension-registers-receives-invoke-and-shutdown-events lambda.extension-that-never-registers-fails-the-invoke lambda.external-extension-that-exits-before-registering-fails-the-invoke lambda.extension-register-after-init-is-refused lambda.external-extension-layer-runs-identically-across-runtime-families lambda.exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments lambda.exec-wrapper-naming-a-missing-file-rejected-at-invoke
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.code-signing-enforce-rejects-unsigned-package-rejected|_client_error#22|AssertionError|raise AssertionError(
Issue-Key: lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded and returned {result!r}")
Issue-Key: lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code|run#9|AssertionError|raise AssertionError(
Issue-Key: lambda.event-invoke-failure-not-surfaced|run#22|AssertionError|assert nf_msg == expected_nf_msg, (
Issue-Key: lambda.invoke-refuses-a-function-arn-from-another-region|run#15|AssertionError|assert error.get("Message") == expected, (
Issue-Key: lambda|InvokeWithResponseStream|InvalidRuntimeException
Issue-Key: lambda.invoke-with-response-stream-managed-streaming|run#13|AssertionError|assert spread >= 1.0, f"chunks arrived within {spread:.3f}s of each other - looks buffered, not streamed: {chunk_times}"
Issue-Key: lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes|run#10|AssertionError|assert rest == b"", f"stream ended mid-line: trailing bytes {rest[:200]!r}"
Issue-Key: lambda.external-extension-registers-receives-invoke-and-shutdown-events|run#20|AssertionError|assert last["ext_log_exists"], (
Issue-Key: lambda.extension-that-never-registers-fails-the-invoke|run#8|AssertionError|assert bad["status"] == 200 and bad["error"] == "Unhandled", (
Issue-Key: lambda.external-extension-that-exits-before-registering-fails-the-invoke|run#8|AssertionError|assert bad["status"] == 200 and bad["error"] == "Unhandled", (
Issue-Key: lambda.extension-register-after-init-is-refused|run#12|AssertionError|assert late, f"no late-register attempt recorded for this invocation: {records!r}"
Issue-Key: lambda.external-extension-layer-runs-identically-across-runtime-families|run#7|AssertionError|assert not cold["function_error"], (
Issue-Key: lambda.exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments|run#8|AssertionError|assert body["wrapped"] == "set-by-wrapper", (
Issue-Key: lambda.exec-wrapper-naming-a-missing-file-rejected-at-invoke|run#5|AssertionError|assert control["error"] is None and json.loads(control["raw"]) == {"wrapped": "set-by-wrapper"}, (
Issue-Commit: 677b6e35c5226b67b9bfea5e554d250e3c084a9c

# lambda: code signing not enforced, no in-flight update conflict, wrong ResourceNotFoundException messages on Invoke, InvokeWithResponseStream not streaming, extensions and AWS_LAMBDA_EXEC_WRAPPER never run (8 net-new causes)

> _Note: this issue was identified against the code base at commit `677b6e35c5226b67b9bfea5e554d250e3c084a9c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Eight net-new root causes across the Lambda control plane's refusals and the data plane's invoke contract.

Cause 1 of 8: an unsigned package is accepted under a code-signing policy of ENFORCE.
AWS: Smithy models UpdateFunctionCode/CreateFunction errors CodeVerificationFailedException (client, HTTP 400 — "The code signature failed one or more of the validation checks for signature mismatch or expiry, and the code signing policy is set to ENFORCE. Lambda blocks the deployment.") and InvalidCodeSignatureException (client, 400), plus CodeSigningConfigNotFoundException (404). A function with an ENFORCE code-signing config must refuse an unsigned deployment package.
Emulator: the call SUCCEEDS — 200 with the updated FunctionConfiguration, no error.
Why: a client relying on code signing as a deployment control gets a false pass; unsigned code deploys where AWS would block it.
Fix: code-signing validation in the CreateFunction/UpdateFunctionCode handler.
Traces: lambda.code-signing-enforce-rejects-unsigned-package-rejected.

Cause 2 of 8: there is no in-flight update window, so a concurrent code update is never refused.
AWS: FunctionConfiguration carries State/LastUpdateStatus; while LastUpdateStatus=InProgress a second UpdateFunctionCode is refused with ResourceConflictException (modeled on UpdateFunctionCode, client, HTTP 409), and the previous code keeps serving invocations until the update completes.
Emulator: UpdateFunctionCode issued while GetFunctionConfiguration still reports LastUpdateStatus='InProgress' (LastUpdateStatusReasonCode='Creating') returns 200 with a full FunctionConfiguration instead of a 409 ResourceConflictException; and across three padded updates no InProgress window ever stayed open across a mid-flight Invoke (the status read back 'Successful'), so the update is applied synchronously.
Why: a client's deploy orchestration cannot observe or handle the conflict AWS raises, and cannot test the "previous code keeps serving" guarantee.
Fix: function-state/LastUpdateStatus state machine and the conflict check in the UpdateFunctionCode / UpdateFunctionConfiguration handlers.
Traces: lambda.deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected, lambda.deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code.

Cause 3 of 8: Invoke's ResourceNotFoundException message appends a qualifier the caller did not send.
AWS: Invoke models ResourceNotFoundException (client, HTTP 404); for an unqualified function ARN the message is "Function not found: <the ARN as supplied>" — the fixed text is "Function not found: ", the variable part is exactly the FunctionName/ARN from the request.
Emulator: the message is "Function not found: <arn>:$LATEST" — it appends ':$LATEST' to an unqualified ARN.
Why: clients and tooling that match or echo the message (and users reading it) see an identifier that was never requested.
Fix: the ResourceNotFoundException message construction in the Invoke handler.
Traces: lambda.event-invoke-failure-not-surfaced.

Cause 4 of 8: a function ARN from another region is reported as not found.
AWS: Invoke models ResourceNotFoundException (404) and answers a cross-region ARN with the region-mismatch message "Functions from '<arn region>' are not reachable in this region ('<endpoint region>')" — fixed text with the two regions taken from the request ARN and the endpoint.
Emulator: it answers "Function not found: <the cross-region ARN>" instead.
Why: a client misreads a region-targeting bug as a missing function and retries or recreates the function in the wrong region.
Fix: the Invoke handler's ARN parsing/region check and its ResourceNotFoundException message.
Traces: lambda.invoke-refuses-a-function-arn-from-another-region.

Cause 5 of 8: InvokeWithResponseStream refuses the custom runtime it must support.
AWS: InvokeWithResponseStream is modeled (POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations) with InvalidRuntimeException typed as a SERVER error (HTTP 502) — i.e. not the answer for a validly configured function. provided.al2023 is a supported Lambda runtime identifier.
Emulator: InvokeWithResponseStream (and Invoke) answer InvalidRuntimeException with message "The runtime provided.al2023 is not supported by this emulator, which runs the Python and node runtimes." — an emulator-authored message naming its own limits, and the SDK retries it to max retries first.
Why: a client exercising a custom-runtime streaming function gets a 502-class server error instead of its response stream; the message tells it about the emulator rather than about AWS.
Fix: custom-runtime (provided.*) support in the invoke path, or at minimum the error classification/message of the Invoke and InvokeWithResponseStream handlers.
Traces: lambda.custom-runtime-streaming-response-mode-delivers-chunks-via-invoke-with-response-stream.

Cause 6 of 8: InvokeWithResponseStream buffers the whole response instead of streaming it.
AWS: the operation's output member EventStream is @streaming (httpPayload), so chunks reach the caller as the handler writes them, spread over time.
Emulator: all chunks arrive within 0.000 s of each other (inter-arrival times ~1.4e-4 s), i.e. the body is produced and then flushed at once; a related check sees the stream end mid-line with trailing bytes '{"errorMessage":"<id> Task timed out after 30.00 seconds","trace":[]}'.
Why: a client that streams to reduce time-to-first-byte, or that consumes a long-running response incrementally, cannot observe or validate that behaviour.
Fix: response streaming in the InvokeWithResponseStream handler/serializer.
Traces: lambda.invoke-with-response-stream-managed-streaming, lambda.deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes.

Cause 7 of 8: Lambda extensions never run and the Extensions API lifecycle is not enforced.
AWS: recorded behavior only; AWS executes every executable under /opt/extensions during init, requires each external extension to call Register before init completes, fails the invocation when an extension exits before registering or never registers, and refuses a late Register.
Emulator: nothing under /opt/extensions/ is executed (the extension never wrote its /tmp file); a never-registering extension and a crashing extension both leave the invoke answering 200 with FunctionError absent and the handler's normal payload; no late-register attempt is recorded at all; and in the cross-runtime leg the extension's presence turns the invoke into Unhandled "<id> Task timed out after 4.00 seconds".
Why: a client cannot test an extension (telemetry agent, secrets cache) at all, and a broken extension that AWS would surface as a failed invocation passes silently.
Fix: extension discovery/execution and the Extensions API (Register / INVOKE / SHUTDOWN) in the invoke execution path.
Traces: lambda.external-extension-registers-receives-invoke-and-shutdown-events, lambda.extension-that-never-registers-fails-the-invoke, lambda.external-extension-that-exits-before-registering-fails-the-invoke, lambda.extension-register-after-init-is-refused, lambda.external-extension-layer-runs-identically-across-runtime-families.

Cause 8 of 8: AWS_LAMBDA_EXEC_WRAPPER is ignored.
AWS: recorded behavior only; when AWS_LAMBDA_EXEC_WRAPPER names an executable, Lambda starts the runtime through that wrapper (so the wrapper's exported variables and arguments are visible to the handler) and fails the invoke at runtime when it names a missing file.
Emulator: the wrapper is not run — the handler's environment does not contain the wrapper's export (observed None), and a wrapper naming a missing file still yields a successful invoke (error=None, payload {"wrapped": null}) instead of a runtime failure.
Why: the documented hook for injecting profilers/agents silently does nothing, and a misconfigured wrapper looks healthy.
Fix: exec-wrapper handling in the runtime bootstrap of the invoke execution path.
Traces: lambda.exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments, lambda.exec-wrapper-naming-a-missing-file-rejected-at-invoke.

## Evidence
Expected vs actual, from the replay:
- code-signing-enforce-rejects-unsigned-package-rejected: expected a refusal (CodeVerificationFailedException, 400); AssertionError: the call was expected to be refused but it SUCCEEDED — an unsigned deployment package was accepted under an Enforce code-signing policy
- deep-lambda-1-fn-state-vpc-attach-in-progress-invoke-allowed-code-update-rejected: expected ResourceConflictException (409); got 200 with FunctionConfiguration (LastUpdateStatus 'InProgress', LastUpdateStatusReasonCode 'Creating')
- deep-lambda-1-fn-state-padded-code-update-in-progress-serves-previous-code: AssertionError: in 3 padded updates, no InProgress window stayed open across the mid-flight Invoke and the competing UpdateFunctionCode
- event-invoke-failure-not-surfaced: expected "Function not found: arn:aws:lambda:<region>:<acct>:function:<name>"; got "Function not found: arn:aws:lambda:<region>:<acct>:function:<name>:$LATEST"
- invoke-refuses-a-function-arn-from-another-region: expected "Functions from 'us-east-1' are not reachable in this region ('us-west-2')"; got "Function not found: arn:aws:lambda:us-east-1:<acct>:function:<name>"
- custom-runtime-streaming-response-mode-delivers-chunks-via-invoke-with-response-stream: botocore.errorfactory.InvalidRuntimeException on InvokeWithResponseStream (after max retries): "The runtime provided.al2023 is not supported by this emulator, which runs the Python and node runtimes."
- invoke-with-response-stream-managed-streaming: AssertionError: chunks arrived within 0.000s of each other - looks buffered, not streamed
- deep-lambda-runtime-truth-stream-first-chunk-reaches-client-before-handler-finishes: AssertionError: stream ended mid-line: trailing bytes b'{"errorMessage":"<id> Task timed out after 30.00 seconds","trace":[]}'
- external-extension-registers-receives-invoke-and-shutdown-events: AssertionError: the extension never wrote '/tmp/sim2real-ext.log'; nothing under /opt/extensions/ was executed
- extension-that-never-registers-fails-the-invoke: AssertionError: never-registering extension: status=200 FunctionError=None raw=b'{"ok": true}'
- external-extension-that-exits-before-registering-fails-the-invoke: AssertionError: crashing extension: status=200 FunctionError=None raw=b'{"ok": true}'
- extension-register-after-init-is-refused: AssertionError: no late-register attempt recorded for this invocation: []
- external-extension-layer-runs-identically-across-runtime-families: AssertionError: python3.13/arm64: the extension must not break the invocation: FunctionError='Unhandled' "<id> Task timed out after 4.00 seconds"
- exec-wrapper-script-starts-the-runtime-with-its-environment-and-arguments: AssertionError: the wrapper's export is not in the handler's environment: None
- exec-wrapper-naming-a-missing-file-rejected-at-invoke: AssertionError: positive control (executable wrapper): error=None raw=b'{"wrapped": null}'

## Steps to Reproduce
1. Code signing: CreateCodeSigningConfig with UntrustedArtifactOnDeployment='Enforce', attach it to a function, then UpdateFunctionCode with an unsigned zip -> expect CodeVerificationFailedException 400, observe 200.
2. In-flight update: CreateFunction with a VpcConfig (or a padded code update), and while GetFunctionConfiguration reports LastUpdateStatus='InProgress' call UpdateFunctionCode -> expect ResourceConflictException 409, observe 200.
3. Messages: Invoke a non-existent function by unqualified ARN, and Invoke a function ARN from another region; compare the ResourceNotFoundException messages.
4. Streaming/custom runtime: CreateFunction with Runtime='provided.al2023' plus a bootstrap, call InvokeWithResponseStream; and call InvokeWithResponseStream on a supported runtime that writes chunks with delays, timing the chunk arrivals.
5. Extensions: attach a layer containing extensions/<name> (executable) and invoke; also use an extension that never registers, one that exits before registering, and one that registers late.
6. Exec wrapper: set AWS_LAMBDA_EXEC_WRAPPER to a layer-shipped wrapper that exports a variable, and to a missing file; invoke in both cases.
7. Re-replay the check ids in trace_ids.
