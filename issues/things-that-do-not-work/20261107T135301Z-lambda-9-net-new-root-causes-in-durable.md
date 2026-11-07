Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.durable-execution-callback-already-resolved-rejected lambda.durable-execution-callback-failure-flow lambda.durable-execution-callback-success-flow lambda.durable-execution-checkpoint-sequence-monotonic lambda.durable-execution-history-ordering lambda.durable-execution-kms-errors lambda.durable-execution-lifecycle lambda.durable-execution-name-conflict-rejected lambda.durable-execution-server-managed-append-only-history lambda.get-durable-execution-state-invalid-checkpoint-token-rejected lambda.list-durable-executions-by-function lambda.stop-durable-execution-already-terminal-is-a-no-op lambda.stop-durable-execution-terminal-state-is-a-no-op lambda.stream-url-error-after-first-write-truncates-the-200-body lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies lambda.url-function-timeout-rejected-with-502 lambda.url-invalid-status-code-result-rejected-with-502 lambda.url-response-over-6mb-rejected-with-502 lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied lambda.function-url-missing-permission-not-invoked lambda.url-aws-iam-signature-scoped-to-another-service-rejected lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity lambda.url-cookie-header-split-into-event-cookies-array lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers lambda.function-url-response-stream-mode-chunks-over-https lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment lambda.stream-response-chunks-map-onto-handler-writes lambda.e2e-sqs-trigger lambda.log-tail-python-json-extra-fields-merge-into-the-record lambda.log-tail-python-json-levels-keep-python-level-names lambda.log-tail-python-json-logging-exception-carries-error-fields lambda.log-tail-python-json-unhandled-error-record-uses-log-level-key lambda.log-tail-python-text-format-defaults-root-logger-to-warning lambda.log-tail-runtime-env-exposes-log-format-and-level lambda.logging-config-json-format-filters-application-log-level lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent lambda.print-appears-in-logs lambda.env-var-size-and-reserved-key-rejected lambda.50-distinct-code-artifacts-no-shared-code-store lambda.alias-weight-ladder-observed-distribution lambda.alias-weighted-routing-splits-traffic lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected lambda.invoke-status-codes lambda.reserved-zero-blocks-invoke lambda.url-deleted-config-stops-serving-the-url lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.durable-execution-callback-already-resolved-rejected|_wait_for_callback_id#5|Timeout|time.sleep(2)
Issue-Key: lambda.durable-execution-callback-failure-flow|_receive_callback_id#7|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=10)
Issue-Key: lambda.durable-execution-callback-success-flow|_wait_for_callback_id#6|Timeout|time.sleep(3.0)
Issue-Key: lambda.durable-execution-checkpoint-sequence-monotonic|run#5|AssertionError|assert status == "SUCCEEDED", f"execution ended in unexpected status {status}"
Issue-Key: lambda.durable-execution-history-ordering|run#5|AssertionError|assert status == "SUCCEEDED", f"execution ended in unexpected status {status}"
Issue-Key: lambda.durable-execution-kms-errors|run#20|Timeout|time.sleep(2)
Issue-Key: lambda.durable-execution-lifecycle|run#11|AssertionError|assert final["Status"] == "SUCCEEDED", f"expected SUCCEEDED, got {final['Status']}: {final.get('Error')}"
Issue-Key: lambda.durable-execution-name-conflict-rejected|run#7|AssertionError|raise AssertionError("Invoke did not return a DurableExecutionArn; durable functions unsupported here")
Issue-Key: lambda.durable-execution-server-managed-append-only-history|run#14|KeyError|assert decoded["result"]["value"] == "sim2real-behav-durable", decoded
Issue-Key: lambda.get-durable-execution-state-invalid-checkpoint-token-rejected|run#6|KeyError|arn = resp["DurableExecutionArn"]
Issue-Key: lambda.list-durable-executions-by-function|_poll_until#11|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.stop-durable-execution-already-terminal-is-a-no-op|run#6|KeyError|arn = resp["DurableExecutionArn"]
Issue-Key: lambda.stop-durable-execution-terminal-state-is-a-no-op|run#7|AssertionError|assert final["Status"] == "SUCCEEDED", f"expected the quick execution to finish SUCCEEDED, got {final['Status']}"
Issue-Key: lambda.stream-url-error-after-first-write-truncates-the-200-body|run#7|AssertionError|assert (full["status"], full["body"]) == (200, b"part-0;part-1;part-2;done"), (
Issue-Key: lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies|run#8|AssertionError|assert shaped["status"] == 201, f"the prelude's statusCode must be the status, got {shaped!r}"
Issue-Key: lambda.url-function-timeout-rejected-with-502|run#10|AssertionError|assert slow["body"] == b"Internal Server Error", (
Issue-Key: lambda.url-invalid-status-code-result-rejected-with-502|run#8|AssertionError|assert bad["body"] == b"Internal Server Error", (
Issue-Key: lambda.url-response-over-6mb-rejected-with-502|run#11|AssertionError|assert big["body"] == b"Internal Server Error", (
Issue-Key: lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error|run#10|AssertionError|assert failed["body"] == b"Internal Server Error", (
Issue-Key: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied|_poll_until#20|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Key: lambda.function-url-missing-permission-not-invoked|run#22|AssertionError|assert granted is not None, (
Issue-Key: lambda.url-aws-iam-signature-scoped-to-another-service-rejected|run#9|AssertionError|assert wrong["headers"].get("x-amzn-errortype") == "InvalidSignatureException", wrong["headers"]
Issue-Key: lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature|run#11|AssertionError|assert tampered["headers"].get("x-amzn-errortype") == "InvalidSignatureException", (
Issue-Key: lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity|run#8|AssertionError|assert unsigned["headers"].get("x-amzn-errortype") == "AccessDeniedException", unsigned["headers"]
Issue-Key: lambda.url-cookie-header-split-into-event-cookies-array|run#10|KeyError|assert ev["headers"]["cookie"] == "c1=v1; c2=v2; theme=dark", (
Issue-Key: lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers|run#21|AssertionError|assert (http_ctx["method"], http_ctx["path"], http_ctx["userAgent"]) == (
Issue-Key: lambda.function-url-response-stream-mode-chunks-over-https|_signed_get_lines#14|Timeout|with urllib.request.urlopen(req, timeout=timeout_s) as resp:
Issue-Key: lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment|run#22|AssertionError|assert warm["spread"] >= HANDLER_SLEEP_S / 2.0, (
Issue-Key: lambda.stream-response-chunks-map-onto-handler-writes|run#18|AssertionError|assert spread >= HANDLER_SLEEP_S / 2.0, (
Issue-Key: lambda.e2e-sqs-trigger|_wait_for_invocation_count#20|AssertionError|raise AssertionError(
Issue-Key: lambda.log-tail-python-json-extra-fields-merge-into-the-record|run#9|AssertionError|assert len(recs) == 1, f"expected one record with message X-{m}; tail {tail!r}"
Issue-Key: lambda.log-tail-python-json-levels-keep-python-level-names|run#9|AssertionError|assert body == {"effective": 20}, f"the root logger's effective level must be INFO (20): {body!r}"
Issue-Key: lambda.log-tail-python-json-logging-exception-carries-error-fields|run#9|AssertionError|assert len(recs) == 1, f"expected one record with message EXC-{m}; tail {tail!r}"
Issue-Key: lambda.log-tail-python-json-unhandled-error-record-uses-log-level-key|run#9|AssertionError|assert len(recs) == 1, f"expected one error record; tail {tail!r}"
Issue-Key: lambda.log-tail-python-text-format-defaults-root-logger-to-warning|run#11|AssertionError|assert len(rec) == 1, f"expected one {level} record for {prefix}-{m}; tail was {tail!r}"
Issue-Key: lambda.log-tail-runtime-env-exposes-log-format-and-level|run#11|AssertionError|assert body2 == {"AWS_LAMBDA_LOG_FORMAT": "JSON", "AWS_LAMBDA_LOG_LEVEL": "WARN"}, (
Issue-Key: lambda.logging-config-json-format-filters-application-log-level|run#21|AssertionError|assert debug_marker in tail, (
Issue-Key: lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda.print-appears-in-logs|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: lambda.env-var-size-and-reserved-key-rejected|run#13|AssertionError|assert big_msg == (
Issue-Key: lambda.50-distinct-code-artifacts-no-shared-code-store|run#46|Timeout|time.sleep(4.0)
Issue-Key: lambda.alias-weight-ladder-observed-distribution|_sample#16|Timeout|return {"rows": rows, "counts": counts, "bad": [r for r in rows if r[2] is not None]}
Issue-Key: lambda.alias-weighted-routing-splits-traffic|run#27|Timeout|resp = lam.invoke(FunctionName=f"{fn}:{alias}", Payload=b"{}")
Issue-Key: lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination|_receive#28|Timeout|got = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=1, WaitTimeSeconds=5)
Issue-Key: lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected|_poll_tag#12|Timeout|seen = _tags(lam, fn_arn)
Issue-Key: lambda.invoke-status-codes|_poll_log_marker#17|Timeout|time.sleep(interval_s)
Issue-Key: lambda.reserved-zero-blocks-invoke|_poll_marker#8|Timeout|time.sleep(1.0)
Issue-Key: lambda.url-deleted-config-stops-serving-the-url|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running|_error#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Commit: 76f746b5965121353b22ae227b995a601e952faf

# lambda: 9 net-new root causes in durable executions, function URLs, response streaming, invocation logging and never-settling control-plane reads

> _Note: this issue was identified against the code base at commit `76f746b5965121353b22ae227b995a601e952faf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Nine distinct net-new root causes under '### lambda'. None of these checks is marked ALREADY FILED or OWNED in the replay report. Open issues 20261027T203721Z-lambda-a-durable-execution-is-accepted-but-never-runs.md, 20261023T114649Z-lambda-function-urls-unrouted-configuration-blocks-dropped-from.md and 20261107T091747Z-lambda-loggingconfig-is-ignored-by-the-invoke-log.md describe neighbouring symptoms but carry none of the replay keys below.

Cause 1 of 9: durable executions are accepted but never execute to SUCCEEDED, and Invoke omits DurableExecutionArn.
AWS: Smithy models Invoke input DurableExecutionName (header X-Amz-Durable-Execution-Name, @length 1-64, pattern ^[a-zA-Z0-9-_]+$) and output DurableExecutionArn (header X-Amz-Durable-Execution-Arn), plus error DurableExecutionAlreadyStartedException (httpError 409). A durable invoke returns that ARN, the execution runs to Status SUCCEEDED, GetDurableExecutionState returns an append-only history with monotonic checkpoint sequence numbers and the step results, a duplicate name is refused 409, and StopDurableExecution on a terminal execution is a no-op.
Emulator: Invoke returns no DurableExecutionArn at all (KeyError 'DurableExecutionArn'; 'Invoke did not return a DurableExecutionArn; durable functions unsupported here'), executions that do start end Status FAILED — one with {'ErrorType': 'Runtime.ImportModuleError', 'ErrorMessage': "Unable to import module 'lambda_function': No module named 'aws_durable_execution_sdk_python'"} — history reads come back without the step result (KeyError 'result'), and the callback/KMS legs never settle inside the 60 s check budget.
Why: no durable-execution client can start, resume, inspect or stop an execution; a ListDurableExecutions consumer sees every execution FAILED.
Fix: the durable-execution engine and the DurableExecutionArn/state serialisers behind Invoke, GetDurableExecutionState, ListDurableExecutions, StopDurableExecution, plus the durable SDK module in the Python runtime.
Traces: lambda.durable-execution-callback-already-resolved-rejected, lambda.durable-execution-callback-failure-flow, lambda.durable-execution-callback-success-flow, lambda.durable-execution-checkpoint-sequence-monotonic, lambda.durable-execution-history-ordering, lambda.durable-execution-kms-errors, lambda.durable-execution-lifecycle, lambda.durable-execution-name-conflict-rejected, lambda.durable-execution-server-managed-append-only-history, lambda.get-durable-execution-state-invalid-checkpoint-token-rejected, lambda.list-durable-executions-by-function, lambda.stop-durable-execution-already-terminal-is-a-no-op, lambda.stop-durable-execution-terminal-state-is-a-no-op.

Cause 2 of 9: a function URL's 502 body is JSON instead of AWS's plain-text body.
AWS: when the function behind a URL times out, returns a malformed/invalid status code, exceeds the 6 MB response cap, or raises, the URL answers HTTP 502 with the body exactly 'Internal Server Error' (fixed text, no variable part) and no JSON envelope.
Emulator: the status is 502 but the body is b'{"message":"Internal Server Error"}'.
Why: browsers and SDK clients that match the documented body (or parse it as text) see a JSON object; error-page and retry logic keyed on the body diverges.
Fix: the function-URL data-plane error serializer.
Traces: lambda.stream-url-error-after-first-write-truncates-the-200-body, lambda.stream-url-http-response-stream-metadata-sets-status-headers-and-cookies, lambda.url-function-timeout-rejected-with-502, lambda.url-invalid-status-code-result-rejected-with-502, lambda.url-response-over-6mb-rejected-with-502, lambda.url-unhandled-function-error-rejected-with-502-without-leaking-the-error.

Cause 3 of 9: a function URL's AWS_IAM refusals use the wrong error code and are not re-evaluated when a grant is revoked.
AWS: an unsigned request to an AWS_IAM function URL, or one signed for another service, is refused 403 with x-amzn-ErrorType 'AccessDeniedException'; a request whose body was tampered with after signing is refused with x-amzn-ErrorType 'InvalidSignatureException'; anonymous access requires both lambda:InvokeFunctionUrl and lambda:InvokeFunction, and revoking lambda:InvokeFunction must start refusing 403 immediately.
Emulator: the refusals carry x-amzn-errortype 'AccessDenied' (unsigned and wrong-service) and 'SignatureDoesNotMatch' (tampered body) — neither is a modelled Lambda error code — and after revoking lambda:InvokeFunction the probe never observes three consecutive 403s inside the budget.
Why: SigV4 clients branch on x-amzn-ErrorType to decide whether to re-sign (InvalidSignatureException) or to fail (AccessDeniedException); and an authorization revocation is not honoured.
Fix: the function-URL authorizer and its error-code mapping.
Traces: lambda.deep-lambda-3-url-auth-without-invoke-function-grant-denied, lambda.function-url-missing-permission-not-invoked, lambda.url-aws-iam-signature-scoped-to-another-service-rejected, lambda.url-aws-iam-tampered-body-rejected-with-invalid-signature, lambda.url-aws-iam-unsigned-request-forbidden-signed-request-served-with-caller-identity.

Cause 4 of 9: the function-URL payload-2.0 event is incomplete.
AWS: the 2.0 event carries a 'cookies' array built by splitting the request's Cookie header, and requestContext.http has method, path, protocol, sourceIp and userAgent with the path as requested.
Emulator: the event has no 'cookies' key at all (KeyError 'cookie') and requestContext.http is wrong for the requested raw path/query.
Why: a handler reading event['cookies'] raises; path/query-driven routing inside the handler takes the wrong branch.
Fix: the function-URL event builder (payload format 2.0).
Traces: lambda.url-cookie-header-split-into-event-cookies-array, lambda.url-get-event-v2-carries-raw-path-query-and-lowercased-headers.

Cause 5 of 9: response streaming is buffered to completion and then re-framed.
AWS: InvokeWithResponseStream and a RESPONSE_STREAM function URL deliver each handler write as it happens, so frame arrival times track the handler's sleeps.
Emulator: 'the first and last frame arrived 0.000s apart, but the handler sleeps 2.7s between its first and last write — the response was buffered to completion and then re-framed. Arrivals: [0.0, 0.0, 0.0, 0.0]'; the same for a pre-warmed provisioned environment, and the function-URL streaming leg does not settle in the budget.
Why: a client that consumes a stream incrementally (progress, time-to-first-byte, long-running output) sees none of that behaviour and cannot distinguish streaming from buffered responses.
Fix: the streaming response path in the invoke runtime and the function-URL data plane.
Traces: lambda.function-url-response-stream-mode-chunks-over-https, lambda.stream-provisioned-streaming-function-streams-from-prewarmed-environment, lambda.stream-response-chunks-map-onto-handler-writes.

Cause 6 of 9: application log records are not produced in the configured format/level and the invocation log is not delivered.
AWS: with LoggingConfig.LogFormat=JSON each logging call becomes one structured record carrying level, message, timestamp, requestId and the logger's extra fields; python's root logger is set from ApplicationLogLevel (INFO -> effective level 20, default WARNING); an unhandled error becomes one ERROR record; the sandbox sees AWS_LAMBDA_LOG_FORMAT and AWS_LAMBDA_LOG_LEVEL; and the invocation's lines reach the log group.
Emulator: the tail contains only START/END/REPORT plus raw text lines, so expected records are missing ('expected one record with message X-…', 'expected one error record', 'expected one WARNING record'), the root logger's effective level stays 30 when INFO was configured, AWS_LAMBDA_LOG_FORMAT and AWS_LAMBDA_LOG_LEVEL read back None inside the sandbox, a DEBUG line never reaches the tail at all, and print()/log-group-override checks do not settle in the budget.
Why: a client that ships structured logs, filters by level, or asserts on CloudWatch content gets plain text with no level metadata and loses records entirely.
Fix: the runtime's logging bridge (structured record emission, ApplicationLogLevel/SystemLogLevel wiring, AWS_LAMBDA_LOG_* env vars) and log delivery to CloudWatch Logs.
Traces: lambda.e2e-sqs-trigger, lambda.log-tail-python-json-extra-fields-merge-into-the-record, lambda.log-tail-python-json-levels-keep-python-level-names, lambda.log-tail-python-json-logging-exception-carries-error-fields, lambda.log-tail-python-json-unhandled-error-record-uses-log-level-key, lambda.log-tail-python-text-format-defaults-root-logger-to-warning, lambda.log-tail-runtime-env-exposes-log-format-and-level, lambda.logging-config-json-format-filters-application-log-level, lambda.logging-config-log-group-override-routes-logs-and-leaves-the-default-group-absent, lambda.print-appears-in-logs.

Cause 7 of 9: the 4 KB environment-variable refusal quotes the serialized map instead of the measured size.
AWS: CreateFunction/UpdateFunctionConfiguration refuse an oversized Environment with InvalidParameterValueException (Smithy: CreateFunction error InvalidParameterValueException, httpError 400) and the message 'Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. String measured: <measured size>' — the sentence is fixed text and the only variable part is the measured byte count taken from the request.
Emulator: the same fixed sentence is followed by the entire serialized variable map (thousands of characters of the request's values) instead of the measured size.
Why: a client that reads the measured size out of the refusal to report how far over the limit it is cannot parse the message, and the refusal leaks the whole environment.
Fix: the environment-size validation message in the CreateFunction/UpdateFunctionConfiguration validation.
Traces: lambda.env-var-size-and-reserved-key-rejected.

Cause 8 of 9: ten control-plane/alias/concurrency reads never settle inside the 60 s check budget.
AWS: CreateFunction/PublishVersion/CreateAlias with routing weights, PutFunctionConcurrency(ReservedConcurrentExecutions=0) (Smithy: @range min 0), Invoke status codes, PutRuntimeManagementConfig and DeleteFunctionUrlConfig all answer in well under a second, and the behaviour they configure is observable on the next call.
Emulator: each of these checks ends with 'Error: Check timed out after 60.0s' — the emulator never reaches a state where the configured behaviour is observable (no response, or a converge-loop that never converges).
Why: a client cannot complete an alias-weighting, runtime-pin, reserved-concurrency, async-payload or function-URL-deletion workflow at all; the call appears to hang.
Fix: the handlers/state for weighted aliases, runtime-management config, reserved concurrency, asynchronous invoke payload handling and DeleteFunctionUrlConfig, plus whatever convergence loop they share.
Traces: lambda.50-distinct-code-artifacts-no-shared-code-store, lambda.alias-weight-ladder-observed-distribution, lambda.alias-weighted-routing-splits-traffic, lambda.async-invoke-alias-qualifier-applies-to-onfailure-destination, lambda.deep-lambda-3-runtime-pin-foreign-runtime-arn-rejected, lambda.deep-lambda-3-runtime-pin-manual-arn-survives-cold-start, lambda.deep-lambda-runtime-truth-async-payload-band-delivered-intact-over-1mb-rejected, lambda.invoke-status-codes, lambda.reserved-zero-blocks-invoke, lambda.url-deleted-config-stops-serving-the-url.

Cause 9 of 9: UpdateFunctionCode accepts a corrupt deployment package.
AWS: Smithy models UpdateFunctionCode errors InvalidParameterValueException (400) and CodeVerificationFailedException (400); a ZipFile that is not a readable zip is refused and the function keeps running its previous code.
Emulator: the call succeeds with HTTP 200 and a FunctionConfiguration naming the new CodeSha256, so the corrupt artifact is adopted.
Why: a client's bad build is accepted and only discovered at invoke time (or not at all), instead of being refused at update time with the previous code left serving.
Fix: zip validation in the UpdateFunctionCode handler.
Traces: lambda.update-function-code-corrupt-zip-rejected-previous-code-keeps-running.

## Evidence
Cause 1: KeyError 'DurableExecutionArn'; 'Invoke did not return a DurableExecutionArn; durable functions unsupported here'; 'expected SUCCEEDED, got FAILED: {ErrorMessage: "Unable to import module \'lambda_function\': No module named \'aws_durable_execution_sdk_python\'", ErrorType: Runtime.ImportModuleError}'; ListDurableExecutions returned both executions with Status 'FAILED'.
Cause 2: expected 502 body 'Internal Server Error', got b'{"message":"Internal Server Error"}' (four URL checks) and status 502 with that body where every streamed write should have arrived.
Cause 3: headers {'x-amzn-errortype': 'AccessDenied'} for an unsigned and for a wrong-service-signed request; 'expected x-amzn-ErrorType InvalidSignatureException, got {x-amzn-errortype: SignatureDoesNotMatch}'; TimeoutError 'three consecutive 403s after revoking lambda:InvokeFunction'.
Cause 4: KeyError 'cookie'; "requestContext.http is wrong: {'method': 'GET', 'path': '/some/pa%20th/x', 'protocol': 'HTTP/1.1', 'sourceIp': '127.0.0.1', 'userAgent': 'sim2real-agent'}".
Cause 5: 'the first and last frame arrived 0.000s apart, but the handler sleeps 2.7s between its first and last write … Arrivals: [0.0, 0.0, 0.0, 0.0]'; same with 2.1 s for the provisioned variant.
Cause 6: tail 'START RequestId: … \nEND RequestId: …\nREPORT RequestId: …' with no application record; "the root logger's effective level must be INFO (20): {'effective': 30}"; "after the switch the sandbox must see JSON/WARN: {'AWS_LAMBDA_LOG_FORMAT': None, 'AWS_LAMBDA_LOG_LEVEL': None}"; 'expected >= 1 log line(s) matching sim2real-trigger-fired … found 0'.
Cause 7: 'Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit. String measured: {"SIM2REAL_BLOB":"xxxx…"}' (the whole map) instead of the measured byte count.
Cause 8: 'Error: Check timed out after 60.0s' for all ten listed check ids.
Cause 9: 'expected the call to be refused, but it succeeded: {ResponseMetadata: {HTTPStatusCode: 200}, FunctionName: sim2real-behav-l8-corrupt-…}'.

## Steps to Reproduce
1. Clone the repo and run the behavioral suite (run_behavioral_suite).
2. Re-replay the check ids in trace_ids; all are listed under '### lambda'.
3. Minimal repro for cause 1: Invoke a function with X-Amz-Durable-Execution-Name set — the response has no X-Amz-Durable-Execution-Arn header.
4. Minimal repro for cause 2: CreateFunctionUrlConfig(AuthType='NONE') on a handler that raises, then GET the URL — 502 with body {"message":"Internal Server Error"}.
5. Minimal repro for cause 3: GET an AWS_IAM function URL unsigned — 403 with x-amzn-errortype 'AccessDenied'.
6. Minimal repro for cause 9: UpdateFunctionCode with ZipFile=b'not-a-zip' — the call returns 200.
